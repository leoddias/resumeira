//! What happens after the recording stops: audio in, note out.
//!
//! This is the seam where every other module meets, so it is deliberately
//! thin and orchestration-only. The two expensive halves — transcription and
//! summarization — are traits, which is what makes the whole sequence
//! testable without a Whisper model, a network, or an API key.
//!
//! Tracks are transcribed **separately and then merged** rather than mixed
//! into one stream first. It costs a second pass, and it buys the one thing
//! mixing destroys forever: knowing whether a line came from the user's
//! microphone or from everyone else (ADR-0004).

use crate::audio::Track;
use crate::config::AudioRetention;
use crate::storage;
use crate::summarize::{SummarizeError, Summary};
use crate::transcribe::{TranscribeError, Transcript};
use std::path::{Path, PathBuf};

/// Where the pipeline is, so the UI can say something true while it waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Transcribing,
    Summarizing,
    Saving,
}

/// Anything that can stop a meeting from becoming a note.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("no audio was recorded for this meeting")]
    NoAudio,

    #[error(transparent)]
    Transcribe(#[from] TranscribeError),

    #[error(transparent)]
    Summarize(#[from] SummarizeError),

    #[error("writing the note failed: {0}")]
    Storage(String),
}

/// Turns one track's audio file into a transcript.
pub trait Transcriber {
    fn transcribe(
        &self,
        track: Track,
        audio: &Path,
    ) -> impl std::future::Future<Output = Result<Transcript, TranscribeError>> + Send;
}

/// Turns a transcript into a summary.
pub trait Summarizer {
    fn summarize(
        &self,
        transcript: &Transcript,
    ) -> impl std::future::Future<Output = Result<Summary, SummarizeError>> + Send;
}

/// What the pipeline produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub note: PathBuf,
    pub title: String,
    pub transcript: Transcript,
}

/// Runs a recorded meeting folder through to a written note.
///
/// The order is not arbitrary: the note is written before the audio is
/// considered disposable, so a retention setting can never delete the only
/// copy of a meeting whose note failed to save.
pub async fn process<T, S, P>(
    folder: &Path,
    transcriber: &T,
    summarizer: &S,
    retention: AudioRetention,
    mut on_stage: P,
) -> Result<Outcome, PipelineError>
where
    T: Transcriber,
    S: Summarizer,
    P: FnMut(Stage),
{
    on_stage(Stage::Transcribing);

    let mut parts = Vec::new();
    let mut failure: Option<TranscribeError> = None;
    for track in Track::all() {
        let audio = folder.join(format!("{}.opus", track.file_stem()));
        if !audio.is_file() {
            // One missing track is normal: a machine with no loopback device
            // still records the microphone.
            continue;
        }
        match transcriber.transcribe(track, &audio).await {
            Ok(transcript) => parts.push(transcript),
            Err(error) => {
                // A track that fails to transcribe must not cost the other
                // one — a note from half the meeting beats no note at all.
                // But the reason is kept: if *nothing* transcribes, the user
                // needs to hear "your key was rejected", not a guess about
                // their microphone.
                log::warn!("could not transcribe the {track:?} track: {error}");
                failure.get_or_insert(error);
            }
        }
    }

    let Some(transcript) = Transcript::merge(parts) else {
        // Report why transcription failed when it did. `NoAudio` is reserved
        // for the case where there was genuinely nothing to transcribe —
        // telling someone their meeting was silent when their API key was
        // rejected sends them to fix the wrong thing.
        return Err(match failure {
            Some(error) => PipelineError::Transcribe(error),
            None => PipelineError::NoAudio,
        });
    };
    if transcript.is_empty() {
        // Nobody spoke. Sending this to a model costs money and invents
        // content, so stop here and say so.
        return Err(PipelineError::NoAudio);
    }

    on_stage(Stage::Summarizing);
    let summary = summarizer.summarize(&transcript).await?;

    on_stage(Stage::Saving);
    let note = storage::write_note(folder, &summary, &transcript)
        .map_err(|error| PipelineError::Storage(error.to_string()))?;

    if retention == AudioRetention::DeleteAfterTranscription {
        delete_audio(folder);
    }

    Ok(Outcome {
        note,
        title: summary.title,
        transcript,
    })
}

