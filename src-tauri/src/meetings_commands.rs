//! Tauri commands for browsing meetings.
//!
//! The index answers lists and searches; the note file answers a read. That
//! split is deliberate: the database is a cache that can be rebuilt, and the
//! file is the thing the user owns (ADR-0007).

use crate::audio::Track;
use crate::index;
use crate::storage;
use crate::transcribe;
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
    /// Who said it, when the note recorded a name (ADR-0021).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
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
/// The written form is `[mm:ss] Speaker: text`, as
/// [`Transcript::to_note_text`] writes it. Every part is optional on the way
/// back in: a line that does not match is kept whole, with no timestamp and
/// no speaker, because losing transcript text to a format change would be
/// worse than showing it plainly. That is also what keeps notes written
/// before ADR-0021 - bare text lines - readable.
fn parse_transcript(text: &str) -> Vec<TranscriptLine> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| parse_transcript_line(line.trim()))
        .collect()
}

/// One written line, read back as far as it parses.
fn parse_transcript_line(line: &str) -> TranscriptLine {
    let Some((start, rest)) = split_timestamp(line) else {
        return TranscriptLine {
            start: 0.0,
            text: line.to_owned(),
            track: None,
            speaker: None,
        };
    };

    match split_speaker(rest) {
        Some((label, text)) => {
            // The two fallback labels are how an unidentified line is
            // written, so they come back as the track they stand for rather
            // than as somebody called "You".
            let (track, speaker) = match label {
                transcribe::MIC_LABEL => (Some(Track::Mic), None),
                transcribe::SYSTEM_LABEL => (Some(Track::System), None),
                transcribe::UNKNOWN_LABEL => (None, None),
                name => (None, Some(name.to_owned())),
            };
            TranscriptLine {
                start,
                text: text.to_owned(),
                track,
                speaker,
            }
        }
        None => TranscriptLine {
            start,
            text: rest.to_owned(),
            track: None,
            speaker: None,
        },
    }
}

/// Splits a leading `[mm:ss]` or `[h:mm:ss]` off, returning the seconds it
/// stands for and the rest of the line.
fn split_timestamp(line: &str) -> Option<(f64, &str)> {
    let rest = line.strip_prefix('[')?;
    let (stamp, rest) = rest.split_once(']')?;

    let mut seconds = 0u64;
    let mut parts = 0;
    for part in stamp.split(':') {
        seconds = seconds.checked_mul(60)?.checked_add(part.parse().ok()?)?;
        parts += 1;
    }
    if !(2..=3).contains(&parts) {
        return None;
    }

    Some((seconds as f64, rest.trim_start()))
}

/// Splits the `Speaker: ` label off the text.
///
/// Every timestamped line this app writes carries a label, so the first
/// `": "` is the separator and a colon later in the sentence is just speech.
/// The length cap is a guard against a file this app did not write.
fn split_speaker(rest: &str) -> Option<(&str, &str)> {
    let (label, text) = rest.split_once(": ")?;
    let label = label.trim();
    if label.is_empty() || label.chars().count() > MAX_SPEAKER_LEN {
        return None;
    }
    Some((label, text.trim()))
}

/// Mirrors `diarize`'s own cap, so a label this app wrote is always a label
/// this app reads back.
const MAX_SPEAKER_LEN: usize = 60;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcribe::{Segment, Transcript};

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
        assert_eq!(
            lines[0].start, 0.0,
            "a note written before ADR-0021 still reads back, just without a clock"
        );
    }

    #[test]
    fn a_written_line_reads_back_with_its_timestamp_and_speaker() {
        let lines = parse_transcript("[01:14] Ana: morning Leo");
        assert_eq!(lines[0].start, 74.0);
        assert_eq!(lines[0].speaker.as_deref(), Some("Ana"));
        assert_eq!(lines[0].text, "morning Leo");
        assert_eq!(lines[0].track, None);
    }

    #[test]
    fn an_hour_long_meeting_keeps_its_timestamps() {
        let lines = parse_transcript("[1:00:05] Ana: still here");
        assert_eq!(lines[0].start, 3605.0);
    }

    #[test]
    fn the_track_fallbacks_read_back_as_tracks_not_as_people() {
        let lines = parse_transcript("[00:00] You: mine\n[00:02] Others: theirs");
        assert_eq!(lines[0].track, Some(Track::Mic));
        assert_eq!(lines[0].speaker, None);
        assert_eq!(lines[1].track, Some(Track::System));
        assert_eq!(lines[1].text, "theirs");
    }

    #[test]
    fn a_colon_inside_speech_stays_inside_the_speech() {
        let lines = parse_transcript("[00:03] Ana: so here is the thing: we ship on Friday");
        assert_eq!(lines[0].speaker.as_deref(), Some("Ana"));
        assert_eq!(lines[0].text, "so here is the thing: we ship on Friday");
    }

    #[test]
    fn a_named_line_keeps_the_name_and_gives_up_the_track() {
        let lines = parse_transcript("[00:00] Leo: morning");
        assert_eq!(lines[0].speaker.as_deref(), Some("Leo"));
        assert_eq!(
            lines[0].track, None,
            "the note writes the name instead of the track, by decision (ADR-0021)"
        );
    }

    #[test]
    fn a_participant_actually_called_you_reads_back_as_the_track() {
        let lines = parse_transcript("[00:00] You: morning");
        assert_eq!(
            (lines[0].track, lines[0].speaker.clone()),
            (Some(Track::Mic), None),
            "the fallback labels are reserved words on disk; a person named You loses to them"
        );
    }

    #[test]
    fn an_unattributable_line_is_written_and_read_as_nobody() {
        let lines = parse_transcript("[00:03] Unknown: somebody said this");
        assert_eq!(lines[0].speaker, None);
        assert_eq!(lines[0].track, None);
        assert_eq!(lines[0].text, "somebody said this");
    }

    #[test]
    fn a_round_trip_through_the_note_form_preserves_who_spoke() {
        let transcript = Transcript {
            segments: vec![
                Segment {
                    start: 0.0,
                    end: 1.0,
                    text: "morning".to_owned(),
                    track: Some(Track::Mic),
                    speaker: Some("Leo".to_owned()),
                },
                Segment {
                    start: 74.0,
                    end: 75.0,
                    text: "morning Leo".to_owned(),
                    track: Some(Track::System),
                    speaker: None,
                },
            ],
            language: None,
            engine: transcribe::Engine::Local,
        };

        let lines = parse_transcript(&transcript.to_note_text());

        assert_eq!(lines[0].speaker.as_deref(), Some("Leo"));
        assert_eq!(lines[0].start, 0.0);
        assert_eq!(lines[1].track, Some(Track::System));
        assert_eq!(lines[1].start, 74.0);
    }
}
