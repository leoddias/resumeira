//! A rebuildable SQLite index over the notes on disk (ADR-0007).
//!
//! Every row here is derived from a `notes.md` file via
//! [`crate::storage::read_note`]. The database is disposable by design: if
//! it is lost or corrupted, [`rebuild_from_disk`] regenerates every row from
//! the meeting folders alone. An index write always follows a successful
//! file write, never precedes it — this module never invents note content,
//! only indexes what [`crate::storage`] already wrote.
//!
//! Never logs note content or transcript text (docs/CONVENTIONS.md §
//! Privacy) — only folder paths and counts.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use crate::storage::{self, ParsedNote};

/// One indexed note, as returned by [`list`] and [`search`].
#[derive(Debug, Clone, PartialEq)]
pub struct NoteRecord {
    pub folder: PathBuf,
    pub title: String,
    pub engine: String,
    pub model: String,
    pub language: Option<String>,
    pub created_at: String,
    /// First couple of lines of the summary, for a list row. Kept here so
    /// listing meetings is one query rather than one file read per meeting.
    pub preview: String,
}

/// Anything that can go wrong opening or querying the index.
///
/// Carries paths and error kinds only — never note content or transcript
/// text (docs/CONVENTIONS.md § Privacy).
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("opening the index at '{path}' failed: {source}")]
    Open {
        path: String,
        #[source]
        source: rusqlite::Error,
    },
    #[error("preparing index directory '{path}' failed: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("index query failed: {source}")]
    Query {
        #[source]
        source: rusqlite::Error,
    },
}

/// Opens (creating if needed) the index database at `db_path` and ensures
/// its schema exists. Safe to call on a path that does not exist yet — the
/// parent directory and the database file are both created — and safe to
/// call again after the file has been deleted, since the schema is
/// recreated from scratch.
///
/// `notes_fts` is a standalone FTS5 table, added purely additively — it is
/// never required for `notes` to exist and `notes` never depends on it, so
/// opening an older database that only has `notes` is not a migration, just
/// one more `CREATE TABLE IF NOT EXISTS`. Any row already in `notes` that is
/// missing from `notes_fts` (an older database opened for the first time
/// after this change, or a database that was rebuilt before this table
/// existed) is backfilled on every open, so search "just works" for
/// existing users without a version check or a destructive rewrite. If
/// `notes_fts` itself ever gets corrupted or out of sync, it is exactly as
/// disposable as the rest of the index: `rebuild_from_disk` regenerates it
/// from the notes on disk like everything else.
pub fn open(db_path: &Path) -> Result<Connection, IndexError> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| IndexError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }

    let conn = Connection::open(db_path).map_err(|source| IndexError::Open {
        path: db_path.display().to_string(),
        source,
    })?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS notes (
            folder      TEXT PRIMARY KEY,
            title       TEXT NOT NULL,
            engine      TEXT NOT NULL,
            model       TEXT NOT NULL,
            language    TEXT,
            created_at  TEXT NOT NULL,
            summary     TEXT NOT NULL,
            transcript  TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS notes_created_at ON notes (created_at DESC);
        CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
            folder UNINDEXED,
            title,
            summary,
            transcript
        );
        INSERT INTO notes_fts (folder, title, summary, transcript)
        SELECT folder, title, summary, transcript FROM notes
        WHERE folder NOT IN (SELECT folder FROM notes_fts);",
    )
    .map_err(|source| IndexError::Query { source })?;

    Ok(conn)
}

/// Replaces `folder`'s row in `notes_fts`, if any, with the given text.
/// Delete-then-insert because FTS5 has no `ON CONFLICT` support to upsert
/// against.
fn upsert_fts(conn: &Connection, folder: &str, note: &ParsedNote) -> Result<(), IndexError> {
    conn.execute("DELETE FROM notes_fts WHERE folder = ?1", params![folder])
        .map_err(|source| IndexError::Query { source })?;
    conn.execute(
        "INSERT INTO notes_fts (folder, title, summary, transcript) VALUES (?1, ?2, ?3, ?4)",
        params![folder, note.title, note.summary_text, note.transcript_text],
    )
    .map_err(|source| IndexError::Query { source })?;
    Ok(())
}

