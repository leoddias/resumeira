//! The meeting note as a file the user owns (ADR-0007).
//!
//! `notes.md` is the source of truth: a summary, a divider, then the full
//! transcript. Everything the index knows is derived from this file, so
//! [`read_note`] must be able to parse back what [`write_note`] wrote —
//! that round trip is what makes `index::rebuild_from_disk` a real recovery
//! path instead of an aspiration.
//!
//! The write is atomic (temp file, then rename) so a crash mid-write can
//! never destroy a note that was already on disk.
//!
//! Never logs note content or transcript text (docs/CONVENTIONS.md §
//! Privacy) — errors here carry paths and error kinds only.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::summarize::Summary;
use crate::transcribe::{Engine, Transcript};

/// Filename every meeting folder holds its note under.
const NOTE_FILENAME: &str = "notes.md";

/// The exact text that separates the summary section from the transcript.
/// Both [`render_note`] and [`parse_note`] must agree on this.
const DIVIDER: &str = "\n---\n\n## Transcript\n\n";

/// Machine-readable header line. Carries the fields that must survive a
/// round trip byte-for-byte (the title in particular can hold newlines,
/// since it is model-generated) — everything after it in the file is for
/// humans and is reconstructed as best-effort searchable text.
#[derive(Debug, Serialize, Deserialize)]
struct NoteMeta {
    title: String,
    engine: String,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    created_at: String,
}

/// What [`read_note`] recovers from a `notes.md` file.
///
/// This is not a byte-for-byte [`Summary`]/[`Transcript`] reconstruction —
/// bullets and decisions are flattened into `summary_text` — because all the
/// index needs is title, provenance, and searchable text. The title,
/// engine, model, language and creation time are exact.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedNote {
    pub title: String,
    pub engine: String,
    pub model: String,
    pub language: Option<String>,
    pub created_at: String,
    pub summary_text: String,
    pub transcript_text: String,
}

/// Anything that can go wrong reading or writing a note.
///
/// Carries paths and error kinds only — never note content or transcript
/// text (docs/CONVENTIONS.md § Privacy).
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("note I/O at '{path}' failed: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("note at '{path}' is not in the expected format")]
    Malformed { path: String },
}

/// Writes `folder/notes.md`: summary first, a divider, then the full
/// transcript (ADR-0007). Returns the note's path.
///
/// The write is atomic: content lands in a temp file in the same folder,
/// then that file is renamed onto `notes.md`. A crash or a failed write
/// leaves a partial temp file, never a half-written `notes.md` — and if
/// `notes.md` already existed, that copy is untouched until the rename
/// succeeds.
pub fn write_note(
    folder: &Path,
    summary: &Summary,
    transcript: &Transcript,
) -> Result<PathBuf, StorageError> {
    write_note_at(folder, summary, transcript, Local::now())
}

/// Same as [`write_note`] with an injected clock, so `created_at` is
/// deterministic in tests.
fn write_note_at(
    folder: &Path,
    summary: &Summary,
    transcript: &Transcript,
    now: DateTime<Local>,
) -> Result<PathBuf, StorageError> {
    std::fs::create_dir_all(folder).map_err(|source| StorageError::Io {
        path: folder.display().to_string(),
        source,
    })?;

    let content = render_note(summary, transcript, now);

    let final_path = folder.join(NOTE_FILENAME);
    let tmp_path = folder.join(format!("{NOTE_FILENAME}.tmp"));

    std::fs::write(&tmp_path, content.as_bytes()).map_err(|source| StorageError::Io {
        path: tmp_path.display().to_string(),
        source,
    })?;
    std::fs::rename(&tmp_path, &final_path).map_err(|source| StorageError::Io {
        path: final_path.display().to_string(),
        source,
    })?;

    Ok(final_path)
}

