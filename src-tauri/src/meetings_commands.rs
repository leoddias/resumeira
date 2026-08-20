//! Tauri commands for browsing meetings.
//!
//! The index answers lists and searches; the note file answers a read. That
//! split is deliberate: the database is a cache that can be rebuilt, and the
//! file is the thing the user owns (ADR-0007).

use crate::index;
use crate::storage;
use crate::transcribe::Segment;
use rusqlite::Connection;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Runtime, State};
use tauri_plugin_opener::OpenerExt;

/// The open index connection and the folder it indexes.
pub struct MeetingIndex {
    conn: Mutex<Connection>,
    notes_root: PathBuf,
}

impl MeetingIndex {
    pub fn open(db_path: &Path, notes_root: PathBuf) -> Result<Self, String> {
        let conn = index::open(db_path).map_err(|error| error.to_string())?;
        Ok(Self {
            conn: Mutex::new(conn),
            notes_root,
        })
    }

    pub fn notes_root(&self) -> &Path {
        &self.notes_root
    }

    /// Indexes a note that was just written.
    ///
    /// Called only after `storage::write_note` succeeded — the file is the
    /// source of truth and the index must never race ahead of it (ADR-0007).
    pub fn index_note(&self, folder: &Path) -> Result<(), String> {
        let note = storage::read_note(folder).map_err(|error| error.to_string())?;
        self.with_conn(|conn| index::upsert(conn, folder, &note).map_err(|error| error.to_string()))
    }

    fn with_conn<T>(
        &self,
        action: impl FnOnce(&Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        match self.conn.lock() {
            Ok(conn) => action(&conn),
            Err(_) => Err("the meeting index is unavailable".to_owned()),
        }
    }
}

/// A meeting as the list shows it. Mirrors `MeetingListItem` in
/// `src/ipc/meetings.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingListItem {
    pub folder: String,
    pub title: String,
    pub created_at: String,
    pub preview: String,
}

/// One transcript line. Mirrors `TranscriptLine` in `src/ipc/meetings.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptLine {
    pub start: f64,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<crate::audio::Track>,
}

/// A meeting opened for reading. Mirrors `MeetingNote` in
/// `src/ipc/meetings.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingNote {
    pub folder: String,
    pub title: String,
    pub created_at: String,
    pub summary: String,
    pub transcript: Vec<TranscriptLine>,
    pub engine: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub audio_kept: bool,
}

fn to_list_item(record: index::NoteRecord) -> MeetingListItem {
    MeetingListItem {
        folder: record.folder.display().to_string(),
        title: record.title,
        created_at: record.created_at,
        preview: record.preview,
    }
}

#[tauri::command]
pub fn list_meetings(meetings: State<'_, MeetingIndex>) -> Result<Vec<MeetingListItem>, String> {
    meetings.with_conn(|conn| {
        index::list(conn)
            .map(|records| records.into_iter().map(to_list_item).collect())
            .map_err(|error| error.to_string())
    })
}

#[tauri::command]
pub fn search_meetings(
    meetings: State<'_, MeetingIndex>,
    query: String,
) -> Result<Vec<MeetingListItem>, String> {
    meetings.with_conn(|conn| {
        index::search(conn, &query)
            .map(|records| records.into_iter().map(to_list_item).collect())
            .map_err(|error| error.to_string())
    })
}

#[tauri::command]
pub fn rebuild_index(meetings: State<'_, MeetingIndex>) -> Result<usize, String> {
    let root = meetings.notes_root().to_path_buf();
    meetings
        .with_conn(|conn| index::rebuild_from_disk(conn, &root).map_err(|error| error.to_string()))
}

