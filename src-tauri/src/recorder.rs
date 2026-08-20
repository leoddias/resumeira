//! Recording session lifecycle: owns the meeting folder and the per-track
//! pipeline (`CaptureSource` chunk -> `ChunkConverter` -> `TrackWriter`).
//!
//! The converter is injected rather than called directly (see
//! [`crate::audio::ChunkConverter`]) so this module never depends on
//! `audio::resample` and can be built and tested independently of it.
//!
//! Safety invariant this module exists to guarantee: a failure on one track
//! (capture or write) never stops the other track, and nothing here panics
//! on a path reachable while a recording is in progress.

use crate::audio::{
    AudioChunk, AudioError, CaptureSource, ChunkConverter, ChunkSink, ErrorSink, Track, TrackWriter,
};
use chrono::{DateTime, Local};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Per-track outcome reported by [`RecordingSession::stop`].
///
/// `error` carries only the error's `Display` text (a kind, never sample
/// data or a device secret) so it can be logged or shown in the UI safely.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackReport {
    pub track: Track,
    pub sample_count: u64,
    pub error: Option<String>,
}

/// Per-track status of a session that is still running.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackLiveness {
    pub track: Track,
    /// Still capturing. False once the device died or a write failed.
    pub live: bool,
    /// Why the track stopped, when it did.
    pub error: Option<String>,
}

/// Result of ending a recording session.
#[derive(Debug, Clone, PartialEq)]
pub struct StopReport {
    pub folder: PathBuf,
    pub tracks: Vec<TrackReport>,
}

/// One track's capture source and writer, as passed to [`RecordingSession::start`].
pub type TrackSpec = (Track, Box<dyn CaptureSource>, Box<dyn TrackWriter>);

/// Shared state a track's chunk sink writes into, read back by `stop`.
struct ActiveTrack {
    track: Track,
    source: Box<dyn CaptureSource>,
    writer: Arc<Mutex<Option<Box<dyn TrackWriter>>>>,
    sample_count: Arc<AtomicU64>,
    error: Arc<Mutex<Option<AudioError>>>,
}

enum TrackSlot {
    Active(ActiveTrack),
    /// The source failed to start; nothing was ever recorded for this track.
    FailedToStart {
        track: Track,
        error: AudioError,
    },
}

/// One meeting recording: a timestamped folder plus one pipeline per track.
pub struct RecordingSession {
    folder: PathBuf,
    tracks: Vec<TrackSlot>,
    /// Cached result of the first `stop()` call, so a second call is a
    /// no-op that returns the same report instead of re-finishing writers.
    stop_report: Option<StopReport>,
}

impl RecordingSession {
    /// Starts a new recording session under `notes_root`.
    ///
    /// Creates the meeting folder (`YYYY-MM-DD-HHMM` in local time; see
    /// [`resolve_folder`] for the collision rule) and starts every capture
    /// source. A source that fails to start does **not** abort the session
    /// or the other tracks — it is recorded and surfaces in the
    /// [`StopReport`]. Only a failure to create the folder itself fails the
    /// whole call, since without it there is nowhere to write.
    pub fn start(
        notes_root: &Path,
        tracks: Vec<TrackSpec>,
        converter: ChunkConverter,
    ) -> Result<Self, AudioError> {
        Self::start_at(notes_root, tracks, converter, Local::now())
    }

