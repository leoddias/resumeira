//! Linux loopback capture, from the default sink's PulseAudio monitor source.
//!
//! Every PulseAudio sink has a *monitor source* carrying exactly what that
//! sink is playing, and the server resolves the special device name
//! `@DEFAULT_MONITOR@` to the monitor of whichever sink is default. That is
//! the whole mechanism: no mixing, no virtual cable to install, no
//! permission prompt.
//!
//! This is written against PulseAudio rather than PipeWire's native API on
//! purpose. PipeWire ships `pipewire-pulse`, which serves this same protocol
//! and this same `@DEFAULT_MONITOR@` name, so one implementation covers both
//! — including the years of distributions that have not finished moving. A
//! machine running bare ALSA has no monitor source at all and reports
//! [`AudioError::NoDevice`] rather than recording silence.
//!
//! `pa_simple_read` blocks, so capture runs on its own thread. It is not a
//! realtime callback like cpal's, which makes the shutdown handshake below
//! the only genuinely delicate part of this file.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use libpulse_binding::def::BufferAttr;
use libpulse_binding::error::PAErr;
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction;
use libpulse_simple_binding::Simple;

use crate::audio::capture::sample::f32le_to_samples;
use crate::audio::{AudioChunk, AudioError, CaptureSource, ChunkSink, ErrorSink};

/// PulseAudio's own name for "the monitor of whatever sink is default". It
/// is resolved by the server at connect time, so the stream follows the
/// user's default-output choice as it was when the meeting started.
const DEFAULT_MONITOR: &str = "@DEFAULT_MONITOR@";

/// What the server is asked to deliver. The monitor's native format may
/// differ and PulseAudio converts; 48 kHz stereo `f32` is asked for so the
/// chunks reaching the recorder look like the ones the Windows backend
/// produces, and go through the same tested resampler.
const CAPTURE_RATE: u32 = 48_000;
const CAPTURE_CHANNELS: u8 = 2;

/// One read, in frames — 20 ms. This is also the resolution of `stop`: a
/// read already in flight has to finish before the worker notices it should
/// leave, so a smaller buffer means a more responsive shutdown and more
/// syscalls. 20 ms is short enough that nobody sees it.
const FRAMES_PER_READ: usize = CAPTURE_RATE as usize / 50;
const BYTES_PER_READ: usize = FRAMES_PER_READ * CAPTURE_CHANNELS as usize * 4;

/// How long `stop` waits for the capture thread to acknowledge before giving
/// up on it. Only reachable if a read is blocked with no audio to return,
/// which recording from a monitor is specifically supposed to prevent.
const STOP_TIMEOUT: Duration = Duration::from_millis(750);

/// What the UI shows for this track. Deliberately not the name of a sink:
/// `@DEFAULT_MONITOR@` is resolved by the server, and naming a device this
/// code never asked for would be a guess presented as a fact.
const DEVICE_LABEL: &str = "default output monitor (PulseAudio)";

pub struct SystemCapture {
    /// Cleared by `stop`; read by the worker before every delivery, so no
    /// chunk is handed to the recorder after the recorder asked it to stop.
    running: Arc<AtomicBool>,
    /// The worker signals here as it leaves, so `stop` can wait for it
    /// without an unbounded `join` on a thread parked inside a blocking read.
    exited: Option<Receiver<()>>,
    worker: Option<JoinHandle<()>>,
}

impl SystemCapture {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            exited: None,
            worker: None,
        }
    }
}

