//! End-to-end check against real audio hardware.
//!
//! Every other test in this project uses synthetic buffers, and twice that
//! was not enough: a green suite once hid an app that recorded nothing, and
//! later hid a fix that would have killed the microphone at the start of
//! every meeting. Both were found only by opening real devices.
//!
//! This test goes one layer further than those: it records from the actual
//! microphone and loopback device, encodes to Opus, decodes it back, and
//! drives the real pipeline through to a written note. Transcription and
//! summarization are stubbed, because a Whisper model is 1.6 GB and an LLM
//! call needs a key — everything *between* the microphone and `notes.md` is
//! the real thing.
//!
//! Ignored by default, since it needs hardware. Run it deliberately:
//!
//! ```text
//! cargo test --manifest-path src-tauri/Cargo.toml --test end_to_end -- --ignored --nocapture
//! ```
//!
//! Play some audio while it runs to give the loopback track something to
//! capture; it passes either way, but the byte counts are more interesting.

use resumeira_lib::audio::{decoder, Track};
use resumeira_lib::config::AudioRetention;
use resumeira_lib::pipeline::{self, Summarizer, Transcriber};
use resumeira_lib::recorder::RecordingSession;
use resumeira_lib::session::TrackFactory;
use resumeira_lib::storage;
use resumeira_lib::summarize::{ActionItem, SummarizeError, Summary};
use resumeira_lib::tracks::LiveTrackFactory;
use resumeira_lib::transcribe::{Engine, Segment, TranscribeError, Transcript};
use std::path::Path;
use std::time::Duration;

/// Stands in for Whisper: reports how much audio actually decoded, so the
/// assertions can tell "the pipeline ran" from "the pipeline ran on silence".
struct MeasuringTranscriber;

impl Transcriber for MeasuringTranscriber {
    async fn transcribe(
        &self,
        track: Track,
        audio: &Path,
        progress: pipeline::Sink<pipeline::TrackProgress>,
    ) -> Result<Transcript, TranscribeError> {
        progress.report(pipeline::TrackProgress {
            percent: Some(100),
            line: None,
        });
        let samples = decoder::decode_opus_file(audio).map_err(|error| {
            TranscribeError::LocalEngine(format!("decoding the recorded track failed: {error}"))
        })?;

        let seconds = samples.len() as f64 / 16_000.0;
        let peak = samples.iter().fold(0.0_f32, |acc, s| acc.max(s.abs()));
        println!(
            "{track:?}: {} samples ({seconds:.2}s), peak {peak:.4}",
            samples.len()
        );

        assert!(
            !samples.is_empty(),
            "{track:?} decoded to nothing — the recording never reached disk"
        );

        Ok(Transcript {
            segments: vec![Segment {
                start: 0.0,
                end: seconds,
                text: format!("{track:?} captured {seconds:.2} seconds"),
                track: Some(track),
                speaker: None,
            }],
            language: Some("en".to_owned()),
            engine: Engine::Local,
        })
    }
}

/// Stands in for the LLM, so no key and no network are needed.
struct StubSummarizer;

impl Summarizer for StubSummarizer {
    async fn summarize(&self, transcript: &Transcript) -> Result<Summary, SummarizeError> {
        Ok(Summary {
            title: "Hardware smoke test".to_owned(),
            bullets: transcript
                .segments
                .iter()
                .map(|segment| segment.text.clone())
                .collect(),
            decisions: vec!["The pipeline ran end to end".to_owned()],
            action_items: vec![ActionItem {
                task: "Record a real meeting".to_owned(),
                owner: None,
                due: None,
            }],
            model: "stub".to_owned(),
        })
    }
}

#[tokio::test]
#[ignore = "needs a real microphone and output device"]
async fn a_real_recording_becomes_a_real_note() {
    let root = tempfile::tempdir().expect("temp dir");

    // 1. Record from the actual devices, through the actual factory the app
    //    uses — not a fake. This is the part that unit tests cannot reach.
    // The folder must exist before the factory runs: writers open their
    // files inside it. Getting this backwards is what the app itself got
    // wrong until this test caught it.
    let folder = resumeira_lib::recorder::create_meeting_folder(root.path())
        .expect("the meeting folder is created");
    let factory = LiveTrackFactory;
    let built = factory
        .build(&folder)
        .expect("the factory builds track specs");
    assert!(
        !built.is_empty(),
        "no track could be prepared; is any audio device present?"
    );
    for (spec, device_name) in &built {
        println!("prepared {:?} on '{}'", spec.0, device_name);
    }

    let specs = built.into_iter().map(|(spec, _)| spec).collect();
    let mut session = RecordingSession::start_in(folder.clone(), specs, factory.converter())
        .expect("a session starts");

    println!("recording for 3 seconds into {}", folder.display());
    std::thread::sleep(Duration::from_secs(3));

    let report = session.stop();
    for track in &report.tracks {
        println!(
            "{:?}: {} samples, error {:?}",
            track.track, track.sample_count, track.error
        );
    }
    assert!(
        report.tracks.iter().any(|t| t.sample_count > 0),
        "no track captured a single sample: {:?}",
        report.tracks
    );

    // 2. Drive the real pipeline over that audio, with the two expensive
    //    halves stubbed.
    let outcome = pipeline::process(
        &folder,
        &MeasuringTranscriber,
        &StubSummarizer,
        None::<&resumeira_lib::live::LiveIdentifier>,
        AudioRetention::Keep,
        |stage| println!("stage: {stage:?}"),
        pipeline::Sink::new(|progress: pipeline::Transcribing| {
            println!(
                "transcribing {:?} ({}/{}): {:?}%",
                progress.track, progress.index, progress.total, progress.percent
            );
        }),
    )
    .await
    .expect("the pipeline turns the recording into a note");

    // 3. The note exists, and reads back as what was written.
    assert!(
        outcome.note.is_file(),
        "no note at {}",
        outcome.note.display()
    );
    let parsed = storage::read_note(&folder).expect("the note parses back");
    assert_eq!(parsed.title, "Hardware smoke test");
    assert!(
        !parsed.transcript_text.trim().is_empty(),
        "the note has no transcript section"
    );

    // 4. The audio is still there, because retention said keep.
    assert!(
        Track::all()
            .iter()
            .any(|track| folder.join(format!("{}.opus", track.file_stem())).is_file()),
        "the recording was deleted despite AudioRetention::Keep"
    );

    println!("note written to {}", outcome.note.display());
}
