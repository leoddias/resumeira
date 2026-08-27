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
use crate::diarize::Turn;
use crate::storage;
use crate::summarize::{SummarizeError, Summary};
use crate::transcribe::{TranscribeError, Transcript};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A cloneable, optional callback the pipeline hands to its steps.
///
/// Optional because every test and the CLI summarizer want the pipeline
/// without a UI attached; cloneable and `Send + Sync + 'static` because the
/// local engine hands its copy to a blocking worker thread.
pub struct Sink<T>(Option<Arc<dyn Fn(T) + Send + Sync>>);

impl<T> Sink<T> {
    pub fn new(report: impl Fn(T) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(report)))
    }

    /// A sink nobody is listening to.
    pub fn silent() -> Self {
        Self(None)
    }

    pub fn report(&self, value: T) {
        if let Some(sink) = &self.0 {
            sink(value);
        }
    }
}

impl<T> Clone for Sink<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Default for Sink<T> {
    fn default() -> Self {
        Self::silent()
    }
}

/// Movement inside one track's transcription, as the engine sees it.
///
/// Transcribing an hour of audio takes minutes with nothing to show for it,
/// which is indistinguishable from a hang. Engines that can say how far they
/// are, and what they have heard, say so through this.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrackProgress {
    /// How much of this track has been decoded, 0-100. `None` from an engine
    /// that cannot say — a single cloud request either returns or does not.
    pub percent: Option<u32>,
    /// A line the engine has just produced, when it produced one.
    pub line: Option<String>,
}

/// The same movement, placed in the whole transcription step.
#[derive(Debug, Clone, PartialEq)]
pub struct Transcribing {
    pub track: Track,
    /// 1-based position of this track among the ones being transcribed.
    pub index: usize,
    pub total: usize,
    pub percent: Option<u32>,
    pub line: Option<String>,
}

/// Where the pipeline is, so the UI can say something true while it waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Transcribing,
    /// Working out who spoke each line (ADR-0021). Skipped when the user
    /// turned speaker identification off.
    Identifying,
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

impl PipelineError {
    /// The same failure, safe to write to a log file.
    ///
    /// Only the summary step can carry text this app did not write — a
    /// provider's error body or a CLI's stderr, either of which can quote the
    /// transcript back. See [`SummarizeError::log_safe`].
    pub fn log_safe(&self) -> String {
        match self {
            PipelineError::Summarize(error) => error.log_safe(),
            other => other.to_string(),
        }
    }
}

/// Turns one track's audio file into a transcript.
pub trait Transcriber {
    fn transcribe(
        &self,
        track: Track,
        audio: &Path,
        progress: Sink<TrackProgress>,
    ) -> impl std::future::Future<Output = Result<Transcript, TranscribeError>> + Send;
}