impl Default for SystemCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureSource for SystemCapture {
    fn start(&mut self, sink: ChunkSink, on_error: ErrorSink) -> Result<(), AudioError> {
        // Idempotent restart: tear down whatever might already be running.
        self.stop()?;

        let spec = Spec {
            format: Format::F32le,
            channels: CAPTURE_CHANNELS,
            rate: CAPTURE_RATE,
        };
        // A spec the server would reject produces a confusing failure much
        // later; the binding can check it up front.
        if !spec.is_valid() {
            return Err(AudioError::UnsupportedConfig {
                device: DEVICE_LABEL.to_string(),
                reason: format!(
                    "{CAPTURE_RATE} Hz / {CAPTURE_CHANNELS} channel f32 is not a valid PulseAudio spec"
                ),
            });
        }

        // Only `fragsize` matters for a record stream; the rest take the
        // server's defaults. Bounding it keeps `stop` responsive.
        let attr = BufferAttr {
            maxlength: u32::MAX,
            tlength: u32::MAX,
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: BYTES_PER_READ as u32,
        };

        // Connecting here rather than on the worker thread is what lets a
        // machine with no sound server fail out of `start`, where the
        // recorder can report the track as failed before the meeting begins
        // — instead of "succeeding" and producing an empty track.
        let stream = Simple::new(
            None,
            "Resumeira",
            Direction::Record,
            Some(DEFAULT_MONITOR),
            "meeting system audio",
            &spec,
            None,
            Some(&attr),
        )
        .map_err(connect_error)?;

        let running = Arc::new(AtomicBool::new(true));
        let (exit_tx, exit_rx) = sync_channel::<()>(1);

        let worker_running = Arc::clone(&running);
        let worker = std::thread::Builder::new()
            .name("resumeira-system-audio".to_owned())
            .spawn(move || capture_loop(&stream, &worker_running, sink, on_error, exit_tx))
            .map_err(|err| {
                AudioError::Stream(format!("could not start the system audio thread: {err}"))
            })?;

        self.running = running;
        self.exited = Some(exit_rx);
        self.worker = Some(worker);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        self.running.store(false, Ordering::Release);

        if let Some(exited) = self.exited.take() {
            // A plain `join` would block forever on a read that never
            // returns. Waiting on the worker's own goodbye, with a deadline,
            // cannot: in the worst case the thread is left to finish its
            // read and exit on its own, having already been silenced by the
            // flag above. `stop` must always return — it is on the path that
            // ends a meeting.
            if exited.recv_timeout(STOP_TIMEOUT).is_err() {
                log::warn!(
                    "capture[system audio]: the PulseAudio thread did not acknowledge stop \
                     within {STOP_TIMEOUT:?}; it will exit when its read returns"
                );
                self.worker = None;
                return Ok(());
            }
        }

        if let Some(worker) = self.worker.take() {
            // Acknowledged already, so this join is immediate.
            let _ = worker.join();
        }
        Ok(())
    }

    fn device_name(&self) -> String {
        DEVICE_LABEL.to_string()
    }
}

/// Reads until told to stop or until the server hangs up.
fn capture_loop(
    stream: &Simple,
    running: &AtomicBool,
    mut sink: ChunkSink,
    mut on_error: ErrorSink,
    exited: SyncSender<()>,
) {
    let mut buffer = vec![0u8; BYTES_PER_READ];

    while running.load(Ordering::Acquire) {
        match stream.read(&mut buffer) {
            Ok(()) => {
                // Checked again after the read: `stop` may have been called
                // while this one was in flight, and a chunk delivered after
                // that would reach a recorder already closing its files.
                // Losing the last 20 ms of a meeting is not a real cost.
                if !running.load(Ordering::Acquire) {
                    break;
                }
                sink(AudioChunk {
                    samples: f32le_to_samples(&buffer),
                    sample_rate: CAPTURE_RATE,
                    channels: u16::from(CAPTURE_CHANNELS),
                });
            }
            Err(err) => {
                // A failure during a deliberate shutdown is the shutdown, not
                // a fault, and must not be reported as a dead device.
                if running.load(Ordering::Acquire) {
                    let mapped = read_error(err);
                    log::error!("capture[system audio]: {mapped}");
                    on_error(mapped);
                }
                break;
            }
        }
    }

    // Always sent, on every exit path, or `stop` waits out its whole timeout.
    let _ = exited.send(());
}

/// Maps a failure to connect to the monitor source.
///
/// The overwhelmingly common cause is that there is no PulseAudio or
/// PipeWire to connect to, which is a missing device from the user's point
/// of view, not a mysterious stream error — so it is reported as one.
fn connect_error(err: PAErr) -> AudioError {
    // The server's own message is logged rather than carried: `NoDevice`
    // holds a `&'static str`, and every failure here means one thing to the
    // user — there is no sound server here to record from.
    log::error!(
        "capture[system audio]: connecting to {DEFAULT_MONITOR} failed: {}",
        describe(err)
    );
    AudioError::NoDevice("system output monitor")
}

