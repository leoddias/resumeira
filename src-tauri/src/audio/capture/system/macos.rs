//! macOS loopback capture, via ScreenCaptureKit's audio output (macOS 13.0+).
//!
//! macOS has no loopback device and no supported public API for tapping an
//! output device before macOS 14.4. What it does have, since Ventura, is
//! ScreenCaptureKit: a screen-capture stream can also deliver the system
//! audio mix. That is what this backend uses, and it is why recording a
//! meeting asks for **Screen Recording** permission — a name that has
//! nothing to do with audio and will confuse anyone who is not told, so the
//! error says exactly where to grant it.
//!
//! Two consequences worth knowing, both visible to the user:
//!
//! * The stream captures the **whole system mix**, not a chosen output
//!   device. There is nothing to select, and `device_name` says so rather
//!   than naming a device this code never asked for.
//! * A display has to be attached and a content filter built around it,
//!   because SCK has no audio-only stream. The video side is configured at
//!   the smallest size the API accepts and no screen handler is registered,
//!   so no frame is ever delivered — but a headless Mac has no shareable
//!   display and cannot record system audio this way at all.
//!
//! Audio arrives as planar (non-interleaved) `f32`, one buffer per channel.
//! [`interleave_audio_buffers`] turns that into the interleaved chunk the
//! rest of the pipeline expects, and is unit-tested on every platform.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use screencapturekit::error::SCStreamErrorCode;
use screencapturekit::prelude::*;
use screencapturekit::stream::configuration::AudioSampleRate;
use screencapturekit::stream::delegate_trait::StreamCallbacks;

use crate::audio::capture::sample::interleave_audio_buffers;
use crate::audio::{AudioChunk, AudioError, CaptureSource, ChunkSink, ErrorSink};

/// What SCK is asked to deliver. Matching the Windows backend's shape keeps
/// the chunks going through the same tested resampler.
///
/// Typed as the framework's own enum rather than a bare `i32` on purpose:
/// ScreenCaptureKit accepts 8, 16, 24 and 48 kHz and **silently substitutes
/// 48 kHz for anything else**, with no way to read back what it settled on.
/// A plain integer here would let someone write `44_100`, get 48 kHz audio
/// stamped 44100, and ship a system track that is permanently ~9% slow —
/// with nothing failing anywhere. The enum makes that unwriteable.
const CAPTURE_RATE: AudioSampleRate = AudioSampleRate::Rate48000;
const CAPTURE_CHANNELS: i32 = 2;

/// The video side, made as cheap as the API allows. No `Screen` handler is
/// registered, so these frames are never delivered — the dimensions only
/// stop the framework from allocating full-resolution surfaces for a stream
/// nobody reads.
const IDLE_VIDEO_EDGE: u32 = 2;

/// One notional frame per hour, for the same reason.
const IDLE_FRAME_SECONDS: i64 = 3_600;

/// What the UI shows for this track. Not a device name, because there is no
/// device: SCK hands over the system mix.
const DEVICE_LABEL: &str = "system audio mix (ScreenCaptureKit)";

/// Where the user grants what this backend needs. Worded as the Settings
/// pane is worded, because the user has to find it.
const SCREEN_RECORDING_GRANT: &str =
    "allow Resumeira under System Settings > Privacy & Security > Screen & System Audio Recording";

pub struct SystemCapture {
    stream: Option<SCStream>,
    /// Cleared by `stop`, so the delegate can tell a deliberate shutdown
    /// from a stream that died on its own. Without it, ending a meeting
    /// normally would report a device failure.
    running: Arc<AtomicBool>,
}

