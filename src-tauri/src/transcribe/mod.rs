//! Transcription contracts.
//!
//! Two engines produce the same [`Transcript`]: a local Whisper model and a
//! cloud API. Which one runs is decided by [`Engine`], which comes from
//! settings and is never inferred at the last moment — an implicit fallback
//! that uploaded a meeting because the local model failed would be the worst
//! bug this product could ship (ADR-0005).

pub mod api;
pub mod local;
pub mod model;
pub mod routing;

use crate::audio::Track;
use serde::{Deserialize, Serialize};

/// Which engine transcribes a meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Engine {
    /// Runs on this machine. Nothing leaves it.
    Local,
    /// Sends audio to the configured provider, using the user's key.
    Api,
}

/// Cloud transcription providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApiProvider {
    Groq,
    OpenAi,
}

impl ApiProvider {
    /// Keychain entry name for this provider's key.
    pub fn key_name(self) -> &'static str {
        match self {
            ApiProvider::Groq => "groq",
            ApiProvider::OpenAi => "openai",
        }
    }
}

/// One continuous piece of speech, as the transcriber heard it.
///
/// `Debug` is implemented by hand and prints no text: this is what somebody
/// said in a meeting, and a stray `{:?}` in a log line would put it on disk
/// (docs/CONVENTIONS.md § Privacy).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    /// Seconds from the start of the meeting.
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// Which track carried this speech, when the engine can tell them apart.
    ///
    /// The two tracks are transcribed separately when possible, which is what
    /// makes "you" and "them" distinguishable without real diarization
    /// (ADR-0004).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<Track>,
    /// Who said it, once the speaker step has run (ADR-0021).
    ///
    /// Either a name the conversation itself supplied ("Ana") or a stable
    /// anonymous label ("Speaker 2"). `None` means nobody has tried, or the
    /// attempt could not place this line — never a guess.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

impl Segment {
    /// How this line is attributed, best first: the identified speaker, then
    /// the track it arrived on, then nothing.
    ///
    /// The track fallback is what ADR-0004 bought and is worth showing on its
    /// own: "You" and "Others" is less than a name, but it is more than
    /// silence about who spoke.
    pub fn speaker_label(&self) -> Option<&str> {
        match (self.speaker.as_deref(), self.track) {
            (Some(speaker), _) => Some(speaker),
            (None, Some(Track::Mic)) => Some(MIC_LABEL),
            (None, Some(Track::System)) => Some(SYSTEM_LABEL),
            (None, None) => None,
        }
    }
}

/// Written when a mic line has no identified speaker. Parsed back to
/// [`Track::Mic`] when a note is read, so the round trip loses nothing.
pub const MIC_LABEL: &str = "You";
/// The system-track counterpart of [`MIC_LABEL`].
pub const SYSTEM_LABEL: &str = "Others";
/// Written for a line with no speaker *and* no track.
///
/// A note always carries a label so that the first `": "` after the
/// timestamp is unambiguously the separator - speech is full of colons, and
/// without this a sentence could be read back as somebody's name.
pub const UNKNOWN_LABEL: &str = "Unknown";

/// `mm:ss` for a position in the meeting, widening past an hour.
///
/// Written into `notes.md`, so the format is part of the file contract that
/// `meetings_commands::parse_transcript` reads back.
pub(crate) fn format_timestamp(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (hours, minutes, secs) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes:02}:{secs:02}")
    }
}

/// A whole meeting's speech.
///
/// `Debug` is redacted — see [`Segment`].
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transcript {
    pub segments: Vec<Segment>,
    /// BCP-47-ish language tag the engine detected, when it reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Which engine produced this, so a note can say so honestly.
    pub engine: Engine,
}

