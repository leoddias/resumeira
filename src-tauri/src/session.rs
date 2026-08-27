//! App-level recording state: what the tray and the window both talk to.
//!
//! [`RecordingSession`](crate::recorder::RecordingSession) owns one meeting's
//! pipeline. This module owns *whether there is a meeting*, publishes that as
//! the state the UI renders, and is the single place a recording can be
//! started or stopped from — the tray and the window must never disagree
//! about whether a microphone is live.
//!
//! Track construction is injected through [`TrackFactory`] so this state
//! machine is testable without a microphone, a loopback device, or an encoder.

use crate::audio::{AudioError, ChunkConverter, Track};
use crate::recorder::{RecordingSession, StopReport, TrackSpec};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;

/// Post-recording work, reported coarsely so the UI can say something true
/// while the user waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessingStage {
    Transcribing,
    Identifying,
    Summarizing,
    Saving,
}

/// One track as the UI sees it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackStatus {
    pub track: Track,
    pub device_name: String,
    /// A track can stop on its own without ending the meeting.
    pub live: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One track's current loudness, as the UI sees it.
///
/// Serializes to `TrackLevel` in `src/ipc/types.ts`; the shape is asserted
/// in this module's tests so the two cannot drift apart silently.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackLevel {
    pub track: Track,
    /// Bar height in `0.0..=1.0`, already scaled for display.
    pub level: f32,
    /// Whether the device is delivering audio at all. Silence from a live
    /// device reads `level: 0.0, receiving: true`; a device that is not
    /// there reads `false`, and the UI says something different about each.
    pub receiving: bool,
}

/// How far the transcription step has got, for the UI.
///
/// Carries a line of the meeting, so it goes to the window and nowhere else:
/// never to a log, never to disk (docs/CONVENTIONS.md § Privacy). It is
/// cleared the moment transcription ends, so no fragment of a meeting is
/// left sitting in the app's state.
///
/// Serializes to `TranscribeProgress` in `src/ipc/types.ts`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscribeProgress {
    pub track: Track,
    /// 1-based position among the tracks this meeting recorded.
    pub index: usize,
    pub total: usize,
    /// 0-100, or absent for an engine that cannot say — the UI shows an
    /// indeterminate bar rather than a made-up number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub percent: Option<u32>,
    /// The most recent line the engine produced, when it produced one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
}

/// What the app is doing right now.
///
/// Serializes to the discriminated union declared in `src/ipc/types.ts`;
/// the shape is asserted in this module's tests so the two cannot drift
/// apart silently.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum RecordingState {
    Idle,
    Starting,
    #[serde(rename_all = "camelCase")]
    Recording {
        /// Unix milliseconds, for the elapsed-time display.
        started_at: i64,
        tracks: Vec<TrackStatus>,
    },
    Stopping,
    #[serde(rename_all = "camelCase")]
    Processing {
        stage: ProcessingStage,
        /// Unix milliseconds when the pipeline started, so the UI can show
        /// how long the wait has been rather than a label that never moves.
        started_at: i64,
        /// Only ever set during [`ProcessingStage::Transcribing`].
        #[serde(skip_serializing_if = "Option::is_none")]
        transcribing: Option<TranscribeProgress>,
    },
    #[serde(rename_all = "camelCase")]
    Failed {
        error: String,
    },
}

impl RecordingState {
    /// Whether a microphone is live. The UI must never claim otherwise while
    /// this is true.
    pub fn is_capturing(&self) -> bool {
        matches!(
            self,
            RecordingState::Recording { .. } | RecordingState::Starting
        )
    }

    /// Whether a new recording may begin.
    pub fn can_start(&self) -> bool {
        matches!(self, RecordingState::Idle | RecordingState::Failed { .. })
    }

    /// Whether the current recording may be stopped.
    pub fn can_stop(&self) -> bool {
        matches!(self, RecordingState::Recording { .. })
    }
}