    /// Same as [`Self::start`] with an injected clock, so folder naming is
    /// deterministic in tests.
    fn start_at(
        notes_root: &Path,
        tracks: Vec<TrackSpec>,
        converter: ChunkConverter,
        now: DateTime<Local>,
    ) -> Result<Self, AudioError> {
        let folder = resolve_folder(notes_root, now);
        std::fs::create_dir_all(&folder).map_err(|source| AudioError::Io {
            path: folder.display().to_string(),
            source,
        })?;

        let mut slots = Vec::with_capacity(tracks.len());
        for (track, mut source, writer) in tracks {
            let writer_slot: Arc<Mutex<Option<Box<dyn TrackWriter>>>> =
                Arc::new(Mutex::new(Some(writer)));
            let sample_count = Arc::new(AtomicU64::new(0));
            let error_slot: Arc<Mutex<Option<AudioError>>> = Arc::new(Mutex::new(None));

            let sink_writer = writer_slot.clone();
            let sink_samples = sample_count.clone();
            let sink_error = error_slot.clone();
            let sink_converter = converter.clone();
            let sink: ChunkSink = Box::new(move |chunk: AudioChunk| {
                let mut error_guard = match sink_error.lock() {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
                // Track already failed: drop further chunks instead of
                // writing past a broken pipeline.
                if error_guard.is_some() {
                    return;
                }

                let mono = sink_converter(&chunk);

                let mut writer_guard = match sink_writer.lock() {
                    Ok(guard) => guard,
                    Err(_) => return,
                };
                let write_result = match writer_guard.as_mut() {
                    Some(active_writer) => active_writer.write(&mono),
                    None => return,
                };
                match write_result {
                    Ok(()) => {
                        sink_samples.fetch_add(mono.len() as u64, Ordering::Relaxed);
                    }
                    Err(err) => {
                        // Stop this track only: finalize the writer so
                        // whatever was already flushed stays in a
                        // well-formed container, then drop it so no more
                        // chunks are written. Every other track is
                        // untouched. `stop()` sees the writer already gone
                        // and will not try to finish it again.
                        if let Some(failed_writer) = writer_guard.take() {
                            let _ = failed_writer.finish();
                        }
                        *error_guard = Some(err);
                    }
                }
            });

            // A device lost mid-recording arrives here, not through the
            // chunk path. It fails exactly one track, the same way a write
            // error does, so `stop()` and the UI report it identically.
            let fault_error = error_slot.clone();
            let fault_writer = writer_slot.clone();
            let on_error: ErrorSink = Box::new(move |err| {
                let Ok(mut error_guard) = fault_error.lock() else {
                    return;
                };
                if error_guard.is_some() {
                    return;
                }
                if let Ok(mut writer_guard) = fault_writer.lock() {
                    if let Some(failed_writer) = writer_guard.take() {
                        let _ = failed_writer.finish();
                    }
                }
                *error_guard = Some(err);
            });

            match source.start(sink, on_error) {
                Ok(()) => slots.push(TrackSlot::Active(ActiveTrack {
                    track,
                    source,
                    writer: writer_slot,
                    sample_count,
                    error: error_slot,
                })),
                Err(error) => slots.push(TrackSlot::FailedToStart { track, error }),
            }
        }

        Ok(RecordingSession {
            folder,
            tracks: slots,
            stop_report: None,
        })
    }

    /// The meeting folder this session is writing into.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Per-track status *while the session is still running*.
    ///
    /// Exists so the UI can stop showing a track as live the moment its
    /// device disappears, rather than finding out only at `stop()`.
    pub fn track_liveness(&self) -> Vec<TrackLiveness> {
        self.tracks
            .iter()
            .map(|slot| match slot {
                TrackSlot::Active(active) => {
                    let error = match active.error.lock() {
                        Ok(guard) => guard.as_ref().map(ToString::to_string),
                        // A poisoned lock means a panicking sink; report the
                        // track as dead rather than claiming it is fine.
                        Err(_) => Some("track state is unavailable".to_owned()),
                    };
                    TrackLiveness {
                        track: active.track,
                        live: error.is_none(),
                        error,
                    }
                }
                TrackSlot::FailedToStart { track, error } => TrackLiveness {
                    track: *track,
                    live: false,
                    error: Some(error.to_string()),
                },
            })
            .collect()
    }

    /// Stops every source, finishes every writer, and returns the folder
    /// path plus a per-track report. Idempotent: a second call returns the
    /// same report without touching a source or writer again.
    pub fn stop(&mut self) -> StopReport {
        if let Some(report) = &self.stop_report {
            return report.clone();
        }

        let mut track_reports = Vec::with_capacity(self.tracks.len());
        for slot in &mut self.tracks {
            match slot {
                TrackSlot::Active(active) => {
                    let stop_error = active.source.stop().err();

                    let writer = match active.writer.lock() {
                        Ok(mut guard) => guard.take(),
                        Err(_) => None,
                    };
                    let finish_error = writer.and_then(|w| w.finish().err());

                    let write_error = match active.error.lock() {
                        Ok(guard) => guard.as_ref().map(ToString::to_string),
                        Err(_) => None,
                    };

                    let error = write_error
                        .or_else(|| stop_error.map(|e| e.to_string()))
                        .or_else(|| finish_error.map(|e| e.to_string()));

                    track_reports.push(TrackReport {
                        track: active.track,
                        sample_count: active.sample_count.load(Ordering::Relaxed),
                        error,
                    });
                }
                TrackSlot::FailedToStart { track, error } => {
                    track_reports.push(TrackReport {
                        track: *track,
                        sample_count: 0,
                        error: Some(error.to_string()),
                    });
                }
            }
        }

        let report = StopReport {
            folder: self.folder.clone(),
            tracks: track_reports,
        };
        self.stop_report = Some(report.clone());
        report
    }
}

impl Drop for RecordingSession {
    /// Best-effort safety net: if the caller forgets to `stop()`, still
    /// finish writers so buffered audio is not silently lost. Errors are
    /// swallowed here — there is no caller left to hand them to.
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Meeting folder name for `now`, local time: `YYYY-MM-DD-HHMM`.
fn folder_stem(now: DateTime<Local>) -> String {
    now.format("%Y-%m-%d-%H%M").to_string()
}

/// Picks the folder this session will write into.
///
/// Starts from `notes_root/YYYY-MM-DD-HHMM`. If that path already exists
/// (two recordings started in the same minute, or a leftover folder from a
/// prior run), appends `-2`, `-3`, ... until an unused path is found. Never
/// returns a path that already exists, so a session can never overwrite
/// another meeting's files.
fn resolve_folder(notes_root: &Path, now: DateTime<Local>) -> PathBuf {
    let stem = folder_stem(now);
    let base = notes_root.join(&stem);
    if !base.exists() {
        return base;
    }
    let mut suffix = 2u32;
    loop {
        let candidate = notes_root.join(format!("{stem}-{suffix}"));
        if !candidate.exists() {
            return candidate;
        }
        suffix += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioChunk;
    use chrono::TimeZone;
    use std::sync::atomic::AtomicUsize;
    use tempfile::tempdir;

    /// A `CaptureSource` double whose sink is exposed to the test via
    /// `sink_slot`, so the test can push chunks by calling it directly —
    /// no real audio thread involved.
    struct FakeSource {
        device_name: &'static str,
        error_slot: Arc<Mutex<Option<ErrorSink>>>,
        start_error: Option<AudioError>,
        sink_slot: Arc<Mutex<Option<ChunkSink>>>,
        stop_calls: Arc<AtomicUsize>,
    }

    impl FakeSource {
        fn new(
            device_name: &'static str,
        ) -> (Self, Arc<Mutex<Option<ChunkSink>>>, Arc<AtomicUsize>) {
            let sink_slot = Arc::new(Mutex::new(None));
            let stop_calls = Arc::new(AtomicUsize::new(0));
            (
                FakeSource {
                    device_name,
                    error_slot: Arc::new(Mutex::new(None)),
                    start_error: None,
                    sink_slot: sink_slot.clone(),
                    stop_calls: stop_calls.clone(),
                },
                sink_slot,
                stop_calls,
            )
        }

        /// Like `new`, but also hands back the error sink the session
        /// installs, so a test can simulate a device dying mid-recording.
        #[allow(clippy::type_complexity)]
        fn with_error_channel(
            device_name: &'static str,
        ) -> (
            Self,
            Arc<Mutex<Option<ChunkSink>>>,
            Arc<Mutex<Option<ErrorSink>>>,
        ) {
            let sink_slot = Arc::new(Mutex::new(None));
            let error_slot = Arc::new(Mutex::new(None));
            (
                FakeSource {
                    device_name,
                    error_slot: error_slot.clone(),
                    start_error: None,
                    sink_slot: sink_slot.clone(),
                    stop_calls: Arc::new(AtomicUsize::new(0)),
                },
                sink_slot,
                error_slot,
            )
        }

        fn failing_to_start(device_name: &'static str, error: AudioError) -> Self {
            let sink_slot = Arc::new(Mutex::new(None));
            FakeSource {
                device_name,
                error_slot: Arc::new(Mutex::new(None)),
                start_error: Some(error),
                sink_slot,
                stop_calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl CaptureSource for FakeSource {
        fn start(&mut self, sink: ChunkSink, on_error: ErrorSink) -> Result<(), AudioError> {
            if let Some(error) = self.start_error.take() {
                return Err(error);
            }
            if let Ok(mut guard) = self.sink_slot.lock() {
                *guard = Some(sink);
            }
            if let Ok(mut guard) = self.error_slot.lock() {
                *guard = Some(on_error);
            }
            Ok(())
        }

        fn stop(&mut self) -> Result<(), AudioError> {
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn device_name(&self) -> String {
            self.device_name.to_string()
        }
    }

    /// Fires the error sink a started `FakeSource` was given, simulating a
    /// device that disappeared while the meeting was still running.
    fn fail_device(error_slot: &Arc<Mutex<Option<ErrorSink>>>, error: AudioError) {
        if let Ok(mut guard) = error_slot.lock() {
            if let Some(on_error) = guard.as_mut() {
                on_error(error);
            }
        }
    }

    /// Pushes a chunk through a started `FakeSource`'s sink, if any.
    fn push_chunk(sink_slot: &Arc<Mutex<Option<ChunkSink>>>, samples: Vec<f32>) {
        if let Ok(mut guard) = sink_slot.lock() {
            if let Some(sink) = guard.as_mut() {
                sink(AudioChunk {
                    samples,
                    sample_rate: 16_000,
                    channels: 1,
                });
            }
        }
    }

    /// A `TrackWriter` double that records how many samples it received and
    /// can be told to fail on a specific (1-based) write call.
    struct FakeWriter {
        call_count: Arc<AtomicUsize>,
        fail_on_call: Option<usize>,
        finished: Arc<Mutex<bool>>,
    }

    impl FakeWriter {
        fn new() -> (Self, Arc<AtomicUsize>, Arc<Mutex<bool>>) {
            let call_count = Arc::new(AtomicUsize::new(0));
            let finished = Arc::new(Mutex::new(false));
            (
                FakeWriter {
                    call_count: call_count.clone(),
                    fail_on_call: None,
                    finished: finished.clone(),
                },
                call_count,
                finished,
            )
        }

        fn failing_on_call(fail_on_call: usize) -> (Self, Arc<AtomicUsize>, Arc<Mutex<bool>>) {
            let (mut writer, call_count, finished) = Self::new();
            writer.fail_on_call = Some(fail_on_call);
            (writer, call_count, finished)
        }
    }

    impl TrackWriter for FakeWriter {
        fn write(&mut self, _samples: &[f32]) -> Result<(), AudioError> {
            let call_no = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_on_call == Some(call_no) {
                return Err(AudioError::Encode("fake write failure".into()));
            }
            Ok(())
        }

        fn finish(self: Box<Self>) -> Result<(), AudioError> {
            if let Ok(mut guard) = self.finished.lock() {
                *guard = true;
            }
            Ok(())
        }
    }

    /// A converter that counts how many times it ran and passes samples
    /// through unchanged (this packet must not depend on the real
    /// resampler).
    fn counting_converter(calls: Arc<AtomicUsize>) -> ChunkConverter {
        Arc::new(move |chunk: &AudioChunk| {
            calls.fetch_add(1, Ordering::SeqCst);
            chunk.samples.clone()
        })
    }

    fn fixed_now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 8, 19, 10, 30, 0).unwrap()
    }

    #[test]
    fn happy_path_writes_both_tracks() {
        let dir = tempdir().unwrap();
        let (mic_source, mic_sink, _) = FakeSource::new("mic");
        let (system_source, system_sink, _) = FakeSource::new("system");
        let (mic_writer, mic_calls, mic_finished) = FakeWriter::new();
        let (system_writer, system_calls, system_finished) = FakeWriter::new();
        let convert_calls = Arc::new(AtomicUsize::new(0));

        let mut session = RecordingSession::start_at(
            dir.path(),
            vec![
                (Track::Mic, Box::new(mic_source), Box::new(mic_writer)),
                (
                    Track::System,
                    Box::new(system_source),
                    Box::new(system_writer),
                ),
            ],
            counting_converter(convert_calls.clone()),
            fixed_now(),
        )
        .unwrap();

        push_chunk(&mic_sink, vec![0.1; 4]);
        push_chunk(&mic_sink, vec![0.2; 4]);
        push_chunk(&system_sink, vec![0.3; 4]);

        let report = session.stop();

        assert_eq!(mic_calls.load(Ordering::SeqCst), 2);
        assert_eq!(system_calls.load(Ordering::SeqCst), 1);
        assert!(*mic_finished.lock().unwrap());
        assert!(*system_finished.lock().unwrap());
        assert_eq!(convert_calls.load(Ordering::SeqCst), 3);

        assert_eq!(report.folder, dir.path().join("2026-08-19-1030"));
        let mic_report = report
            .tracks
            .iter()
            .find(|t| t.track == Track::Mic)
            .unwrap();
        let system_report = report
            .tracks
            .iter()
            .find(|t| t.track == Track::System)
            .unwrap();
        assert_eq!(mic_report.sample_count, 8);
        assert_eq!(mic_report.error, None);
        assert_eq!(system_report.sample_count, 4);
        assert_eq!(system_report.error, None);
    }

    #[test]
    fn writer_error_does_not_stop_the_other_track() {
        let dir = tempdir().unwrap();
        let (mic_source, mic_sink, _) = FakeSource::new("mic");
        let (system_source, system_sink, _) = FakeSource::new("system");
        let (mic_writer, mic_calls, mic_finished) = FakeWriter::failing_on_call(3);
        let (system_writer, system_calls, _) = FakeWriter::new();
        let convert_calls = Arc::new(AtomicUsize::new(0));

        let mut session = RecordingSession::start_at(
            dir.path(),
            vec![
                (Track::Mic, Box::new(mic_source), Box::new(mic_writer)),
                (
                    Track::System,
                    Box::new(system_source),
                    Box::new(system_writer),
                ),
            ],
            counting_converter(convert_calls),
            fixed_now(),
        )
        .unwrap();

        for _ in 0..5 {
            push_chunk(&mic_sink, vec![1.0; 2]);
            push_chunk(&system_sink, vec![1.0; 2]);
        }

        let report = session.stop();

        // Mic: calls 1, 2 succeed; call 3 fails and the track stops, so
        // chunks 4 and 5 are never written.
        assert_eq!(mic_calls.load(Ordering::SeqCst), 3);
        // Even though the write failed, the writer must still be finalized
        // so whatever was already flushed stays in a playable container.
        assert!(*mic_finished.lock().unwrap());
        // System keeps recording all five chunks, unaffected by the mic
        // failure.
        assert_eq!(system_calls.load(Ordering::SeqCst), 5);

        let mic_report = report
            .tracks
            .iter()
            .find(|t| t.track == Track::Mic)
            .unwrap();
        let system_report = report
            .tracks
            .iter()
            .find(|t| t.track == Track::System)
            .unwrap();
        assert_eq!(mic_report.sample_count, 4); // two successful writes of 2 samples
        assert!(mic_report.error.is_some());
        assert_eq!(system_report.sample_count, 10);
        assert_eq!(system_report.error, None);
    }

    #[test]
    fn source_start_failure_is_reported_without_aborting_session() {
        let dir = tempdir().unwrap();
        let mic_source = FakeSource::failing_to_start("mic", AudioError::NoDevice("mic"));
        let (system_source, system_sink, _) = FakeSource::new("system");
        let (mic_writer, _, _) = FakeWriter::new();
        let (system_writer, system_calls, _) = FakeWriter::new();
        let convert_calls = Arc::new(AtomicUsize::new(0));

        let mut session = RecordingSession::start_at(
            dir.path(),
            vec![
                (Track::Mic, Box::new(mic_source), Box::new(mic_writer)),
                (
                    Track::System,
                    Box::new(system_source),
                    Box::new(system_writer),
                ),
            ],
            counting_converter(convert_calls),
            fixed_now(),
        )
        .unwrap();

        push_chunk(&system_sink, vec![1.0; 4]);
        let report = session.stop();

        assert_eq!(system_calls.load(Ordering::SeqCst), 1);
        let mic_report = report
            .tracks
            .iter()
            .find(|t| t.track == Track::Mic)
            .unwrap();
        assert_eq!(mic_report.sample_count, 0);
        assert!(mic_report.error.is_some());
        let system_report = report
            .tracks
            .iter()
            .find(|t| t.track == Track::System)
            .unwrap();
        assert_eq!(system_report.error, None);
        assert_eq!(system_report.sample_count, 4);
    }

    #[test]
    fn stop_is_idempotent() {
        let dir = tempdir().unwrap();
        let (mic_source, mic_sink, mic_stop_calls) = FakeSource::new("mic");
        let (mic_writer, _, mic_finished) = FakeWriter::new();
        let convert_calls = Arc::new(AtomicUsize::new(0));

        let mut session = RecordingSession::start_at(
            dir.path(),
            vec![(Track::Mic, Box::new(mic_source), Box::new(mic_writer))],
            counting_converter(convert_calls),
            fixed_now(),
        )
        .unwrap();

        push_chunk(&mic_sink, vec![1.0; 4]);
        let first = session.stop();
        let second = session.stop();

        assert_eq!(first, second);
        assert_eq!(mic_stop_calls.load(Ordering::SeqCst), 1);
        assert!(*mic_finished.lock().unwrap());
    }

    #[test]
    fn folder_collision_gets_a_deterministic_suffix() {
        let dir = tempdir().unwrap();
        let now = fixed_now();
        let convert_calls = Arc::new(AtomicUsize::new(0));

        let (first_source, _, _) = FakeSource::new("mic");
        let (first_writer, _, _) = FakeWriter::new();
        let mut first = RecordingSession::start_at(
            dir.path(),
            vec![(Track::Mic, Box::new(first_source), Box::new(first_writer))],
            counting_converter(convert_calls.clone()),
            now,
        )
        .unwrap();
        assert_eq!(first.folder(), dir.path().join("2026-08-19-1030"));

        let (second_source, _, _) = FakeSource::new("mic");
        let (second_writer, _, _) = FakeWriter::new();
        let mut second = RecordingSession::start_at(
            dir.path(),
            vec![(Track::Mic, Box::new(second_source), Box::new(second_writer))],
            counting_converter(convert_calls.clone()),
            now,
        )
        .unwrap();
        assert_eq!(second.folder(), dir.path().join("2026-08-19-1030-2"));

        let (third_source, _, _) = FakeSource::new("mic");
        let (third_writer, _, _) = FakeWriter::new();
        let mut third = RecordingSession::start_at(
            dir.path(),
            vec![(Track::Mic, Box::new(third_source), Box::new(third_writer))],
            counting_converter(convert_calls),
            now,
        )
        .unwrap();
        assert_eq!(third.folder(), dir.path().join("2026-08-19-1030-3"));

        first.stop();
        second.stop();
        third.stop();
    }

    #[test]
    fn converter_runs_exactly_once_per_chunk() {
        let dir = tempdir().unwrap();
        let (mic_source, mic_sink, _) = FakeSource::new("mic");
        let (system_source, system_sink, _) = FakeSource::new("system");
        let (mic_writer, _, _) = FakeWriter::new();
        let (system_writer, _, _) = FakeWriter::new();
        let convert_calls = Arc::new(AtomicUsize::new(0));

        let mut session = RecordingSession::start_at(
            dir.path(),
            vec![
                (Track::Mic, Box::new(mic_source), Box::new(mic_writer)),
                (
                    Track::System,
                    Box::new(system_source),
                    Box::new(system_writer),
                ),
            ],
            counting_converter(convert_calls.clone()),
            fixed_now(),
        )
        .unwrap();

        push_chunk(&mic_sink, vec![1.0; 2]);
        push_chunk(&mic_sink, vec![1.0; 2]);
        push_chunk(&mic_sink, vec![1.0; 2]);
        push_chunk(&system_sink, vec![1.0; 2]);
        push_chunk(&system_sink, vec![1.0; 2]);

        assert_eq!(convert_calls.load(Ordering::SeqCst), 5);
        session.stop();
    }

    #[test]
    fn a_device_lost_mid_recording_stops_only_that_track() {
        let dir = tempdir().expect("temp dir");
        let (mic_source, mic_sink, mic_errors) = FakeSource::with_error_channel("mic");
        let (system_source, system_sink, _) = FakeSource::with_error_channel("system");
        let (mic_writer, mic_writes, _) = FakeWriter::new();
        let (system_writer, system_writes, _) = FakeWriter::new();

        let mut session = RecordingSession::start(
            dir.path(),
            vec![
                (Track::Mic, Box::new(mic_source), Box::new(mic_writer)),
                (
                    Track::System,
                    Box::new(system_source),
                    Box::new(system_writer),
                ),
            ],
            counting_converter(Arc::new(AtomicUsize::new(0))),
        )
        .expect("session starts");

        push_chunk(&mic_sink, vec![0.1; 4]);
        push_chunk(&system_sink, vec![0.2; 4]);

        // The microphone is unplugged.
        fail_device(&mic_errors, AudioError::Stream("device removed".to_owned()));

        // Anything the dead device somehow still delivers is dropped, while
        // the other track keeps recording normally.
        push_chunk(&mic_sink, vec![0.3; 4]);
        push_chunk(&system_sink, vec![0.4; 4]);

        assert_eq!(
            mic_writes.load(Ordering::SeqCst),
            1,
            "the lost track must stop writing at the failure"
        );
        assert_eq!(
            system_writes.load(Ordering::SeqCst),
            2,
            "the surviving track must keep recording"
        );

        let report = session.stop();
        let mic = report
            .tracks
            .iter()
            .find(|t| t.track == Track::Mic)
            .expect("mic reported");
        assert!(
            mic.error
                .as_deref()
                .is_some_and(|e| e.contains("device removed")),
            "the report must say why the track died, got {:?}",
            mic.error
        );
        let system = report
            .tracks
            .iter()
            .find(|t| t.track == Track::System)
            .expect("system reported");
        assert_eq!(system.error, None);
    }

    #[test]
    fn liveness_reports_a_dead_track_before_the_session_is_stopped() {
        let dir = tempdir().expect("temp dir");
        let (mic_source, _mic_sink, mic_errors) = FakeSource::with_error_channel("mic");
        let (system_source, _system_sink, _) = FakeSource::with_error_channel("system");
        let (mic_writer, _, _) = FakeWriter::new();
        let (system_writer, _, _) = FakeWriter::new();

        let session = RecordingSession::start(
            dir.path(),
            vec![
                (Track::Mic, Box::new(mic_source), Box::new(mic_writer)),
                (
                    Track::System,
                    Box::new(system_source),
                    Box::new(system_writer),
                ),
            ],
            counting_converter(Arc::new(AtomicUsize::new(0))),
        )
        .expect("session starts");

        assert!(
            session.track_liveness().iter().all(|t| t.live),
            "both tracks start live"
        );

        fail_device(&mic_errors, AudioError::Stream("device removed".to_owned()));

        let liveness = session.track_liveness();
        let mic = liveness
            .iter()
            .find(|t| t.track == Track::Mic)
            .expect("mic listed");
        let system = liveness
            .iter()
            .find(|t| t.track == Track::System)
            .expect("system listed");
        assert!(!mic.live, "a lost device must not still be reported live");
        assert!(mic.error.is_some());
        assert!(system.live);
    }
}