impl Transcript {
    /// The transcript as plain text, one line per segment, each prefixed with
    /// whoever said it when that is known.
    ///
    /// This is what a model is asked to summarize: an action item can only
    /// name an owner if the transcript said who was speaking.
    pub fn to_prompt_text(&self) -> String {
        self.non_empty_segments()
            .map(|segment| match segment.speaker_label() {
                Some(label) => format!("{label}: {}", segment.text.trim()),
                None => segment.text.trim().to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The transcript as it is written into `notes.md`: `[mm:ss] Ana: text`,
    /// falling back to the track's label, then to [`UNKNOWN_LABEL`].
    ///
    /// The timestamp and the label are not decoration - they are the only
    /// copy of that information once the note is on disk, and
    /// `meetings_commands::parse_transcript` reads them back.
    pub fn to_note_text(&self) -> String {
        self.non_empty_segments()
            .map(|segment| {
                let stamp = format_timestamp(segment.start);
                let label = segment.speaker_label().unwrap_or(UNKNOWN_LABEL);
                format!("[{stamp}] {label}: {}", segment.text.trim())
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Everyone the speaker step actually identified, in the order they first
    /// spoke.
    ///
    /// Derived here rather than asked of a model, so it cannot name someone
    /// who never appears in the transcript. Empty when the step did not run
    /// or placed nothing: the track fallbacks ("You", "Others") are a way of
    /// rendering a line, not people, so they are deliberately excluded.
    pub fn participants(&self) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        for segment in self.non_empty_segments() {
            if let Some(speaker) = segment.speaker.as_deref() {
                if !seen.iter().any(|known| known == speaker) {
                    seen.push(speaker.to_owned());
                }
            }
        }
        seen
    }

    /// Segments that carry actual speech, in order.
    fn non_empty_segments(&self) -> impl Iterator<Item = &Segment> {
        self.segments
            .iter()
            .filter(|segment| !segment.text.trim().is_empty())
    }

    /// Total speech duration in seconds: the end of the last segment.
    pub fn duration_secs(&self) -> f64 {
        self.segments
            .iter()
            .map(|segment| segment.end)
            .fold(0.0, f64::max)
    }

    /// Whether there is anything worth summarizing.
    ///
    /// A meeting where nobody spoke must not be sent to an LLM: it costs the
    /// user money and produces invented content.
    pub fn is_empty(&self) -> bool {
        self.segments
            .iter()
            .all(|segment| segment.text.trim().is_empty())
    }

    /// Merges per-track transcripts into one timeline, ordered by start time.
    pub fn merge(parts: Vec<Transcript>) -> Option<Transcript> {
        let engine = parts.first()?.engine;
        let language = parts.iter().find_map(|part| part.language.clone());

        let mut segments: Vec<Segment> = parts
            .into_iter()
            .flat_map(|part| part.segments)
            .filter(|segment| !segment.text.trim().is_empty())
            .collect();
        segments.sort_by(|a, b| a.start.total_cmp(&b.start));

        Some(Transcript {
            segments,
            language,
            engine,
        })
    }
}

impl std::fmt::Debug for Segment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Segment")
            .field("start", &self.start)
            .field("end", &self.end)
            .field("track", &self.track)
            // The name itself is somebody's, so only its presence is printed.
            .field("speaker_known", &self.speaker.is_some())
            .field("text_chars", &self.text.chars().count())
            .finish()
    }
}

impl std::fmt::Debug for Transcript {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transcript")
            .field("engine", &self.engine)
            .field("language", &self.language)
            .field("segments", &self.segments.len())
            .finish()
    }
}

/// Anything that can go wrong while transcribing.
///
/// Carries error kinds and provider names only — never transcript text, audio
/// samples, or key material (docs/CONVENTIONS.md § Privacy).
#[derive(Debug, thiserror::Error)]
pub enum TranscribeError {
    #[error("no API key configured for {provider}")]
    MissingKey { provider: &'static str },

    #[error("{provider} rejected the key")]
    Unauthorized { provider: &'static str },

    #[error("{provider} is rate limiting; retry after {retry_after_secs}s")]
    RateLimited {
        provider: &'static str,
        retry_after_secs: u64,
    },

    #[error("{provider} returned an unexpected response: {reason}")]
    BadResponse {
        provider: &'static str,
        reason: String,
    },

    #[error("network request to {provider} failed: {reason}")]
    Network {
        provider: &'static str,
        reason: String,
    },

    #[error("the local model '{model}' is not installed")]
    ModelMissing { model: String },

    #[error("the local model failed: {0}")]
    LocalEngine(String),

    #[error("reading '{path}' failed: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("there is no audio to transcribe")]
    NoAudio,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(start: f64, end: f64, text: &str) -> Segment {
        Segment {
            start,
            end,
            text: text.to_owned(),
            track: None,
            speaker: None,
        }
    }

    fn transcript(segments: Vec<Segment>) -> Transcript {
        Transcript {
            segments,
            language: Some("en".to_owned()),
            engine: Engine::Local,
        }
    }

    fn attributed(start: f64, text: &str, track: Track, speaker: Option<&str>) -> Segment {
        Segment {
            track: Some(track),
            speaker: speaker.map(str::to_owned),
            ..segment(start, start + 1.0, text)
        }
    }

    #[test]
    fn a_line_is_labelled_by_its_speaker_first_and_its_track_second() {
        let named = attributed(0.0, "hi", Track::System, Some("Ana"));
        assert_eq!(named.speaker_label(), Some("Ana"));

        let unnamed = attributed(0.0, "hi", Track::System, None);
        assert_eq!(
            unnamed.speaker_label(),
            Some(SYSTEM_LABEL),
            "the track is what ADR-0004 bought; it stands in when nobody was named"
        );

        assert_eq!(segment(0.0, 1.0, "hi").speaker_label(), None);
    }

    #[test]
    fn prompt_text_carries_the_speaker_so_an_owner_can_be_named() {
        let t = transcript(vec![
            attributed(0.0, "I will send it", Track::Mic, Some("Leo")),
            attributed(1.0, "thanks", Track::System, None),
        ]);
        assert_eq!(t.to_prompt_text(), "Leo: I will send it\nOthers: thanks");
    }

    #[test]
    fn note_text_writes_the_timestamp_and_the_label() {
        let t = transcript(vec![
            attributed(0.0, "morning", Track::Mic, None),
            attributed(74.0, "hi", Track::System, Some("Ana")),
            segment(3605.0, 3606.0, "unattributed"),
        ]);
        assert_eq!(
            t.to_note_text(),
            "[00:00] You: morning\n[01:14] Ana: hi\n[1:00:05] Unknown: unattributed"
        );
    }

    #[test]
    fn participants_are_the_identified_speakers_in_first_speaking_order() {
        let t = transcript(vec![
            attributed(0.0, "hi", Track::System, Some("Ana")),
            attributed(1.0, "hello", Track::Mic, Some("Leo")),
            attributed(2.0, "again", Track::System, Some("Ana")),
        ]);
        assert_eq!(t.participants(), vec!["Ana".to_owned(), "Leo".to_owned()]);
    }

    #[test]
    fn an_unidentified_meeting_has_no_participants() {
        let t = transcript(vec![
            attributed(0.0, "hi", Track::Mic, None),
            attributed(1.0, "hello", Track::System, None),
        ]);
        assert!(
            t.participants().is_empty(),
            "'You' and 'Others' are ways of rendering a line, not people"
        );
    }

    #[test]
    fn merging_tracks_keeps_the_speakers_already_placed() {
        let mine = transcript(vec![attributed(0.0, "hi", Track::Mic, Some("Leo"))]);
        let theirs = transcript(vec![attributed(1.0, "hello", Track::System, Some("Ana"))]);
        let merged = Transcript::merge(vec![mine, theirs]).expect("merged");
        assert_eq!(
            merged.participants(),
            vec!["Leo".to_owned(), "Ana".to_owned()]
        );
    }

    #[test]
    fn a_speakers_name_never_reaches_a_debug_line() {
        let printed = format!("{:?}", attributed(0.0, "hi", Track::Mic, Some("Ana")));
        assert!(!printed.contains("Ana"), "{printed}");
        assert!(!printed.contains("hi"), "{printed}");
        assert!(printed.contains("speaker_known: true"), "{printed}");
    }

    #[test]
    fn prompt_text_joins_segments_and_drops_blanks() {
        let t = transcript(vec![
            segment(0.0, 1.0, "hello"),
            segment(1.0, 2.0, "   "),
            segment(2.0, 3.0, " there "),
        ]);
        assert_eq!(t.to_prompt_text(), "hello\nthere");
    }

    #[test]
    fn duration_is_the_end_of_the_last_segment_even_if_unordered() {
        let t = transcript(vec![segment(10.0, 12.0, "b"), segment(0.0, 1.0, "a")]);
        assert_eq!(t.duration_secs(), 12.0);
    }

    #[test]
    fn a_silent_meeting_is_empty_so_it_is_never_summarized() {
        assert!(transcript(vec![]).is_empty());
        assert!(transcript(vec![segment(0.0, 1.0, "  ")]).is_empty());
        assert!(!transcript(vec![segment(0.0, 1.0, "a word")]).is_empty());
    }

    #[test]
    fn merging_interleaves_tracks_by_time() {
        let mic = Transcript {
            segments: vec![
                Segment {
                    track: Some(Track::Mic),
                    ..segment(0.0, 1.0, "mine first")
                },
                Segment {
                    track: Some(Track::Mic),
                    ..segment(4.0, 5.0, "mine last")
                },
            ],
            language: Some("en".to_owned()),
            engine: Engine::Local,
        };
        let system = Transcript {
            segments: vec![Segment {
                track: Some(Track::System),
                ..segment(2.0, 3.0, "theirs")
            }],
            language: None,
            engine: Engine::Local,
        };

        let merged = Transcript::merge(vec![mic, system]).expect("two parts merge");
        assert_eq!(
            merged
                .segments
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            vec!["mine first", "theirs", "mine last"]
        );
        assert_eq!(merged.segments[1].track, Some(Track::System));
        assert_eq!(merged.language.as_deref(), Some("en"));
    }

    #[test]
    fn merging_nothing_yields_nothing() {
        assert!(Transcript::merge(vec![]).is_none());
    }

    #[test]
    fn engine_survives_a_round_trip_through_settings_json() {
        let json = serde_json::to_string(&Engine::Local).expect("serialize");
        assert_eq!(json, "\"local\"");
        let back: Engine = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, Engine::Local);
    }

    #[test]
    fn provider_key_names_are_stable_because_the_keychain_uses_them() {
        // Changing these strands every existing user's stored key.
        assert_eq!(ApiProvider::Groq.key_name(), "groq");
        assert_eq!(ApiProvider::OpenAi.key_name(), "openai");
    }

    #[test]
    fn debug_output_never_contains_what_was_said() {
        // A stray `{:?}` in a log line must not put a meeting on disk. This
        // is a guard against someone restoring `#[derive(Debug)]` later.
        let spoken = "the merger closes on Friday";
        let t = transcript(vec![segment(0.0, 1.0, spoken)]);

        let rendered = format!("{t:?}");
        assert!(
            !rendered.contains(spoken),
            "Transcript debug leaked speech: {rendered}"
        );
        assert!(rendered.contains("segments: 1"), "{rendered}");

        let rendered = format!("{:?}", t.segments[0]);
        assert!(
            !rendered.contains(spoken),
            "Segment debug leaked speech: {rendered}"
        );
    }

    #[test]
    fn errors_never_carry_transcript_or_key_material() {
        let error = TranscribeError::Unauthorized { provider: "groq" };
        let rendered = error.to_string();
        assert_eq!(rendered, "groq rejected the key");
        assert!(!rendered.contains("sk-"));
    }
}