/// PulseAudio's human-readable text for an error, or its raw code when the
/// library has none. Never contains sample data.
fn describe(err: PAErr) -> String {
    err.to_string()
        .unwrap_or_else(|| format!("PulseAudio error {}", err.0))
}

/// Maps a mid-stream read failure. Names the source and the PulseAudio
/// error, never sample data.
fn read_error(err: PAErr) -> AudioError {
    AudioError::Stream(format!(
        "system audio monitor '{DEFAULT_MONITOR}': {}",
        describe(err)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_before_start_is_a_noop() {
        let mut capture = SystemCapture::new();
        assert!(capture.stop().is_ok());
        assert!(capture.stop().is_ok());
    }

    #[test]
    fn device_name_is_stable_before_and_after_a_failed_start() {
        let capture = SystemCapture::new();
        assert_eq!(capture.device_name(), DEVICE_LABEL);
    }

    #[test]
    fn a_read_buffer_is_a_whole_number_of_frames() {
        assert_eq!(BYTES_PER_READ % (CAPTURE_CHANNELS as usize * 4), 0);
        assert_eq!(
            BYTES_PER_READ / (CAPTURE_CHANNELS as usize * 4),
            FRAMES_PER_READ
        );
    }

    #[test]
    fn the_requested_spec_is_one_pulseaudio_accepts() {
        let spec = Spec {
            format: Format::F32le,
            channels: CAPTURE_CHANNELS,
            rate: CAPTURE_RATE,
        };
        assert!(spec.is_valid());
    }

    #[test]
    fn a_read_error_names_the_source_without_leaking_samples() {
        let AudioError::Stream(msg) = read_error(PAErr(1)) else {
            panic!("expected AudioError::Stream");
        };
        assert!(msg.contains(DEFAULT_MONITOR));
    }

    #[test]
    fn a_failure_to_connect_is_reported_as_a_missing_device() {
        assert!(matches!(
            connect_error(PAErr(1)),
            AudioError::NoDevice("system output monitor")
        ));
    }

    // Needs a real PulseAudio or PipeWire session with something audible
    // playing. Run manually with:
    //   cargo test --manifest-path src-tauri/Cargo.toml -- --ignored system::linux
    #[test]
    #[ignore = "requires a running PulseAudio/PipeWire session"]
    fn captures_the_default_sink_monitor() {
        let mut capture = SystemCapture::new();
        let (tx, rx) = std::sync::mpsc::channel();
        capture
            .start(
                Box::new(move |chunk| {
                    let _ = tx.send(chunk);
                }),
                Box::new(|error| {
                    eprintln!("capture reported an error during a manual run: {error}");
                }),
            )
            .expect("a sound server should be running for a manual run");

        let chunk = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("expected at least one chunk from the monitor source");
        assert!(chunk.frame_count() > 0);
        assert_eq!(chunk.sample_rate, CAPTURE_RATE);

        capture.stop().expect("stop should always succeed");
    }

    /// The shutdown handshake is the part that can hang the app, so it is
    /// exercised against the real server rather than only reasoned about.
    #[test]
    #[ignore = "requires a running PulseAudio/PipeWire session"]
    fn stop_returns_promptly_and_delivers_nothing_afterwards() {
        use std::sync::atomic::AtomicUsize;

        let delivered = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&delivered);

        let mut capture = SystemCapture::new();
        capture
            .start(
                Box::new(move |_| {
                    counter.fetch_add(1, Ordering::Release);
                }),
                Box::new(|_| {}),
            )
            .expect("a sound server should be running for a manual run");

        std::thread::sleep(Duration::from_millis(200));

        let before_stop = std::time::Instant::now();
        capture.stop().expect("stop should always succeed");
        assert!(before_stop.elapsed() < STOP_TIMEOUT + Duration::from_millis(250));

        let after_stop = delivered.load(Ordering::Acquire);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            delivered.load(Ordering::Acquire),
            after_stop,
            "a chunk was delivered after stop returned"
        );
    }
}
