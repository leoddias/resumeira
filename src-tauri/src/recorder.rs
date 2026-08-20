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
use std::time::{Duration, Instant};

/// How long a reported capture fault must go uncleared before the track is
/// considered dead.
///
/// Windows emits a benign underrun when a stream starts, and virtual audio
/// devices glitch under load; both are followed immediately by more audio.
/// A genuinely lost device delivers nothing, so its fault simply never
/// clears. Two seconds is long enough to ride out the startup glitch and
/// short enough that the UI stops claiming a dead microphone is live before
/// the user has said much.
const CAPTURE_FAULT_GRACE: Duration = Duration::from_secs(2);

/// A capture error that has not (yet) been contradicted by fresh audio.
struct CaptureFault {
    error: AudioError,
    seen_at: Instant,
}

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
    /// Set by a write failure. Always fatal for this track.
    error: Arc<Mutex<Option<AudioError>>>,
    /// Set by a capture failure. Fatal only once it outlives the grace
    /// window without any chunk arriving to clear it.
    capture_fault: Arc<Mutex<Option<CaptureFault>>>,
}

impl ActiveTrack {
    /// The error that has actually killed this track, if any.
    ///
    /// A write failure is fatal at once. A capture complaint is fatal only
    /// once it has survived [`CAPTURE_FAULT_GRACE`] without any chunk
    /// arriving to clear it — see the note where the error sink is built.
    fn fatal_error(&self) -> Option<String> {
        match self.error.lock() {
            // A poisoned lock means a panicking sink; report the track as
            // dead rather than claiming it is fine.
            Err(_) => return Some("track state is unavailable".to_owned()),
            Ok(guard) => {
                if let Some(error) = guard.as_ref() {
                    return Some(error.to_string());
                }
            }
        }

        match self.capture_fault.lock() {
            Err(_) => Some("track state is unavailable".to_owned()),
            Ok(guard) => guard.as_ref().and_then(|fault| {
                (fault.seen_at.elapsed() >= CAPTURE_FAULT_GRACE).then(|| fault.error.to_string())
            }),
        }
    }
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
            let capture_fault: Arc<Mutex<Option<CaptureFault>>> = Arc::new(Mutex::new(None));

            let sink_writer = writer_slot.clone();
            let sink_fault = capture_fault.clone();
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
                        // Audio is still flowing, so any earlier capture
                        // complaint was a glitch, not a lost device.
                        if let Ok(mut fault) = sink_fault.lock() {
                            *fault = None;
                        }
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

            // A device problem reported mid-recording is *not* immediately
            // fatal. Windows reliably emits one benign buffer underrun as
            // the audio client primes its ring buffer, and virtual devices
            // (SteelSeries Sonar and friends) glitch harmlessly under load.
            // Treating the first error as death would kill the microphone
            // track at the start of essentially every meeting.
            //
            // So a capture error is recorded as *pending* and the writer is
            // left alone. The next chunk that arrives clears it — a stream
            // that is still delivering audio is alive by definition. Only a
            // fault that no chunk clears within `CAPTURE_FAULT_GRACE` counts
            // as a lost device. Write failures stay immediately fatal: a
            // broken writer does not heal.
            let fault_slot = capture_fault.clone();
            let on_error: ErrorSink = Box::new(move |err| {
                if let Ok(mut guard) = fault_slot.lock() {
                    if guard.is_none() {
                        *guard = Some(CaptureFault {
                            error: err,
                            seen_at: Instant::now(),
                        });
                    }
                }
            });