/// Reads `folder/notes.md` and parses it back into its parts. This is the
/// operation `index::rebuild_from_disk` relies on: everything the index
/// holds can be regenerated from this call alone.
pub fn read_note(folder: &Path) -> Result<ParsedNote, StorageError> {
    let path = folder.join(NOTE_FILENAME);
    let content = std::fs::read_to_string(&path).map_err(|source| StorageError::Io {
        path: path.display().to_string(),
        source,
    })?;
    parse_note(&content).ok_or_else(|| StorageError::Malformed {
        path: path.display().to_string(),
    })
}

fn engine_label(engine: Engine) -> &'static str {
    match engine {
        Engine::Local => "local",
        Engine::Api => "api",
    }
}

fn render_note(summary: &Summary, transcript: &Transcript, now: DateTime<Local>) -> String {
    let meta = NoteMeta {
        title: summary.title.clone(),
        engine: engine_label(transcript.engine).to_owned(),
        model: summary.model.clone(),
        language: transcript.language.clone(),
        created_at: now.to_rfc3339(),
    };
    // `NoteMeta` is a fixed, controlled shape (two strings, two more
    // strings), so serialization cannot fail.
    let meta_json = serde_json::to_string(&meta).expect("NoteMeta always serializes");

    // The heading is for humans reading the file directly; a title with
    // embedded newlines is flattened to one line here, but the exact
    // original stays recoverable from the meta line above.
    let display_title = summary.title.replace(['\r', '\n'], " ");

    let mut out = String::new();
    out.push_str("<!-- resumeira:meta ");
    out.push_str(&meta_json);
    out.push_str(" -->\n\n");
    out.push_str("# ");
    out.push_str(&display_title);
    out.push_str("\n\n");
    out.push_str(&format!(
        "_Generated by {} · {}_\n\n",
        meta.engine, meta.model
    ));

    out.push_str("## Summary\n\n");
    if summary.bullets.is_empty() {
        out.push_str("_No summary points recorded._\n");
    } else {
        for bullet in &summary.bullets {
            out.push_str("- ");
            out.push_str(bullet);
            out.push('\n');
        }
    }
    out.push('\n');

    out.push_str("## Decisions\n\n");
    if summary.decisions.is_empty() {
        out.push_str("_No decisions recorded._\n");
    } else {
        for decision in &summary.decisions {
            out.push_str("- ");
            out.push_str(decision);
            out.push('\n');
        }
    }
    out.push('\n');

    out.push_str("## Action Items\n\n");
    if summary.action_items.is_empty() {
        out.push_str("_No action items recorded._\n");
    } else {
        for item in &summary.action_items {
            out.push_str("- [ ] ");
            out.push_str(&item.task);
            if let Some(owner) = &item.owner {
                out.push_str(&format!(" (owner: {owner})"));
            }
            if let Some(due) = &item.due {
                out.push_str(&format!(" (due: {due})"));
            }
            out.push('\n');
        }
    }

    out.push_str(DIVIDER);
    out.push_str(&transcript.to_plain_text());
    out.push('\n');
    out
}