/// Inserts or replaces a note's row, keyed by its folder path, in both
/// `notes` and its `notes_fts` search shadow.
///
/// Call this only after [`storage::write_note`] has already succeeded — the
/// file is the source of truth and the index write must never race ahead of
/// it (ADR-0007).
///
/// Both tables are written inside one transaction (`unchecked_transaction`
/// works from a shared `&Connection`, which is what callers hold behind a
/// mutex) so a mid-write failure cannot leave `notes` and `notes_fts`
/// disagreeing about a folder.
pub fn upsert(conn: &Connection, folder: &Path, note: &ParsedNote) -> Result<(), IndexError> {
    let folder_key = folder.display().to_string();
    let tx = conn
        .unchecked_transaction()
        .map_err(|source| IndexError::Query { source })?;

    tx.execute(
        "INSERT INTO notes (folder, title, engine, model, language, created_at, summary, transcript)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(folder) DO UPDATE SET
            title = excluded.title,
            engine = excluded.engine,
            model = excluded.model,
            language = excluded.language,
            created_at = excluded.created_at,
            summary = excluded.summary,
            transcript = excluded.transcript",
        params![
            folder_key,
            note.title,
            note.engine,
            note.model,
            note.language,
            note.created_at,
            note.summary_text,
            note.transcript_text,
        ],
    )
    .map_err(|source| IndexError::Query { source })?;

    upsert_fts(&tx, &folder_key, note)?;

    tx.commit().map_err(|source| IndexError::Query { source })?;
    Ok(())
}

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<NoteRecord> {
    let folder: String = row.get("folder")?;
    Ok(NoteRecord {
        folder: PathBuf::from(folder),
        title: row.get("title")?,
        engine: row.get("engine")?,
        model: row.get("model")?,
        language: row.get("language")?,
        created_at: row.get("created_at")?,
        preview: preview_of(&row.get::<_, String>("summary")?),
    })
}

/// How much of a summary a list row shows.
const PREVIEW_CHARS: usize = 180;

/// Condenses a summary into one line for a list row.
///
/// Truncates on a character boundary, not a byte one — meeting summaries are
/// routinely non-ASCII and slicing bytes would panic.
fn preview_of(summary: &str) -> String {
    let flattened = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    if flattened.chars().count() <= PREVIEW_CHARS {
        return flattened;
    }
    let cut: String = flattened.chars().take(PREVIEW_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// Every indexed note, newest first.
pub fn list(conn: &Connection) -> Result<Vec<NoteRecord>, IndexError> {
    let mut stmt = conn
        .prepare(
            "SELECT folder, title, engine, model, language, created_at, summary
             FROM notes ORDER BY created_at DESC, folder DESC",
        )
        .map_err(|source| IndexError::Query { source })?;
    let rows = stmt
        .query_map([], row_to_record)
        .map_err(|source| IndexError::Query { source })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|source| IndexError::Query { source })
}

/// Builds an FTS5 `MATCH` expression that ANDs together every whitespace
/// token of `query`, each wrapped as its own literal phrase.
///
/// Quoting each token (and doubling any embedded `"`) means the tokenizer
/// still splits it into words — so `budget?` matches a stored `budget` since
/// punctuation is not part of the token — while none of FTS5's query syntax
/// (`AND`, `-`, `*`, `(`, `:`, ...) inside a user's query is interpreted as
/// an operator and cannot produce a syntax error or an unintended query
/// shape.
fn build_match_query(trimmed: &str) -> String {
    trimmed
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// Full-text search over title, summary and transcript, best match first.
///
/// Backed by SQLite FTS5 (`notes_fts`): the query and the indexed text are
/// both tokenized, so results rank by relevance (`bm25`) instead of matching
/// only the exact substring a `LIKE` query would need. A blank query returns
/// nothing rather than the whole index — callers that want everything
/// should use [`list`].
pub fn search(conn: &Connection, query: &str) -> Result<Vec<NoteRecord>, IndexError> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let match_query = build_match_query(trimmed);

    let mut stmt = conn
        .prepare(
            "SELECT n.folder, n.title, n.engine, n.model, n.language, n.created_at, n.summary
             FROM notes_fts
             JOIN notes n ON n.folder = notes_fts.folder
             WHERE notes_fts MATCH ?1
             -- bm25 weights follow the table's column order (folder, title,
             -- summary, transcript); a title hit ranks well above the same
             -- word buried in a long transcript. Lower bm25 is a better
             -- match, hence ASC.
             ORDER BY bm25(notes_fts, 0.0, 10.0, 3.0, 1.0) ASC, n.created_at DESC, n.folder DESC",
        )
        .map_err(|source| IndexError::Query { source })?;
    let rows = stmt
        .query_map(params![match_query], row_to_record)
        .map_err(|source| IndexError::Query { source })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|source| IndexError::Query { source })
}