/// Works out who spoke each line of a transcript.
///
/// Returns turns rather than a labelled transcript so the step stays pure at
/// this seam: `diarize::apply` decides what a turn is allowed to touch, and
/// it is tested on its own.
pub trait SpeakerIdentifier {
    fn identify(
        &self,
        transcript: &Transcript,
    ) -> impl std::future::Future<Output = Result<Vec<Turn>, SummarizeError>> + Send;
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
pub async fn process<T, S, D, P>(
    folder: &Path,
    transcriber: &T,
    summarizer: &S,
    identifier: Option<&D>,
    retention: AudioRetention,
    mut on_stage: P,
    on_transcribing: Sink<Transcribing>,
) -> Result<Outcome, PipelineError>
where
    T: Transcriber,
    S: Summarizer,
    D: SpeakerIdentifier,
    P: FnMut(Stage),
{
    on_stage(Stage::Transcribing);

    // One missing track is normal: a machine with no loopback device still
    // records the microphone. Counted up front so "track 1 of 2" is the
    // truth about this meeting rather than about the two tracks in theory.
    let recorded: Vec<(Track, PathBuf)> = Track::all()
        .into_iter()
        .map(|track| (track, folder.join(format!("{}.opus", track.file_stem()))))
        .filter(|(_, audio)| audio.is_file())
        .collect();
    let total = recorded.len();

    let mut parts = Vec::new();
    let mut failure: Option<TranscribeError> = None;
    for (position, (track, audio)) in recorded.into_iter().enumerate() {
        let index = position + 1;
        let sink = on_transcribing.clone();
        let per_track = Sink::new(move |progress: TrackProgress| {
            sink.report(Transcribing {
                track,
                index,
                total,
                percent: progress.percent,
                line: progress.line,
            });
        });
        // An engine that reports nothing still moves the counter, so a
        // second track is visibly under way rather than looking stuck on the
        // first one's last reading.
        per_track.report(TrackProgress::default());

        match transcriber.transcribe(track, &audio, per_track).await {
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

    let mut transcript = transcript;
    if let Some(identifier) = identifier {
        on_stage(Stage::Identifying);
        match identifier.identify(&transcript).await {
            Ok(turns) => crate::diarize::apply(&mut transcript, &turns),
            // Never fatal. A note whose lines say "You" and "Others" is worth
            // far more than no note, and this step is the only one in the
            // pipeline whose absence costs nothing but detail.
            Err(error) => log::warn!("could not identify speakers: {}", error.log_safe()),
        }
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
            speaker: None,
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
            progress: Sink<TrackProgress>,
        ) -> Result<Transcript, TranscribeError> {
            self.seen.lock().expect("lock").push(track);
            progress.report(TrackProgress {
                percent: Some(100),
                line: Some(format!("heard on {track:?}")),
            });
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
                bullets: vec![transcript.to_prompt_text()],
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

    /// Answers with fixed turns, or fails, standing in for the model.
    struct FakeIdentifier {
        turns: Vec<Turn>,
        fails: bool,
    }

    impl FakeIdentifier {
        fn returning(turns: Vec<Turn>) -> Self {
            Self {
                turns,
                fails: false,
            }
        }

        fn failing() -> Self {
            Self {
                turns: Vec::new(),
                fails: true,
            }
        }
    }

    impl SpeakerIdentifier for FakeIdentifier {
        async fn identify(&self, _transcript: &Transcript) -> Result<Vec<Turn>, SummarizeError> {
            if self.fails {
                return Err(SummarizeError::EmptySummary);
            }
            Ok(self.turns.clone())
        }
    }

    async fn run(
        folder: &Path,
        transcriber: &FakeTranscriber,
        summarizer: &FakeSummarizer,
        retention: AudioRetention,
    ) -> (Result<Outcome, PipelineError>, Vec<Stage>) {
        run_with(
            folder,
            transcriber,
            summarizer,
            None::<&FakeIdentifier>,
            retention,
        )
        .await
    }

    async fn run_with(
        folder: &Path,
        transcriber: &FakeTranscriber,
        summarizer: &FakeSummarizer,
        identifier: Option<&FakeIdentifier>,
        retention: AudioRetention,
    ) -> (Result<Outcome, PipelineError>, Vec<Stage>) {
        let (result, stages, _) =
            run_reporting(folder, transcriber, summarizer, identifier, retention).await;
        (result, stages)
    }

    /// Same as [`run_with`], plus everything the transcription step reported.
    async fn run_reporting(
        folder: &Path,
        transcriber: &FakeTranscriber,
        summarizer: &FakeSummarizer,
        identifier: Option<&FakeIdentifier>,
        retention: AudioRetention,
    ) -> (
        Result<Outcome, PipelineError>,
        Vec<Stage>,
        Vec<Transcribing>,
    ) {
        let mut stages = Vec::new();
        let reported = Arc::new(Mutex::new(Vec::new()));
        let sink = reported.clone();
        let result = process(
            folder,
            transcriber,
            summarizer,
            identifier,
            retention,
            |stage| stages.push(stage),
            Sink::new(move |progress| sink.lock().expect("lock").push(progress)),
        )
        .await;
        let reported = reported.lock().expect("lock").clone();
        (result, stages, reported)
    }

    fn turn(from: usize, to: usize, speaker: &str) -> Turn {
        Turn {
            from,
            to,
            speaker: speaker.to_owned(),
        }
    }

    #[tokio::test]
    async fn identified_speakers_reach_the_note_and_the_summarizer() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);
        write_track(dir.path(), Track::System);
        let transcriber =
            FakeTranscriber::new(vec![(Track::Mic, "my line"), (Track::System, "their line")]);
        let identifier = FakeIdentifier::returning(vec![turn(0, 0, "Leo"), turn(1, 1, "Ana")]);

        let (result, stages) = run_with(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            Some(&identifier),
            AudioRetention::Keep,
        )
        .await;

        let outcome = result.expect("outcome");
        assert_eq!(
            outcome.transcript.participants(),
            vec!["Leo".to_owned(), "Ana".to_owned()]
        );
        assert!(
            stages.contains(&Stage::Identifying),
            "the user is told what the wait is for: {stages:?}"
        );
        assert!(
            std::fs::read_to_string(outcome.note.clone())
                .expect("note")
                .contains("Ana: their line"),
            "the label has to survive to disk, not just to the summarizer"
        );
    }

    #[tokio::test]
    async fn a_failed_speaker_step_still_produces_the_note() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);
        let transcriber = FakeTranscriber::new(vec![(Track::Mic, "my line")]);

        let (result, _) = run_with(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            Some(&FakeIdentifier::failing()),
            AudioRetention::Keep,
        )
        .await;

        let outcome = result.expect("a meeting is never lost to an unnamed speaker");
        assert!(
            outcome.transcript.participants().is_empty(),
            "nothing identified means nobody named, never a guess"
        );
    }