            match source.start(sink, on_error) {
                Ok(()) => slots.push(TrackSlot::Active(ActiveTrack {
                    track,
                    source,
                    writer: writer_slot,
                    sample_count,
                    error: error_slot,
                    capture_fault,
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

    /// Test-only: pretend every pending capture fault happened `by` earlier,
    /// so the grace window can be exercised without sleeping.
    #[cfg(test)]
    fn age_capture_faults(&self, by: Duration) {
        for slot in &self.tracks {
            if let TrackSlot::Active(active) = slot {
                if let Ok(mut guard) = active.capture_fault.lock() {
                    if let Some(fault) = guard.as_mut() {
                        if let Some(earlier) = fault.seen_at.checked_sub(by) {
                            fault.seen_at = earlier;
                        }
                    }
                }
            }
        }
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
                    let error = active.fatal_error();
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

                    let write_error = active.fatal_error();

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
    fn a_transient_capture_glitch_does_not_kill_the_track() {
        // Windows reports a benign underrun as the audio client starts. If
        // that ended the recording, every meeting would lose its microphone
        // in the first second.
        let dir = tempdir().expect("temp dir");
        let (source, sink, errors) = FakeSource::with_error_channel("mic");
        let (writer, writes, _) = FakeWriter::new();

        let mut session = RecordingSession::start(
            dir.path(),
            vec![(Track::Mic, Box::new(source), Box::new(writer))],
            counting_converter(Arc::new(AtomicUsize::new(0))),
        )
        .expect("session starts");

        fail_device(&errors, AudioError::Stream("buffer underrun".to_owned()));
        push_chunk(&sink, vec![0.1; 4]);

        assert!(
            session.track_liveness()[0].live,
            "a glitch followed by audio must leave the track live"
        );
        assert_eq!(writes.load(Ordering::SeqCst), 1, "audio kept being written");

        // Even after the grace window, the fault is gone — the chunk cleared it.
        session.age_capture_faults(Duration::from_secs(60));
        assert!(session.track_liveness()[0].live);
        assert_eq!(session.stop().tracks[0].error, None);
    }

    #[test]
    fn a_device_that_stops_delivering_is_reported_dead_after_the_grace_window() {
        let dir = tempdir().expect("temp dir");
        let (source, _sink, errors) = FakeSource::with_error_channel("mic");
        let (writer, _, _) = FakeWriter::new();

        let mut session = RecordingSession::start(
            dir.path(),
            vec![(Track::Mic, Box::new(source), Box::new(writer))],
            counting_converter(Arc::new(AtomicUsize::new(0))),
        )
        .expect("session starts");

        fail_device(&errors, AudioError::Stream("device removed".to_owned()));

        assert!(
            session.track_liveness()[0].live,
            "inside the grace window the track is still given the benefit of the doubt"
        );

        session.age_capture_faults(CAPTURE_FAULT_GRACE);

        let liveness = session.track_liveness();
        assert!(
            !liveness[0].live,
            "a fault no audio cleared means the device is gone"
        );
        assert!(liveness[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("device removed")));

        assert!(session.stop().tracks[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("device removed")));
    }

    #[test]
    fn a_lost_device_on_one_track_leaves_the_other_recording() {
        let dir = tempdir().expect("temp dir");
        let (mic_source, _mic_sink, mic_errors) = FakeSource::with_error_channel("mic");
        let (system_source, system_sink, _) = FakeSource::with_error_channel("system");
        let (mic_writer, _, _) = FakeWriter::new();
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

        fail_device(&mic_errors, AudioError::Stream("device removed".to_owned()));
        session.age_capture_faults(CAPTURE_FAULT_GRACE);

        push_chunk(&system_sink, vec![0.2; 4]);
        push_chunk(&system_sink, vec![0.2; 4]);

        let liveness = session.track_liveness();
        let mic = liveness
            .iter()
            .find(|t| t.track == Track::Mic)
            .expect("mic");
        let system = liveness
            .iter()
            .find(|t| t.track == Track::System)
            .expect("system");
        assert!(!mic.live);
        assert!(system.live, "the surviving track must keep recording");
        assert_eq!(system_writes.load(Ordering::SeqCst), 2);

        let report = session.stop();
        let system_report = report
            .tracks
            .iter()
            .find(|t| t.track == Track::System)
            .expect("system reported");
        assert_eq!(system_report.error, None);
    }

    #[test]
    fn a_write_failure_is_fatal_immediately_with_no_grace() {
        // A broken writer does not heal, so it must not get the benefit of
        // the doubt that a capture glitch gets.
        let dir = tempdir().expect("temp dir");
        let (source, sink, _) = FakeSource::with_error_channel("mic");
        let (writer, _, finished) = FakeWriter::failing_on_call(1);

        let mut session = RecordingSession::start(
            dir.path(),
            vec![(Track::Mic, Box::new(source), Box::new(writer))],
            counting_converter(Arc::new(AtomicUsize::new(0))),
        )
        .expect("session starts");

        push_chunk(&sink, vec![0.1; 4]);

        assert!(
            !session.track_liveness()[0].live,
            "a write failure kills the track at once"
        );
        assert!(
            *finished.lock().expect("lock"),
            "the writer is finalized so the captured audio stays playable"
        );
        assert!(session.stop().tracks[0].error.is_some());
    }
}