/// Rebuilds the entire index from the meeting folders under `notes_root`.
///
/// This is the recovery path ADR-0007 promises: deleting the database (or
/// losing it to corruption) must never lose a note, because every row here
/// is derived from a `notes.md` file, never the other way around. A folder
/// whose note fails to parse is skipped, not fatal to the rest of the
/// rebuild — one bad file must not hide every other meeting.
///
/// Returns how many notes were indexed.
pub fn rebuild_from_disk(conn: &Connection, notes_root: &Path) -> Result<usize, IndexError> {
    // Both tables are cleared inside one transaction so a crash between the
    // two deletes cannot leave `notes_fts` holding rows for folders that
    // `notes` has already forgotten.
    let tx = conn
        .unchecked_transaction()
        .map_err(|source| IndexError::Query { source })?;
    tx.execute("DELETE FROM notes", [])
        .map_err(|source| IndexError::Query { source })?;
    tx.execute("DELETE FROM notes_fts", [])
        .map_err(|source| IndexError::Query { source })?;
    tx.commit().map_err(|source| IndexError::Query { source })?;

    let entries = match std::fs::read_dir(notes_root) {
        Ok(entries) => entries,
        // No notes root yet is not an error: an empty index is correct.
        Err(_) => return Ok(0),
    };

    let mut indexed = 0usize;
    for entry in entries.flatten() {
        let folder = entry.path();
        if !folder.is_dir() {
            continue;
        }
        match storage::read_note(&folder) {
            Ok(note) => {
                upsert(conn, &folder, &note)?;
                indexed += 1;
            }
            Err(error) => {
                // Path and error kind only — never note content.
                log::warn!("rebuild: skipping '{}': {error}", folder.display());
            }
        }
    }

    Ok(indexed)
}

#[cfg(test)]
mod preview_tests {
    use super::preview_of;

    #[test]
    fn a_short_summary_is_its_own_preview() {
        assert_eq!(preview_of("Short one."), "Short one.");
    }

    #[test]
    fn line_breaks_collapse_into_one_line() {
        assert_eq!(
            preview_of(
                "First line.

- a bullet"
            ),
            "First line. - a bullet"
        );
    }

    #[test]
    fn a_long_summary_is_cut_on_a_character_boundary() {
        // Multibyte throughout: a byte-based slice would panic here.
        let long = "ação ".repeat(200);
        let preview = preview_of(&long);
        assert!(preview.ends_with('…'));
        assert!(preview.chars().count() <= 181);
    }

    #[test]
    fn an_empty_summary_previews_as_empty() {
        assert_eq!(
            preview_of(
                "   
  "
            ),
            ""
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarize::Summary;
    use crate::transcribe::{Engine, Segment, Transcript};
    use tempfile::tempdir;

    fn transcript(text: &str) -> Transcript {
        Transcript {
            segments: vec![Segment {
                start: 0.0,
                end: 1.0,
                text: text.to_owned(),
                track: None,
            }],
            language: Some("en".to_owned()),
            engine: Engine::Local,
        }
    }

    fn summary(title: &str) -> Summary {
        Summary {
            title: title.to_owned(),
            bullets: vec!["a bullet point".to_owned()],
            decisions: vec![],
            action_items: vec![],
            model: "test-model".to_owned(),
        }
    }

    /// Writes a real note to disk via `storage::write_note` and returns its
    /// folder, so index tests exercise the same on-disk shape rebuild does.
    fn write_meeting(root: &Path, name: &str, title: &str, transcript_text: &str) -> PathBuf {
        let folder = root.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        storage::write_note(&folder, &summary(title), &transcript(transcript_text)).unwrap();
        folder
    }

    #[test]
    fn upsert_then_list_returns_the_note() {
        let dir = tempdir().unwrap();
        let conn = open(&dir.path().join("index.sqlite")).unwrap();
        let notes_root = tempdir().unwrap();
        let folder = write_meeting(notes_root.path(), "m1", "Weekly sync", "hello there");
        let note = storage::read_note(&folder).unwrap();

        upsert(&conn, &folder, &note).unwrap();

        let listed = list(&conn).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].folder, folder);
        assert_eq!(listed[0].title, "Weekly sync");
    }

    #[test]
    fn upsert_is_idempotent_on_the_same_folder() {
        let dir = tempdir().unwrap();
        let conn = open(&dir.path().join("index.sqlite")).unwrap();
        let notes_root = tempdir().unwrap();
        let folder = write_meeting(notes_root.path(), "m1", "First title", "text");
        let mut note = storage::read_note(&folder).unwrap();

        upsert(&conn, &folder, &note).unwrap();
        note.title = "Updated title".to_owned();
        upsert(&conn, &folder, &note).unwrap();

        let listed = list(&conn).unwrap();
        assert_eq!(listed.len(), 1, "same folder must replace, not duplicate");
        assert_eq!(listed[0].title, "Updated title");
    }