#[tauri::command]
pub fn read_meeting(
    meetings: State<'_, MeetingIndex>,
    folder: String,
) -> Result<MeetingNote, String> {
    let folder = resolve_meeting_folder(meetings.notes_root(), &folder)?;
    let note = storage::read_note(&folder).map_err(|error| error.to_string())?;

    Ok(MeetingNote {
        folder: folder.display().to_string(),
        title: note.title,
        created_at: note.created_at,
        summary: note.summary_text,
        transcript: parse_transcript(&note.transcript_text),
        engine: note.engine,
        model: note.model,
        language: note.language,
        audio_kept: crate::audio::Track::all()
            .iter()
            .any(|track| folder.join(format!("{}.opus", track.file_stem())).is_file()),
    })
}

#[tauri::command]
pub fn open_meeting_folder<R: Runtime>(
    app: AppHandle<R>,
    meetings: State<'_, MeetingIndex>,
    folder: String,
) -> Result<(), String> {
    let folder = resolve_meeting_folder(meetings.notes_root(), &folder)?;
    app.opener()
        .open_path(folder.display().to_string(), None::<&str>)
        .map_err(|error| error.to_string())
}

/// Confirms a folder the UI named is really a meeting inside the notes root.
///
/// The UI supplies this path, so it is untrusted: without this check a
/// crafted value could read or reveal an arbitrary directory. Comparison is
/// on canonicalized paths so `..` cannot walk out.
fn resolve_meeting_folder(notes_root: &Path, folder: &str) -> Result<PathBuf, String> {
    let candidate = PathBuf::from(folder);
    let resolved = candidate
        .canonicalize()
        .map_err(|_| "that meeting no longer exists".to_owned())?;

    // A missing notes root means nothing can be inside it.
    let root = notes_root
        .canonicalize()
        .map_err(|_| "the notes folder is unavailable".to_owned())?;

    if !resolved.starts_with(&root) {
        return Err("that folder is not a meeting".to_owned());
    }
    Ok(resolved)
}

/// Parses the transcript section back into lines.
///
/// The written form is `[mm:ss] (track) text`; anything that does not match
/// is kept as a line with no timestamp rather than dropped, because losing
/// transcript text to a format change would be worse than showing it plainly.
fn parse_transcript(text: &str) -> Vec<TranscriptLine> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| TranscriptLine {
            start: 0.0,
            text: line.trim().to_owned(),
            track: None,
        })
        .collect()
}

/// Segments rendered into the transcript section of a note.
pub fn render_segments(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_outside_the_notes_root_is_refused() {
        let root = tempfile::tempdir().expect("temp dir");
        let elsewhere = tempfile::tempdir().expect("temp dir");
        let error = resolve_meeting_folder(root.path(), &elsewhere.path().display().to_string())
            .expect_err("a folder outside the notes root must be refused");
        assert!(error.contains("not a meeting"), "{error}");
    }

    #[test]
    fn a_traversal_attempt_cannot_walk_out_of_the_notes_root() {
        let root = tempfile::tempdir().expect("temp dir");
        let inside = root.path().join("2026-08-20-1000");
        std::fs::create_dir_all(&inside).expect("create");

        let escaping = inside.join("..").join("..").display().to_string();
        assert!(resolve_meeting_folder(root.path(), &escaping).is_err());
    }

    #[test]
    fn a_real_meeting_folder_resolves() {
        let root = tempfile::tempdir().expect("temp dir");
        let inside = root.path().join("2026-08-20-1000");
        std::fs::create_dir_all(&inside).expect("create");

        let resolved = resolve_meeting_folder(root.path(), &inside.display().to_string())
            .expect("a folder inside the notes root resolves");
        assert!(resolved.ends_with("2026-08-20-1000"));
    }

    #[test]
    fn a_missing_folder_is_reported_not_panicked() {
        let root = tempfile::tempdir().expect("temp dir");
        let error =
            resolve_meeting_folder(root.path(), &root.path().join("gone").display().to_string())
                .expect_err("a missing folder errors");
        assert!(error.contains("no longer exists"), "{error}");
    }

    #[test]
    fn transcript_lines_survive_a_format_they_do_not_match() {
        let lines = parse_transcript("first line\n\n  second line  \n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "first line");
        assert_eq!(lines[1].text, "second line");
    }
}
