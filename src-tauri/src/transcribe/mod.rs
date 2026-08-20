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
    /// The transcript as plain text, one line per segment.
    pub fn to_plain_text(&self) -> String {
        self.segments
            .iter()
            .map(|segment| segment.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
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
        }
    }

    fn transcript(segments: Vec<Segment>) -> Transcript {
        Transcript {
            segments,
            language: Some("en".to_owned()),
            engine: Engine::Local,
        }
    }

    #[test]
    fn plain_text_joins_segments_and_drops_blanks() {
        let t = transcript(vec![
            segment(0.0, 1.0, "hello"),
            segment(1.0, 2.0, "   "),
            segment(2.0, 3.0, " there "),
        ]);
        assert_eq!(t.to_plain_text(), "hello\nthere");
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
