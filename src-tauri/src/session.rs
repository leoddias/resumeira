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

        let built = match inner.factory.build(&folder_root) {
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

        match RecordingSession::start(&folder_root, specs, converter) {
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
    pub fn stop(&self) -> (RecordingState, Option<StopReport>) {
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
        };
        (inner.state.clone(), Some(report))
    }

    /// Moves to a post-recording stage. Ignored unless the app is processing.
    pub fn set_stage(&self, stage: ProcessingStage) -> RecordingState {
        match self.inner.lock() {
            Ok(mut inner) => {
                if matches!(inner.state, RecordingState::Processing { .. }) {
                    inner.state = RecordingState::Processing { stage };
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
    }

    impl TrackFactory for FakeFactory {
        fn build(&self, _folder: &Path) -> Result<Vec<(TrackSpec, String)>, AudioError> {
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
        let (state, report) = manager.stop();
        assert_eq!(
            state,
            RecordingState::Processing {
                stage: ProcessingStage::Saving
            }
        );
        let report = report.expect("a stopped meeting reports what it recorded");
        assert_eq!(report.tracks.len(), 2);
        assert!(!manager.state().is_capturing());
    }

    #[test]
    fn stopping_when_idle_is_harmless() {
        let (manager, _dir) = manager(Track::all().to_vec());
        let (state, report) = manager.stop();
        assert_eq!(state, RecordingState::Idle);
        assert!(report.is_none());
    }

    #[test]
    fn stopping_twice_reports_nothing_the_second_time() {
        let (manager, _dir) = manager(Track::all().to_vec());
        manager.start(1_000);
        let (_, first) = manager.stop();
        let (_, second) = manager.stop();
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
    fn finish_returns_to_idle() {
        let (manager, _dir) = manager(Track::all().to_vec());
        manager.start(1_000);
        manager.stop();
        assert_eq!(manager.finish(), RecordingState::Idle);
        assert!(manager.state().can_start());
    }

    #[test]
    fn stage_only_advances_while_processing() {
        let (manager, _dir) = manager(Track::all().to_vec());
        manager.set_stage(ProcessingStage::Transcribing);
        assert_eq!(manager.state(), RecordingState::Idle);

        manager.start(1_000);
        manager.stop();
        assert_eq!(
            manager.set_stage(ProcessingStage::Transcribing),
            RecordingState::Processing {
                stage: ProcessingStage::Transcribing
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
        })
        .expect("serialize");
        assert_eq!(
            processing,
            serde_json::json!({ "status": "processing", "stage": "transcribing" })
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
}