    #[test]
    fn list_is_newest_first() {
        let dir = tempdir().unwrap();
        let conn = open(&dir.path().join("index.sqlite")).unwrap();
        let notes_root = tempdir().unwrap();

        let older = write_meeting(notes_root.path(), "older", "Older meeting", "text");
        let mut older_note = storage::read_note(&older).unwrap();
        older_note.created_at = "2026-01-01T09:00:00+00:00".to_owned();
        upsert(&conn, &older, &older_note).unwrap();

        let newer = write_meeting(notes_root.path(), "newer", "Newer meeting", "text");
        let mut newer_note = storage::read_note(&newer).unwrap();
        newer_note.created_at = "2026-06-01T09:00:00+00:00".to_owned();
        upsert(&conn, &newer, &newer_note).unwrap();

        let listed = list(&conn).unwrap();
        assert_eq!(listed[0].title, "Newer meeting");
        assert_eq!(listed[1].title, "Older meeting");
    }

    #[test]
    fn search_finds_a_hit_in_the_title() {
        let dir = tempdir().unwrap();
        let conn = open(&dir.path().join("index.sqlite")).unwrap();
        let notes_root = tempdir().unwrap();
        let folder = write_meeting(notes_root.path(), "m1", "Roadmap review", "unrelated text");
        let note = storage::read_note(&folder).unwrap();
        upsert(&conn, &folder, &note).unwrap();

        let hits = search(&conn, "roadmap").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Roadmap review");
    }

    #[test]
    fn search_finds_a_hit_in_the_transcript() {
        let dir = tempdir().unwrap();
        let conn = open(&dir.path().join("index.sqlite")).unwrap();
        let notes_root = tempdir().unwrap();
        let folder = write_meeting(
            notes_root.path(),
            "m1",
            "Unrelated title",
            "we discussed the quarterly budget",
        );
        let note = storage::read_note(&folder).unwrap();
        upsert(&conn, &folder, &note).unwrap();

        let hits = search(&conn, "budget").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Unrelated title");
    }

    #[test]
    fn search_matches_a_word_regardless_of_query_punctuation() {
        // The old `LIKE '%pattern%'` implementation matched only the exact
        // substring typed: a query of "budget?" would never find a
        // transcript that says "budget" without a question mark, since
        // "budget?" is not a substring of "budget". FTS5 tokenizes the
        // query the same way it tokenizes indexed text, so punctuation
        // attached to a search term does not change what it matches.
        let dir = tempdir().unwrap();
        let conn = open(&dir.path().join("index.sqlite")).unwrap();
        let notes_root = tempdir().unwrap();
        let folder = write_meeting(
            notes_root.path(),
            "m1",
            "Unrelated title",
            "we discussed the budget for Q3",
        );
        let note = storage::read_note(&folder).unwrap();
        upsert(&conn, &folder, &note).unwrap();

        let hits = search(&conn, "budget?").unwrap();
        assert_eq!(hits.len(), 1, "FTS5 should tokenize past the punctuation");
        assert_eq!(hits[0].title, "Unrelated title");
    }