impl SystemCapture {
    pub fn new() -> Self {
        Self {
            stream: None,
            running: Arc::new(AtomicBool::new(false)),
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
        // Idempotent restart: drop whatever might already be running first.
        self.stop()?;

        // This is the call that trips the permission check, so its failure
        // is the one that has to name the setting rather than an error code.
        let content = SCShareableContent::get().map_err(start_error)?;
        let displays = content.displays();
        // Two very different causes land here: a genuinely headless Mac, and
        // a Mac whose Screen Recording permission was revoked after this
        // process started — `get()` then succeeds and simply reports there is
        // nothing to capture. The message names both rather than guessing.
        let display = displays.first().ok_or(AudioError::NoDevice(
            "a display to capture from (ScreenCaptureKit needs one even for audio alone;              on a Mac that has a screen, Screen Recording permission has been revoked)",
        ))?;

        let filter = SCContentFilter::create()
            .with_display(display)
            .with_excluding_windows(&[])
            .build();

        let config = SCStreamConfiguration::new()
            .with_captures_audio(true)
            .with_sample_rate(CAPTURE_RATE)
            .with_channel_count(CAPTURE_CHANNELS)
            // Without this, anything Resumeira itself plays would be
            // recorded as if it were the meeting.
            .with_excludes_current_process_audio(true)
            .with_width(IDLE_VIDEO_EDGE)
            .with_height(IDLE_VIDEO_EDGE)
            // Belt and braces on top of registering no screen handler: even
            // if a frame were somehow asked for, one an hour is the rate.
            .with_minimum_frame_interval(&CMTime::new(IDLE_FRAME_SECONDS, 1));

        // Note this is the rate that was *set*, not one the framework
        // confirmed — `SCStreamConfiguration::sample_rate` is a plain
        // property getter and SCK reports no negotiated value. The guard
        // against a wrong rate is the type of `CAPTURE_RATE`, not this call.
        let sample_rate = effective_sample_rate(config.sample_rate());

        // SCK hands its callbacks out to a dispatch queue and takes them as
        // `Fn`, so the `FnMut` sinks this crate uses go behind a mutex. The
        // lock is uncontended in practice: one queue, one handler.
        let sink = Arc::new(Mutex::new(sink));
        let on_error = Arc::new(Mutex::new(on_error));
        let running = Arc::new(AtomicBool::new(true));

        // A stream that dies mid-meeting has to reach the session, or the UI
        // goes on claiming this track is live (ADR-0017). SCK reports that
        // through the delegate, not through the sample handler, so the
        // stream is built with one.
        let mut stream = SCStream::new_with_delegate(
            &filter,
            &config,
            stream_delegate(Arc::clone(&on_error), Arc::clone(&running)),
        );

        // The registration result is the difference between recording a
        // meeting and recording nothing. ScreenCaptureKit can refuse
        // `addStreamOutput`, and the crate reports that only by returning
        // `None` and printing to stderr — which the app's logger does not
        // capture. Ignoring it would leave `start_capture` succeeding, the
        // recorder filing the track as live, and a 45-minute call
        // transcribed with only the user's own voice, reported as a success.
        let registered = stream.add_output_handler(
            move |sample: CMSampleBuffer, of_type: SCStreamOutputType| {
                if of_type != SCStreamOutputType::Audio {
                    return;
                }
                match chunk_from_sample(&sample, sample_rate) {
                    Ok(Some(chunk)) => {
                        if let Ok(mut sink) = sink.lock() {
                            sink(chunk);
                        }
                    }
                    // A buffer with no audio in it is not a failure; SCK
                    // delivers those around start and stop.
                    Ok(None) => {}
                    Err(err) => {
                        // One unreadable buffer costs a few milliseconds,
                        // not the meeting: logged, and the stream lives.
                        log::error!("capture[system audio]: {err}");
                    }
                }
            },
            SCStreamOutputType::Audio,
        );
        if registered.is_none() {
            return Err(AudioError::Stream(
                "system audio (ScreenCaptureKit): the stream refused an audio output handler"
                    .to_string(),
            ));
        }

        // No `Screen` handler is registered: frames are only decoded and
        // copied for output types someone asked for, and this stream wants
        // none of them.

        stream.start_capture().map_err(|err| {
            // Nothing is reported through `on_error` here — a failure to
            // start comes back from `start`, which is what the recorder acts
            // on, and sending it twice would show the user two failures for
            // one cause.
            running.store(false, Ordering::Release);
            start_error(err)
        })?;

        self.running = running;
        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        self.running.store(false, Ordering::Release);

        if let Some(stream) = self.stream.take() {
            // A failure to stop is logged, never returned: `stop` is on the
            // path that ends a meeting, and the stream is dropped either
            // way. Refusing to finish here would cost the recording.
            if let Err(err) = stream.stop_capture() {
                log::warn!("capture[system audio]: stopping the SCStream failed: {err}");
            }
        }
        Ok(())
    }

    fn device_name(&self) -> String {
        DEVICE_LABEL.to_string()
    }
}

/// Builds the delegate that carries a mid-stream death back to the session.
///
/// `running` keeps a shutdown already under way from looking like a fault:
/// the callback runs on a dispatch queue, so it can land just after `stop`
/// has been called, and putting a device failure on a meeting that ended
/// perfectly well would be a lie the user has no way to check.
fn stream_delegate(on_error: Arc<Mutex<ErrorSink>>, running: Arc<AtomicBool>) -> StreamCallbacks {
    // Only `on_error` is registered. ScreenCaptureKit reports *error* stops
    // and nothing else to a delegate — a `stop_capture` we asked for is never
    // delivered — so `on_stop` would fire only alongside this one, and
    // registering both would report a single death twice.
    StreamCallbacks::new().on_error(move |err: SCError| {
        report(&on_error, &running, mid_stream_error(&err));
    })
}

/// Hands `error` to the session, unless capture is already shutting down.
fn report(on_error: &Arc<Mutex<ErrorSink>>, running: &AtomicBool, error: AudioError) {
    if !running.load(Ordering::Acquire) {
        return;
    }
    log::error!("capture[system audio]: {error}");
    if let Ok(mut on_error) = on_error.lock() {
        on_error(error);
    }
}

/// Maps a ScreenCaptureKit failure that arrives after capture has started.
///
/// Always an [`AudioError::Stream`], never `PermissionDenied`: permission is
/// settled before the stream runs, and telling a user mid-meeting to go and
/// grant something would be advice that does not apply.
fn mid_stream_error(err: &SCError) -> AudioError {
    AudioError::Stream(format!("system audio (ScreenCaptureKit): {err}"))
}

/// Turns one ScreenCaptureKit audio sample buffer into a chunk.
///
/// `Ok(None)` means the buffer carried no audio — normal around start and
/// stop — as distinct from `Err`, which means one arrived and could not be
/// read.
fn chunk_from_sample(
    sample: &CMSampleBuffer,
    sample_rate: u32,
) -> Result<Option<AudioChunk>, AudioError> {
    if !sample.is_data_ready() {
        sample.make_data_ready().map_err(|code| {
            AudioError::Stream(format!(
                "audio sample buffer was not ready (OSStatus {code})"
            ))
        })?;
    }

    let Some(buffers) = sample.audio_buffer_list() else {
        return Ok(None);
    };

    let planes: Vec<(u32, &[u8])> = buffers
        .iter()
        .map(|buffer| (buffer.number_channels, buffer.data()))
        .collect();

    let (samples, channels) = interleave_audio_buffers(&planes);
    if samples.is_empty() {
        return Ok(None);
    }
    // Samples with no channel count is a malformed buffer, not an empty one.
    // Reporting it as "no audio" would drop real audio on the floor without
    // anyone finding out.
    if channels == 0 {
        return Err(AudioError::Stream(
            "system audio (ScreenCaptureKit): a buffer carried samples but no channel count"
                .to_string(),
        ));
    }

    Ok(Some(AudioChunk {
        samples,
        sample_rate,
        channels,
    }))
}

/// The rate to stamp on every chunk, given what the configuration holds.
///
/// This cannot detect a rate the framework refused — SCK does not report one
/// (see [`CAPTURE_RATE`]). All it does is refuse to stamp a nonsensical
/// value: a zero would turn every downstream duration into a division by
/// zero, and a negative one cannot be a rate at all.
fn effective_sample_rate(configured: i32) -> u32 {
    u32::try_from(configured)
        .ok()
        .filter(|rate| *rate > 0)
        .unwrap_or_else(|| CAPTURE_RATE.as_hz() as u32)
}

/// Maps a ScreenCaptureKit failure at start.
///
/// The distinction that matters is permission versus everything else: it is
/// the only failure the user can fix, and the setting it lives under is
/// named after screens, not audio.
fn start_error(err: SCError) -> AudioError {
    if is_permission_failure(&err) {
        return AudioError::PermissionDenied {
            what: "recording system audio",
            grant: SCREEN_RECORDING_GRANT,
        };
    }
    AudioError::Stream(format!("system audio (ScreenCaptureKit): {err}"))
}

/// Whether a ScreenCaptureKit error means "the user has not allowed this".
///
/// Three shapes mean it. `NoShareableContent` is included because that is
/// what an ungranted app actually sees: the framework does not refuse, it
/// reports that there is nothing on this Mac to capture.
fn is_permission_failure(err: &SCError) -> bool {
    matches!(err, SCError::PermissionDenied(_))
        || matches!(err, SCError::NoShareableContent(_))
        || matches!(
            err,
            SCError::SCStreamError {
                code: SCStreamErrorCode::UserDeclined,
                ..
            }
        )
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
    fn device_name_describes_the_mix_not_a_device() {
        let capture = SystemCapture::new();
        assert_eq!(capture.device_name(), DEVICE_LABEL);
    }

    #[test]
    fn a_declined_permission_names_the_setting_to_open() {
        let err = start_error(SCError::PermissionDenied("denied".to_string()));
        let AudioError::PermissionDenied { what, grant } = err else {
            panic!("expected AudioError::PermissionDenied, got {err:?}");
        };
        assert!(what.contains("system audio"));
        assert!(grant.contains("Screen & System Audio Recording"));
    }

    /// The failure an ungranted app actually gets. Reporting it as a generic
    /// stream error would leave the user with nothing to act on.
    #[test]
    fn no_shareable_content_is_treated_as_a_permission_failure() {
        assert!(matches!(
            start_error(SCError::NoShareableContent("none".to_string())),
            AudioError::PermissionDenied { .. }
        ));
    }

    #[test]
    fn the_user_declining_the_prompt_is_a_permission_failure() {
        let err = SCError::from_stream_error_code(SCStreamErrorCode::UserDeclined);
        assert!(matches!(
            start_error(err),
            AudioError::PermissionDenied { .. }
        ));
    }

    #[test]
    fn other_failures_stay_stream_errors_and_name_no_setting() {
        let err = start_error(SCError::StreamError("the display went away".to_string()));
        let AudioError::Stream(msg) = err else {
            panic!("expected AudioError::Stream");
        };
        assert!(msg.contains("ScreenCaptureKit"));
        assert!(!msg.contains("System Settings"));
    }

    /// A stream that dies mid-meeting has to reach the session, and must not
    /// be dressed up as a permission problem the user could act on.
    #[test]
    fn a_mid_stream_death_is_a_stream_error_not_a_permission_prompt() {
        let AudioError::Stream(msg) =
            mid_stream_error(&SCError::StreamError("the display went away".to_string()))
        else {
            panic!("expected AudioError::Stream");
        };
        assert!(msg.contains("the display went away"));
    }

    #[test]
    fn a_rate_the_framework_refused_falls_back_instead_of_stamping_zero() {
        assert_eq!(effective_sample_rate(48_000), 48_000);
        assert_eq!(effective_sample_rate(16_000), 16_000);
        assert_eq!(effective_sample_rate(0), CAPTURE_RATE as u32);
        assert_eq!(effective_sample_rate(-1), CAPTURE_RATE as u32);
    }

    // Needs a real Mac with Screen Recording granted and something audible
    // playing. Run manually with:
    //   cargo test --manifest-path src-tauri/Cargo.toml -- --ignored system::macos
    #[test]
    #[ignore = "requires a Mac with Screen Recording permission granted"]
    fn captures_the_system_mix() {
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
            .expect("Screen Recording permission should be granted for a manual run");

        let chunk = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("expected at least one chunk from the system mix");
        assert!(chunk.frame_count() > 0);
        assert_eq!(chunk.sample_rate, CAPTURE_RATE as u32);

        capture.stop().expect("stop should always succeed");
    }
}