    #[tokio::test]
    async fn no_identifier_means_no_identifying_stage() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);
        let transcriber = FakeTranscriber::new(vec![(Track::Mic, "my line")]);

        let (result, stages) = run(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            AudioRetention::Keep,
        )
        .await;

        result.expect("outcome");
        assert!(
            !stages.contains(&Stage::Identifying),
            "a step the user turned off must not report progress: {stages:?}"
        );
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
        assert_eq!(
            outcome.transcript.to_prompt_text(),
            "You: my line\nOthers: their line",
            "with no speaker step, the track is still what the summarizer is told"
        );
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
        assert_eq!(outcome.transcript.to_prompt_text(), "You: my line");
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

    #[tokio::test]
    async fn every_track_reports_its_place_in_the_transcription_step() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);
        write_track(dir.path(), Track::System);

        let transcriber =
            FakeTranscriber::new(vec![(Track::Mic, "my line"), (Track::System, "their line")]);
        let (result, _, reported) = run_reporting(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            None,
            AudioRetention::Keep,
        )
        .await;
        result.expect("the pipeline produces a note");

        assert!(
            reported.iter().all(|p| p.total == 2),
            "both tracks were recorded, so both must be counted: {reported:?}"
        );
        assert_eq!(
            reported.iter().map(|p| p.index).collect::<Vec<_>>(),
            vec![1, 1, 2, 2],
            "each track opens with an empty reading before the engine speaks"
        );
        assert!(
            reported
                .iter()
                .any(|p| p.track == Track::Mic && p.line.as_deref() == Some("heard on Mic")),
            "what the engine heard must reach the caller: {reported:?}"
        );
    }

    #[tokio::test]
    async fn a_meeting_with_one_track_is_not_counted_as_two() {
        // The machine had no loopback device. Saying "1 of 2" would leave
        // the user waiting for a track that was never recorded.
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);

        let transcriber = FakeTranscriber::new(vec![(Track::Mic, "my line")]);
        let (result, _, reported) = run_reporting(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            None,
            AudioRetention::Keep,
        )
        .await;
        result.expect("the pipeline produces a note");

        assert!(!reported.is_empty());
        assert!(reported.iter().all(|p| p.total == 1 && p.index == 1));
    }

    #[tokio::test]
    async fn a_pipeline_with_no_listener_still_runs() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_track(dir.path(), Track::Mic);

        let transcriber = FakeTranscriber::new(vec![(Track::Mic, "my line")]);
        let result = process(
            dir.path(),
            &transcriber,
            &FakeSummarizer { fail: false },
            None::<&FakeIdentifier>,
            AudioRetention::Keep,
            |_| {},
            Sink::silent(),
        )
        .await;

        assert!(result.is_ok(), "a silent sink must not change the outcome");
    }
}