    #[test]
    fn search_ranks_a_title_hit_above_a_transcript_only_hit() {
        let dir = tempdir().unwrap();
        let conn = open(&dir.path().join("index.sqlite")).unwrap();
        let notes_root = tempdir().unwrap();

        let buried = write_meeting(
            notes_root.path(),
            "m1",
            "Unrelated title",
            "roadmap roadmap roadmap roadmap came up once near the end",
        );
        let buried_note = storage::read_note(&buried).unwrap();
        upsert(&conn, &buried, &buried_note).unwrap();

        let prominent = write_meeting(notes_root.path(), "m2", "Roadmap review", "short text");
        let prominent_note = storage::read_note(&prominent).unwrap();
        upsert(&conn, &prominent, &prominent_note).unwrap();

        let hits = search(&conn, "roadmap").unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].title, "Roadmap review",
            "bm25 ranking should favour the note whose title is the match"
        );
    }

    #[test]
    fn upsert_keeps_notes_fts_in_sync_when_a_note_is_updated() {
        let dir = tempdir().unwrap();
        let conn = open(&dir.path().join("index.sqlite")).unwrap();
        let notes_root = tempdir().unwrap();
        let folder = write_meeting(notes_root.path(), "m1", "First title", "alpha content");
        let mut note = storage::read_note(&folder).unwrap();
        upsert(&conn, &folder, &note).unwrap();
        assert_eq!(search(&conn, "alpha").unwrap().len(), 1);

        note.title = "Second title".to_owned();
        note.transcript_text = "beta content".to_owned();
        upsert(&conn, &folder, &note).unwrap();

        assert!(
            search(&conn, "alpha").unwrap().is_empty(),
            "the stale transcript text must not still be searchable"
        );
        let hits = search(&conn, "beta").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Second title");
    }

    #[test]
    fn opening_a_database_that_predates_notes_fts_backfills_search() {
        // Simulates an existing user's database: only the `notes` table
        // exists (as it would have before this change), populated directly
        // rather than through `upsert`. Opening it must not require wiping
        // or migrating anything — `notes_fts` should simply appear and be
        // backfilled from the rows already there.
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("index.sqlite");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE notes (
                    folder      TEXT PRIMARY KEY,
                    title       TEXT NOT NULL,
                    engine      TEXT NOT NULL,
                    model       TEXT NOT NULL,
                    language    TEXT,
                    created_at  TEXT NOT NULL,
                    summary     TEXT NOT NULL,
                    transcript  TEXT NOT NULL
                );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO notes VALUES ('/legacy/m1', 'Legacy meeting', 'local', 'm', NULL, '2026-01-01T00:00:00+00:00', 'a legacy summary', 'a legacy transcript about onboarding')",
                [],
            )
            .unwrap();
        }

        let conn = open(&db_path).unwrap();
        let hits = search(&conn, "onboarding").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Legacy meeting");
    }

    #[test]
    fn search_with_no_match_is_empty_not_an_error() {
        let dir = tempdir().unwrap();
        let conn = open(&dir.path().join("index.sqlite")).unwrap();
        let notes_root = tempdir().unwrap();
        let folder = write_meeting(notes_root.path(), "m1", "Roadmap review", "text");
        let note = storage::read_note(&folder).unwrap();
        upsert(&conn, &folder, &note).unwrap();

        assert!(search(&conn, "nonexistent term").unwrap().is_empty());
        assert!(search(&conn, "   ").unwrap().is_empty());
    }

    #[test]
    fn rebuild_reconstructs_an_index_deleted_mid_life() {
        let notes_root = tempdir().unwrap();
        write_meeting(notes_root.path(), "m1", "First meeting", "alpha");
        write_meeting(notes_root.path(), "m2", "Second meeting", "beta");

        let db_dir = tempdir().unwrap();
        let db_path = db_dir.path().join("index.sqlite");
        {
            let conn = open(&db_path).unwrap();
            let count = rebuild_from_disk(&conn, notes_root.path()).unwrap();
            assert_eq!(count, 2);
            assert_eq!(list(&conn).unwrap().len(), 2);
        }

        // The database is lost — ADR-0007's whole premise is that this is
        // safe because the notes are still on disk.
        std::fs::remove_file(&db_path).unwrap();

        let conn = open(&db_path).unwrap();
        assert_eq!(list(&conn).unwrap().len(), 0, "a fresh db starts empty");

        let count = rebuild_from_disk(&conn, notes_root.path()).unwrap();
        assert_eq!(count, 2);
        let titles: Vec<String> = list(&conn).unwrap().into_iter().map(|n| n.title).collect();
        assert!(titles.contains(&"First meeting".to_owned()));
        assert!(titles.contains(&"Second meeting".to_owned()));
    }

    #[test]
    fn rebuild_skips_a_folder_with_no_note_instead_of_failing() {
        let notes_root = tempdir().unwrap();
        write_meeting(notes_root.path(), "m1", "Real meeting", "text");
        std::fs::create_dir_all(notes_root.path().join("empty-folder")).unwrap();

        let db_dir = tempdir().unwrap();
        let conn = open(&db_dir.path().join("index.sqlite")).unwrap();
        let count = rebuild_from_disk(&conn, notes_root.path()).unwrap();

        assert_eq!(count, 1);
        assert_eq!(list(&conn).unwrap()[0].title, "Real meeting");
    }

    #[test]
    fn rebuild_on_a_missing_notes_root_yields_an_empty_index() {
        let db_dir = tempdir().unwrap();
        let conn = open(&db_dir.path().join("index.sqlite")).unwrap();
        let missing_root = db_dir.path().join("does-not-exist");

        let count = rebuild_from_disk(&conn, &missing_root).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn open_recreates_the_schema_on_a_brand_new_file() {
        let db_dir = tempdir().unwrap();
        let db_path = db_dir.path().join("nested").join("index.sqlite");
        let conn = open(&db_path).unwrap();
        assert!(list(&conn).unwrap().is_empty());
        assert!(db_path.exists());
    }
}