/// Removes the recorded tracks, and only those.
///
/// Each path is rebuilt from a fixed file stem inside the meeting folder, so
/// nothing derived from a model-generated title can steer a delete.
fn delete_audio(folder: &Path) {
    for track in Track::all() {
        let audio = folder.join(format!("{}.opus", track.file_stem()));
        if audio.is_file() {
            if let Err(error) = std::fs::remove_file(&audio) {
                log::warn!("could not delete {:?} audio: {}", track, error.kind());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summarize::ActionItem;
    use crate::transcribe::{Engine, Segment};
    use std::sync::Mutex;

    fn write_track(folder: &Path, track: Track) {
        std::fs::write(folder.join(format!("{}.opus", track.file_stem())), b"audio")
            .expect("write track");
    }

    fn segment(start: f64, text: &str, track: Track) -> Segment {
        Segment {
            start,
            end: start + 1.0,
            text: text.to_owned(),
            track: Some(track),
        }
    }

    struct FakeTranscriber {
        /// Text to return per track; a track absent here fails.
        answers: Vec<(Track, &'static str)>,
        /// Error every track fails with, when set.
        rejection: Option<TranscribeError>,
        seen: Mutex<Vec<Track>>,
    }

    impl FakeTranscriber {
        fn new(answers: Vec<(Track, &'static str)>) -> Self {
            Self {
                answers,
                rejection: None,
                seen: Mutex::new(Vec::new()),
            }
        }

        /// Fails every track with the same error, as a bad key would.
        fn rejecting(error: TranscribeError) -> Self {
            Self {
                answers: Vec::new(),
                rejection: Some(error),
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl Transcriber for FakeTranscriber {
        async fn transcribe(
            &self,
            track: Track,
            _audio: &Path,
        ) -> Result<Transcript, TranscribeError> {
            self.seen.lock().expect("lock").push(track);
            if let Some(rejection) = &self.rejection {
                return Err(match rejection {
                    TranscribeError::Unauthorized { provider } => {
                        TranscribeError::Unauthorized { provider }
                    }
                    other => TranscribeError::LocalEngine(other.to_string()),
                });
            }
            let Some((_, text)) = self.answers.iter().find(|(t, _)| *t == track) else {
                return Err(TranscribeError::LocalEngine("fake failure".to_owned()));
            };
            Ok(Transcript {
                segments: vec![segment(
                    if track == Track::Mic { 0.0 } else { 1.0 },
                    text,
                    track,
                )],
                language: Some("en".to_owned()),
                engine: Engine::Local,
            })
        }
    }

    struct FakeSummarizer {
        fail: bool,
    }

    impl Summarizer for FakeSummarizer {
        async fn summarize(&self, transcript: &Transcript) -> Result<Summary, SummarizeError> {
            if self.fail {
                return Err(SummarizeError::EmptySummary);
            }
            Ok(Summary {
                title: "Weekly sync".to_owned(),
                bullets: vec![transcript.to_plain_text()],
                decisions: vec![],
                action_items: vec![ActionItem {
                    task: "Ship it".to_owned(),
                    owner: None,
                    due: None,
                }],
                model: "fake-model".to_owned(),
            })
        }
    }

    async fn run(
        folder: &Path,
        transcriber: &FakeTranscriber,
        summarizer: &FakeSummarizer,
        retention: AudioRetention,
    ) -> (Result<Outcome, PipelineError>, Vec<Stage>) {
        let mut stages = Vec::new();
        let result = process(folder, transcriber, summarizer, retention, |stage| {
            stages.push(stage)
        })
        .await;
        (result, stages)
    }

    #[tokio::test]
    async fn a_recorded_meeting_becomes_a_note() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);
        write_track(dir.path(), Track::System);

        let transcriber =
            FakeTranscriber::new(vec![(Track::Mic, "my line"), (Track::System, "their line")]);
        let (result, stages) = run(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            AudioRetention::Keep,
        )
        .await;

        let outcome = result.expect("the pipeline produces a note");
        assert_eq!(outcome.title, "Weekly sync");
        assert!(outcome.note.is_file());
        assert_eq!(
            stages,
            vec![Stage::Transcribing, Stage::Summarizing, Stage::Saving]
        );
    }

    #[tokio::test]
    async fn both_tracks_are_transcribed_and_merged_in_time_order() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);
        write_track(dir.path(), Track::System);

        let transcriber =
            FakeTranscriber::new(vec![(Track::Mic, "my line"), (Track::System, "their line")]);
        let (result, _) = run(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            AudioRetention::Keep,
        )
        .await;

        let outcome = result.expect("outcome");
        assert_eq!(outcome.transcript.to_plain_text(), "my line\ntheir line");
        assert_eq!(
            outcome.transcript.segments[0].track,
            Some(Track::Mic),
            "track attribution is the reason we transcribe separately"
        );
        assert_eq!(outcome.transcript.segments[1].track, Some(Track::System));
    }

    #[tokio::test]
    async fn a_machine_with_no_loopback_still_gets_a_note() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);

        let transcriber = FakeTranscriber::new(vec![(Track::Mic, "my line")]);
        let (result, _) = run(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            AudioRetention::Keep,
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(transcriber.seen.lock().expect("lock").len(), 1);
    }

    #[tokio::test]
    async fn one_track_failing_to_transcribe_does_not_lose_the_other() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);
        write_track(dir.path(), Track::System);

        // Only the mic answers; the system track errors.
        let transcriber = FakeTranscriber::new(vec![(Track::Mic, "my line")]);
        let (result, _) = run(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            AudioRetention::Keep,
        )
        .await;

        let outcome = result.expect("half a meeting still beats no note");
        assert_eq!(outcome.transcript.to_plain_text(), "my line");
    }

    #[tokio::test]
    async fn a_rejected_key_is_reported_as_a_rejected_key() {
        // The worst failure this pipeline can produce is not silence — it is
        // telling the user their microphone failed when their API key was
        // refused, sending them to fix the wrong thing.
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);
        write_track(dir.path(), Track::System);

        let transcriber =
            FakeTranscriber::rejecting(TranscribeError::Unauthorized { provider: "groq" });
        let (result, _) = run(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            AudioRetention::Keep,
        )
        .await;

        let error = result.expect_err("a rejected key must fail the pipeline");
        assert!(
            matches!(
                error,
                PipelineError::Transcribe(TranscribeError::Unauthorized { .. })
            ),
            "expected the real reason, got {error}"
        );
        assert!(
            error.to_string().contains("rejected the key"),
            "the user must see what to fix, got '{error}'"
        );
    }

    #[tokio::test]
    async fn one_track_failing_still_reports_nothing_when_the_other_succeeds() {
        // The error is only surfaced when it explains an empty result.
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);
        write_track(dir.path(), Track::System);

        let transcriber = FakeTranscriber::new(vec![(Track::Mic, "my line")]);
        let (result, _) = run(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            AudioRetention::Keep,
        )
        .await;

        assert!(result.is_ok(), "half a meeting still beats no note");
    }

    #[tokio::test]
    async fn a_meeting_with_no_audio_is_an_error_not_an_empty_note() {
        let dir = tempfile::tempdir().expect("temp dir");
        let transcriber = FakeTranscriber::new(vec![]);
        let (result, stages) = run(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            AudioRetention::Keep,
        )
        .await;

        assert!(matches!(result, Err(PipelineError::NoAudio)));
        assert_eq!(
            stages,
            vec![Stage::Transcribing],
            "a silent meeting must never reach the summarizer"
        );
    }

    #[tokio::test]
    async fn a_silent_meeting_is_never_sent_to_a_model() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);

        // The transcriber answers, but with nothing in it.
        let transcriber = FakeTranscriber::new(vec![(Track::Mic, "   ")]);
        let (result, stages) = run(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            AudioRetention::Keep,
        )
        .await;

        assert!(matches!(result, Err(PipelineError::NoAudio)));
        assert!(!stages.contains(&Stage::Summarizing));
    }

    #[tokio::test]
    async fn audio_survives_a_failed_summary() {
        // The retention setting must never delete the only copy of a meeting
        // whose note was never written.
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);

        let transcriber = FakeTranscriber::new(vec![(Track::Mic, "my line")]);
        let (result, _) = run(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: true },
            AudioRetention::DeleteAfterTranscription,
        )
        .await;

        assert!(result.is_err());
        assert!(
            dir.path().join("mic.opus").is_file(),
            "audio must survive when the note does not"
        );
    }

    #[tokio::test]
    async fn retention_deletes_the_audio_only_after_the_note_exists() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);
        write_track(dir.path(), Track::System);
        std::fs::write(dir.path().join("keep-me.txt"), b"unrelated").expect("write");

        let transcriber =
            FakeTranscriber::new(vec![(Track::Mic, "my line"), (Track::System, "their line")]);
        let (result, _) = run(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            AudioRetention::DeleteAfterTranscription,
        )
        .await;

        let outcome = result.expect("outcome");
        assert!(outcome.note.is_file(), "the note is written first");
        assert!(!dir.path().join("mic.opus").exists());
        assert!(!dir.path().join("system.opus").exists());
        assert!(
            dir.path().join("keep-me.txt").is_file(),
            "deletion must touch only the recorded tracks"
        );
    }

    #[tokio::test]
    async fn keeping_audio_is_the_default_behaviour() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);

        let transcriber = FakeTranscriber::new(vec![(Track::Mic, "my line")]);
        let (result, _) = run(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            AudioRetention::Keep,
        )
        .await;

        assert!(result.is_ok());
        assert!(dir.path().join("mic.opus").is_file());
    }
}