/// Builds the capture sources and writers for one meeting.
///
/// Injected so the state machine can be tested with fakes; the real
/// implementation pairs the platform capture sources with Opus writers in
/// the meeting folder.
pub trait TrackFactory: Send {
    /// Tracks to record, with the device name to show for each.
    ///
    /// Called once per meeting, after the folder exists. Returning fewer
    /// tracks than requested is allowed — a machine with no loopback device
    /// still records the microphone.
    fn build(&self, folder: &std::path::Path) -> Result<Vec<(TrackSpec, String)>, AudioError>;

    /// Converter applied to every captured chunk.
    fn converter(&self) -> ChunkConverter;
}

/// Owns the current meeting, if any.
pub struct SessionManager {
    inner: Mutex<Inner>,
}

struct Inner {
    notes_root: PathBuf,
    factory: Box<dyn TrackFactory>,
    state: RecordingState,
    session: Option<RecordingSession>,
}

impl SessionManager {
    pub fn new(notes_root: PathBuf, factory: Box<dyn TrackFactory>) -> Self {
        Self {
            inner: Mutex::new(Inner {
                notes_root,
                factory,
                state: RecordingState::Idle,
                session: None,
            }),
        }
    }

    /// The current state. Never panics: a poisoned lock reports a failure
    /// rather than taking the app down mid-meeting.
    ///
    /// While recording, per-track liveness is read from the running session
    /// rather than from the cached state, so a device that died is reported
    /// as dead the next time anyone asks.
    pub fn state(&self) -> RecordingState {
        let Ok(inner) = self.inner.lock() else {
            return RecordingState::Failed {
                error: "recording state is unavailable".to_owned(),
            };
        };

        match (&inner.state, &inner.session) {
            (RecordingState::Recording { started_at, tracks }, Some(session)) => {
                let liveness = session.track_liveness();
                RecordingState::Recording {
                    started_at: *started_at,
                    tracks: tracks
                        .iter()
                        .map(
                            |status| match liveness.iter().find(|l| l.track == status.track) {
                                Some(current) => TrackStatus {
                                    track: status.track,
                                    device_name: status.device_name.clone(),
                                    live: current.live,
                                    error: current.error.clone(),
                                },
                                None => status.clone(),
                            },
                        )
                        .collect(),
                }
            }
            _ => inner.state.clone(),
        }
    }