fn parse_note(content: &str) -> Option<ParsedNote> {
    let (first_line, rest) = content.split_once('\n')?;
    let meta_json = first_line
        .strip_prefix("<!-- resumeira:meta ")?
        .strip_suffix(" -->")?;
    let meta: NoteMeta = serde_json::from_str(meta_json).ok()?;

    let divider_at = rest.find(DIVIDER)?;
    let summary_block = &rest[..divider_at];
    let transcript_block = &rest[divider_at + DIVIDER.len()..];

    Some(ParsedNote {
        title: meta.title,
        engine: meta.engine,
        model: meta.model,
        language: meta.language,
        created_at: meta.created_at,
        summary_text: summary_block.trim().to_owned(),
        transcript_text: transcript_block.trim_end_matches('\n').to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarize::ActionItem;
    use crate::transcribe::Segment;
    use chrono::TimeZone;
    use tempfile::tempdir;

    fn fixed_now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 8, 19, 10, 30, 0).unwrap()
    }

    fn segment(start: f64, end: f64, text: &str) -> Segment {
        Segment {
            start,
            end,
            text: text.to_owned(),
            track: None,
        }
    }

    fn summary(title: &str) -> Summary {
        Summary {
            title: title.to_owned(),
            bullets: vec!["Discussed the roadmap".to_owned()],
            decisions: vec!["Ship on Friday".to_owned()],
            action_items: vec![ActionItem {
                task: "Write the release notes".to_owned(),
                owner: Some("Leo".to_owned()),
                due: Some("Thursday".to_owned()),
            }],
            model: "claude-sonnet-5".to_owned(),
        }
    }

    fn transcript() -> Transcript {
        Transcript {
            segments: vec![
                segment(0.0, 1.0, "Let's start."),
                segment(1.0, 2.0, "Sure, go ahead."),
            ],
            language: Some("en".to_owned()),
            engine: Engine::Local,
        }
    }

    #[test]
    fn round_trips_a_plain_note() {
        let dir = tempdir().unwrap();
        let s = summary("Weekly sync");
        let t = transcript();

        let path = write_note_at(dir.path(), &s, &t, fixed_now()).unwrap();
        assert_eq!(path, dir.path().join("notes.md"));

        let parsed = read_note(dir.path()).unwrap();
        assert_eq!(parsed.title, "Weekly sync");
        assert_eq!(parsed.engine, "local");
        assert_eq!(parsed.model, "claude-sonnet-5");
        assert_eq!(parsed.language.as_deref(), Some("en"));
        assert_eq!(parsed.transcript_text, t.to_plain_text());
        assert!(parsed.summary_text.contains("Discussed the roadmap"));
        assert!(parsed.summary_text.contains("Ship on Friday"));
        assert!(parsed.summary_text.contains("Write the release notes"));
    }

    #[test]
    fn round_trips_a_title_with_unicode_emoji_and_newlines() {
        let dir = tempdir().unwrap();
        let title = "Sync café ☕️🚀\nQ3 планирование\n— follow-up";
        let s = summary(title);
        let t = transcript();

        write_note_at(dir.path(), &s, &t, fixed_now()).unwrap();
        let parsed = read_note(dir.path()).unwrap();

        assert_eq!(parsed.title, title);
    }

    #[test]
    fn states_which_engine_and_model_produced_the_note() {
        let dir = tempdir().unwrap();
        let s = summary("API meeting");
        let t = Transcript {
            engine: Engine::Api,
            ..transcript()
        };

        write_note_at(dir.path(), &s, &t, fixed_now()).unwrap();
        let parsed = read_note(dir.path()).unwrap();

        assert_eq!(parsed.engine, "api");
        assert_eq!(parsed.model, "claude-sonnet-5");

        let raw = std::fs::read_to_string(dir.path().join("notes.md")).unwrap();
        assert!(raw.contains("Generated by api"));
    }

    #[test]
    fn a_failed_write_leaves_the_old_note_intact() {
        let dir = tempdir().unwrap();
        let original = summary("Original note");
        let t = transcript();
        write_note_at(dir.path(), &original, &t, fixed_now()).unwrap();
        let before = std::fs::read_to_string(dir.path().join("notes.md")).unwrap();

        // Force the temp-file write to fail by occupying its path with a
        // directory, simulating a crash or a permissions problem mid-write.
        let tmp_path = dir.path().join("notes.md.tmp");
        std::fs::create_dir(&tmp_path).unwrap();

        let replacement = summary("This must never land");
        let result = write_note_at(dir.path(), &replacement, &t, fixed_now());
        assert!(result.is_err());

        let after = std::fs::read_to_string(dir.path().join("notes.md")).unwrap();
        assert_eq!(
            before, after,
            "a failed write must not touch the existing note"
        );
        assert!(!after.contains("This must never land"));
    }

    #[test]
    fn reading_a_missing_note_is_an_io_error_not_a_panic() {
        let dir = tempdir().unwrap();
        let result = read_note(dir.path());
        assert!(matches!(result, Err(StorageError::Io { .. })));
    }

    #[test]
    fn reading_a_malformed_note_is_reported_not_panicked() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("notes.md"), "not a real note").unwrap();
        let result = read_note(dir.path());
        assert!(matches!(result, Err(StorageError::Malformed { .. })));
    }
}