    /// How loud each track is right now, for the UI's activity meter.
    ///
    /// Deliberately not part of [`RecordingState`]: this is polled several
    /// times a second, and folding it into the state would push a full state
    /// event — tray included — at the same rate. Empty when nothing is
    /// recording, so a stale poll after `stop` cannot leave a bar standing.
    pub fn levels(&self) -> Vec<TrackLevel> {
        let Ok(inner) = self.inner.lock() else {
            return Vec::new();
        };
        match (&inner.state, &inner.session) {
            (RecordingState::Recording { .. }, Some(session)) => session
                .track_levels()
                .into_iter()
                .map(|level| TrackLevel {
                    track: level.track,
                    level: level.level,
                    receiving: level.receiving,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Whether a recording is running with every track dead.
    ///
    /// Nothing more will be captured, so continuing to display "Recording"
    /// would be a lie. The caller ends the meeting properly rather than
    /// having this getter do it, so the captured audio still goes through
    /// the note pipeline.
    pub fn all_tracks_dead(&self) -> bool {
        let Ok(inner) = self.inner.lock() else {
            return false;
        };
        match (&inner.state, &inner.session) {
            (RecordingState::Recording { .. }, Some(session)) => {
                let liveness = session.track_liveness();
                !liveness.is_empty() && !liveness.iter().any(|track| track.live)
            }
            _ => false,
        }
    }

    /// Starts a meeting. Returns the state the app moved to, which is a
    /// `Failed` state rather than an `Err` when starting did not work — the
    /// UI renders states, not exceptions.
    pub fn start(&self, now_ms: i64) -> RecordingState {
        let mut inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(_) => {
                return RecordingState::Failed {
                    error: "recording state is unavailable".to_owned(),
                }
            }
        };

        if !inner.state.can_start() {
            return inner.state.clone();
        }

        let folder_root = inner.notes_root.clone();
        let converter = inner.factory.converter();

        // The meeting folder has to exist before the factory runs: track
        // writers open their files *inside* it. Building them against the
        // notes root instead put the audio beside the meeting folder rather
        // than in it, and every meeting then reported "no audio was
        // recorded" while the recording sat one directory up.
        let folder = match crate::recorder::create_meeting_folder(&folder_root) {
            Ok(folder) => folder,
            Err(error) => return inner.fail(error.to_string()),
        };

        let built = match inner.factory.build(&folder) {
            Ok(built) => built,
            Err(error) => return inner.fail(error.to_string()),
        };
        if built.is_empty() {
            return inner.fail("no audio devices available to record".to_owned());
        }

        let mut device_names = Vec::with_capacity(built.len());
        let mut specs = Vec::with_capacity(built.len());
        for (spec, device_name) in built {
            device_names.push((spec.0, device_name));
            specs.push(spec);
        }

        match RecordingSession::start_in(folder, specs, converter) {
            Ok(session) => {
                // Seeded from what actually started, not from optimism: a
                // device that failed to open must show dead from the first
                // frame, not two seconds later when the UI next asks.
                let liveness = session.track_liveness();
                let tracks: Vec<TrackStatus> = device_names
                    .into_iter()
                    .map(|(track, device_name)| {
                        let current = liveness.iter().find(|l| l.track == track);
                        TrackStatus {
                            track,
                            device_name,
                            live: current.map(|l| l.live).unwrap_or(false),
                            error: current.and_then(|l| l.error.clone()),
                        }
                    })
                    .collect();

                if !tracks.iter().any(|track| track.live) {
                    // Nothing is being captured. Saying "Recording" here,
                    // with a ticking clock, is the single most damaging lie
                    // this app can tell: the user walks away believing the
                    // meeting is being saved.
                    let reason = tracks
                        .iter()
                        .find_map(|track| track.error.clone())
                        .unwrap_or_else(|| "no audio device could be opened".to_owned());
                    return inner.fail(reason);
                }

                inner.session = Some(session);
                inner.state = RecordingState::Recording {
                    started_at: now_ms,
                    tracks,
                };
                inner.state.clone()
            }
            Err(error) => inner.fail(error.to_string()),
        }
    }

    /// Ends the meeting and returns both the new state and what was recorded.
    ///
    /// The report is `None` when there was nothing to stop.
    pub fn stop(&self, now_ms: i64) -> (RecordingState, Option<StopReport>) {
        let mut inner = match self.inner.lock() {
            Ok(inner) => inner,
            Err(_) => {
                return (
                    RecordingState::Failed {
                        error: "recording state is unavailable".to_owned(),
                    },
                    None,
                )
            }
        };

        if !inner.state.can_stop() {
            let state = inner.state.clone();
            return (state, None);
        }

        let Some(mut session) = inner.session.take() else {
            // can_stop() was true without a session: treat as a clean idle
            // rather than pretending a meeting is still running.
            inner.state = RecordingState::Idle;
            return (inner.state.clone(), None);
        };

        let report = session.stop();
        inner.state = RecordingState::Processing {
            stage: ProcessingStage::Saving,
            started_at: now_ms,
            transcribing: None,
        };
        (inner.state.clone(), Some(report))
    }

    /// Moves to a post-recording stage. Ignored unless the app is processing.
    ///
    /// Leaving [`ProcessingStage::Transcribing`] drops the preview: the
    /// transcript line it carries has no reason to outlive the step that
    /// produced it, and a stale one under "Writing notes…" would be a lie
    /// about what the app is doing.
    pub fn set_stage(&self, stage: ProcessingStage) -> RecordingState {
        match self.inner.lock() {
            Ok(mut inner) => {
                if let RecordingState::Processing { started_at, .. } = inner.state {
                    inner.state = RecordingState::Processing {
                        stage,
                        started_at,
                        transcribing: None,
                    };
                }
                inner.state.clone()
            }
            Err(_) => RecordingState::Failed {
                error: "recording state is unavailable".to_owned(),
            },
        }
    }

    /// Records how far transcription has got. Ignored unless that is the
    /// stage the app is actually in, so a late callback from a finished
    /// track cannot reopen a step the pipeline has already left.
    pub fn set_transcribe_progress(&self, progress: TranscribeProgress) -> RecordingState {
        match self.inner.lock() {
            Ok(mut inner) => {
                if let RecordingState::Processing {
                    stage: ProcessingStage::Transcribing,
                    started_at,
                    ..
                } = inner.state
                {
                    inner.state = RecordingState::Processing {
                        stage: ProcessingStage::Transcribing,
                        started_at,
                        transcribing: Some(progress),
                    };
                }
                inner.state.clone()
            }
            Err(_) => RecordingState::Failed {
                error: "recording state is unavailable".to_owned(),
            },
        }
    }

    /// Reports that the post-recording work failed.
    ///
    /// The recording itself already survived — this says the note did not
    /// get written, which the user needs to see rather than a silent return
    /// to idle that looks like success.
    pub fn fail(&self, error: String) -> RecordingState {
        match self.inner.lock() {
            Ok(mut inner) => {
                inner.session = None;
                inner.state = RecordingState::Failed { error };
                inner.state.clone()
            }
            Err(_) => RecordingState::Failed {
                error: "recording state is unavailable".to_owned(),
            },
        }
    }

    /// Declares the post-recording work finished.
    pub fn finish(&self) -> RecordingState {
        match self.inner.lock() {
            Ok(mut inner) => {
                inner.state = RecordingState::Idle;
                inner.state.clone()
            }
            Err(_) => RecordingState::Failed {
                error: "recording state is unavailable".to_owned(),
            },
        }
    }
}

impl Inner {
    fn fail(&mut self, error: String) -> RecordingState {
        self.session = None;
        self.state = RecordingState::Failed { error };
        self.state.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{AudioChunk, CaptureSource, ChunkSink, ErrorSink, TrackWriter};
    use std::path::Path;
    use std::sync::Arc;

    struct SilentSource {
        name: String,
        fail_start: bool,
    }

    impl CaptureSource for SilentSource {
        fn start(&mut self, _sink: ChunkSink, _on_error: ErrorSink) -> Result<(), AudioError> {
            if self.fail_start {
                Err(AudioError::NoDevice("input"))
            } else {
                Ok(())
            }
        }
        fn stop(&mut self) -> Result<(), AudioError> {
            Ok(())
        }
        fn device_name(&self) -> String {
            self.name.clone()
        }
    }

    struct NullWriter;

    impl TrackWriter for NullWriter {
        fn write(&mut self, _samples: &[f32]) -> Result<(), AudioError> {
            Ok(())
        }
        fn finish(self: Box<Self>) -> Result<(), AudioError> {
            Ok(())
        }
    }

    struct FakeFactory {
        tracks: Vec<Track>,
        error: Option<&'static str>,
        /// Folder the factory was handed, so a test can prove it is the
        /// meeting folder and not the notes root.
        seen_folder: std::sync::Mutex<Option<PathBuf>>,
    }

    impl TrackFactory for FakeFactory {
        fn build(&self, folder: &Path) -> Result<Vec<(TrackSpec, String)>, AudioError> {
            if let Ok(mut seen) = self.seen_folder.lock() {
                *seen = Some(folder.to_path_buf());
            }
            if self.error.is_some() {
                return Err(AudioError::NoDevice("input"));
            }
            Ok(self
                .tracks
                .iter()
                .map(|track| {
                    let source: Box<dyn CaptureSource> = Box::new(SilentSource {
                        name: format!("{track:?} device"),
                        fail_start: false,
                    });
                    let writer: Box<dyn TrackWriter> = Box::new(NullWriter);
                    ((*track, source, writer), format!("{track:?} device"))
                })
                .collect())
        }

        fn converter(&self) -> ChunkConverter {
            Arc::new(|chunk: &AudioChunk| chunk.samples.clone())
        }
    }

    fn manager(tracks: Vec<Track>) -> (SessionManager, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let manager = SessionManager::new(
            dir.path().to_path_buf(),
            Box::new(FakeFactory {
                tracks,
                error: None,
                seen_folder: std::sync::Mutex::new(None),
            }),
        );
        (manager, dir)
    }

    #[test]
    fn starts_idle() {
        let (manager, _dir) = manager(Track::all().to_vec());
        assert_eq!(manager.state(), RecordingState::Idle);
        assert!(manager.state().can_start());
        assert!(!manager.state().can_stop());
    }

    #[test]
    fn start_reports_both_tracks_as_live() {
        let (manager, _dir) = manager(Track::all().to_vec());
        let state = manager.start(1_000);
        match state {
            RecordingState::Recording { started_at, tracks } => {
                assert_eq!(started_at, 1_000);
                assert_eq!(tracks.len(), 2);
                assert!(tracks.iter().all(|t| t.live));
            }
            other => panic!("expected Recording, got {other:?}"),
        }
        assert!(manager.state().is_capturing());
    }

    #[test]
    fn starting_twice_does_not_replace_a_running_meeting() {
        let (manager, _dir) = manager(Track::all().to_vec());
        let first = manager.start(1_000);
        let second = manager.start(2_000);
        assert_eq!(first, second, "the second start must be a no-op");
    }

    #[test]
    fn stop_moves_to_processing_and_returns_a_report() {
        let (manager, _dir) = manager(Track::all().to_vec());
        manager.start(1_000);
        let (state, report) = manager.stop(2_000);
        assert_eq!(
            state,
            RecordingState::Processing {
                stage: ProcessingStage::Saving,
                started_at: 2_000,
                transcribing: None,
            }
        );
        let report = report.expect("a stopped meeting reports what it recorded");
        assert_eq!(report.tracks.len(), 2);
        assert!(!manager.state().is_capturing());
    }

    #[test]
    fn stopping_when_idle_is_harmless() {
        let (manager, _dir) = manager(Track::all().to_vec());
        let (state, report) = manager.stop(2_000);
        assert_eq!(state, RecordingState::Idle);
        assert!(report.is_none());
    }

    #[test]
    fn stopping_twice_reports_nothing_the_second_time() {
        let (manager, _dir) = manager(Track::all().to_vec());
        manager.start(1_000);
        let (_, first) = manager.stop(2_000);
        let (_, second) = manager.stop(3_000);
        assert!(first.is_some());
        assert!(second.is_none());
    }

    #[test]
    fn a_factory_failure_surfaces_as_failed_not_as_a_silent_idle() {
        let dir = tempfile::tempdir().expect("temp dir");
        let manager = SessionManager::new(
            dir.path().to_path_buf(),
            Box::new(FakeFactory {
                tracks: vec![],
                error: Some("no devices"),
                seen_folder: std::sync::Mutex::new(None),
            }),
        );
        let state = manager.start(1_000);
        assert!(matches!(state, RecordingState::Failed { .. }));
        assert!(!state.is_capturing());
        assert!(state.can_start(), "the user must be able to retry");
    }

    #[test]
    fn a_machine_with_no_devices_fails_loudly() {
        let (manager, _dir) = manager(vec![]);
        let state = manager.start(1_000);
        assert!(matches!(state, RecordingState::Failed { .. }));
    }

    #[test]
    fn recording_with_only_one_track_is_allowed() {
        let (manager, _dir) = manager(vec![Track::Mic]);
        match manager.start(1_000) {
            RecordingState::Recording { tracks, .. } => assert_eq!(tracks.len(), 1),
            other => panic!("expected Recording, got {other:?}"),
        }
    }

    #[test]
    fn the_factory_builds_tracks_inside_the_meeting_folder() {
        // Regression: the factory used to be handed the notes root, so the
        // Opus writers opened their files one directory above the meeting
        // folder. Recording worked, the audio existed, and every meeting
        // still reported "no audio was recorded" because the pipeline looked
        // in the folder the session had created. No unit test caught it —
        // every fake factory ignored the path it was given.
        let dir = tempfile::tempdir().expect("temp dir");
        let factory = FakeFactory {
            tracks: Track::all().to_vec(),
            error: None,
            seen_folder: std::sync::Mutex::new(None),
        };
        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let probe = seen.clone();

        struct Probing {
            inner: FakeFactory,
            probe: std::sync::Arc<std::sync::Mutex<Option<PathBuf>>>,
        }

        impl TrackFactory for Probing {
            fn build(&self, folder: &Path) -> Result<Vec<(TrackSpec, String)>, AudioError> {
                if let Ok(mut probe) = self.probe.lock() {
                    *probe = Some(folder.to_path_buf());
                }
                self.inner.build(folder)
            }
            fn converter(&self) -> ChunkConverter {
                self.inner.converter()
            }
        }

        let notes_root = dir.path().to_path_buf();
        let manager = SessionManager::new(
            notes_root.clone(),
            Box::new(Probing {
                inner: factory,
                probe,
            }),
        );
        manager.start(1_000);

        let handed = seen.lock().expect("lock").clone().expect("factory ran");
        assert_ne!(
            handed, notes_root,
            "the factory must not be handed the notes root"
        );
        assert!(
            handed.starts_with(&notes_root),
            "the meeting folder must live under the notes root, got {handed:?}"
        );
        assert!(
            handed.is_dir(),
            "the meeting folder must exist before the factory runs"
        );
    }

    #[test]
    fn finish_returns_to_idle() {
        let (manager, _dir) = manager(Track::all().to_vec());
        manager.start(1_000);
        manager.stop(2_000);
        assert_eq!(manager.finish(), RecordingState::Idle);
        assert!(manager.state().can_start());
    }

    #[test]
    fn stage_only_advances_while_processing() {
        let (manager, _dir) = manager(Track::all().to_vec());
        manager.set_stage(ProcessingStage::Transcribing);
        assert_eq!(manager.state(), RecordingState::Idle);

        manager.start(1_000);
        manager.stop(2_000);
        assert_eq!(
            manager.set_stage(ProcessingStage::Transcribing),
            RecordingState::Processing {
                stage: ProcessingStage::Transcribing,
                started_at: 2_000,
                transcribing: None,
            }
        );
    }

    // These assertions pin the JSON shape to `src/ipc/types.ts`. If one
    // fails, the frontend contract changed and both sides must move together.
    #[test]
    fn serializes_to_the_shape_the_frontend_expects() {
        let idle = serde_json::to_value(RecordingState::Idle).expect("serialize");
        assert_eq!(idle, serde_json::json!({ "status": "idle" }));

        let recording = serde_json::to_value(RecordingState::Recording {
            started_at: 42,
            tracks: vec![TrackStatus {
                track: Track::Mic,
                device_name: "Test Mic".to_owned(),
                live: true,
                error: None,
            }],
        })
        .expect("serialize");
        assert_eq!(
            recording,
            serde_json::json!({
                "status": "recording",
                "startedAt": 42,
                "tracks": [{ "track": "mic", "deviceName": "Test Mic", "live": true }]
            })
        );

        let processing = serde_json::to_value(RecordingState::Processing {
            stage: ProcessingStage::Transcribing,
            started_at: 7,
            transcribing: None,
        })
        .expect("serialize");
        assert_eq!(
            processing,
            serde_json::json!({ "status": "processing", "stage": "transcribing", "startedAt": 7 })
        );

        let with_progress = serde_json::to_value(RecordingState::Processing {
            stage: ProcessingStage::Transcribing,
            started_at: 7,
            transcribing: Some(TranscribeProgress {
                track: Track::Mic,
                index: 1,
                total: 2,
                percent: Some(40),
                line: Some("so that is the plan".to_owned()),
            }),
        })
        .expect("serialize");
        assert_eq!(
            with_progress,
            serde_json::json!({
                "status": "processing",
                "stage": "transcribing",
                "startedAt": 7,
                "transcribing": {
                    "track": "mic",
                    "index": 1,
                    "total": 2,
                    "percent": 40,
                    "line": "so that is the plan"
                }
            })
        );

        let failed = serde_json::to_value(RecordingState::Failed {
            error: "no input device".to_owned(),
        })
        .expect("serialize");
        assert_eq!(
            failed,
            serde_json::json!({ "status": "failed", "error": "no input device" })
        );
    }

    #[test]
    fn a_track_error_is_reported_as_text_never_as_samples() {
        let status = TrackStatus {
            track: Track::System,
            device_name: "Speakers".to_owned(),
            live: false,
            error: Some(AudioError::NoDevice("output").to_string()),
        };
        let value = serde_json::to_value(&status).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "track": "system",
                "deviceName": "Speakers",
                "live": false,
                "error": "no output device available"
            })
        );
    }

    #[test]
    fn levels_are_empty_unless_a_recording_is_running() {
        let (manager, _dir) = manager(vec![Track::Mic]);
        assert!(manager.levels().is_empty(), "idle must not draw a meter");

        manager.start(0);
        assert_eq!(manager.levels().len(), 1);

        manager.stop(2_000);
        assert!(
            manager.levels().is_empty(),
            "a poll landing after stop must not leave a bar standing"
        );
    }

    #[test]
    fn a_silent_but_live_track_reports_an_empty_meter() {
        let (manager, _dir) = manager(vec![Track::Mic, Track::System]);
        manager.start(0);

        let levels = manager.levels();
        assert_eq!(levels.len(), 2, "one row per track, always");
        assert!(levels.iter().all(|l| l.level == 0.0));
        assert!(
            levels.iter().all(|l| !l.receiving),
            "no chunk has arrived yet, so nothing is receiving"
        );
    }

    #[test]
    fn track_level_serializes_to_the_shape_the_frontend_expects() {
        let level = serde_json::to_value(TrackLevel {
            track: Track::System,
            level: 0.5,
            receiving: true,
        })
        .expect("serialize");
        assert_eq!(
            level,
            serde_json::json!({ "track": "system", "level": 0.5, "receiving": true })
        );
    }

    #[test]
    fn transcription_progress_is_kept_only_while_transcribing() {
        let (manager, _dir) = manager(vec![Track::Mic]);
        let progress = TranscribeProgress {
            track: Track::Mic,
            index: 1,
            total: 1,
            percent: Some(30),
            line: Some("what someone said".to_owned()),
        };

        // Nothing is running: a stray callback must not invent a state.
        assert_eq!(
            manager.set_transcribe_progress(progress.clone()),
            RecordingState::Idle
        );

        manager.start(1_000);
        manager.stop(2_000);
        manager.set_stage(ProcessingStage::Transcribing);
        assert_eq!(
            manager.set_transcribe_progress(progress.clone()),
            RecordingState::Processing {
                stage: ProcessingStage::Transcribing,
                started_at: 2_000,
                transcribing: Some(progress.clone()),
            }
        );

        // A late callback from a finished track must not reopen a step the
        // pipeline has already left.
        manager.set_stage(ProcessingStage::Summarizing);
        assert_eq!(
            manager.set_transcribe_progress(progress),
            RecordingState::Processing {
                stage: ProcessingStage::Summarizing,
                started_at: 2_000,
                transcribing: None,
            }
        );
    }

    #[test]
    fn no_fragment_of_the_meeting_outlives_the_transcription_step() {
        let (manager, _dir) = manager(vec![Track::Mic]);
        manager.start(1_000);
        manager.stop(2_000);
        manager.set_stage(ProcessingStage::Transcribing);
        manager.set_transcribe_progress(TranscribeProgress {
            track: Track::Mic,
            index: 1,
            total: 1,
            percent: Some(90),
            line: Some("something confidential".to_owned()),
        });

        let state = manager.set_stage(ProcessingStage::Summarizing);
        match state {
            RecordingState::Processing { transcribing, .. } => assert!(transcribing.is_none()),
            other => panic!("expected processing, got {other:?}"),
        }
    }

    #[test]
    fn the_processing_clock_starts_when_the_recording_stops() {
        let (manager, _dir) = manager(vec![Track::Mic]);
        manager.start(1_000);
        manager.stop(5_000);

        for stage in [
            ProcessingStage::Transcribing,
            ProcessingStage::Identifying,
            ProcessingStage::Summarizing,
        ] {
            match manager.set_stage(stage) {
                RecordingState::Processing { started_at, .. } => assert_eq!(
                    started_at, 5_000,
                    "the wait is measured from one moment, not restarted per stage"
                ),
                other => panic!("expected processing, got {other:?}"),
            }
        }
    }
}
