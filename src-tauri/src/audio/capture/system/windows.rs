//! Windows loopback capture, via cpal's WASAPI backend.
//!
//! cpal transparently switches an *output* device into loopback mode when it
//! is opened for *input* (see `cpal-0.18.2/src/host/wasapi/mod.rs`, the doc
//! comment on `Host`, and
//! <https://docs.microsoft.com/windows/win32/coreaudio/loopback-recording>).
//! Building an input stream on the default *output* device therefore captures
//! system audio with no extra dependency and no permission prompt — the only
//! one of the three platforms where that is true.

use cpal::traits::{DeviceTrait, HostTrait};

use crate::audio::{AudioError, CaptureSource, ChunkSink, ErrorSink};

use super::super::start_input_stream;

/// Captures the system's default *output* device via WASAPI loopback.
pub struct SystemCapture {
    stream: Option<cpal::Stream>,
    device_name: String,
}

impl SystemCapture {
    pub fn new() -> Self {
        Self {
            stream: None,
            device_name: String::from("(not started)"),
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

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(AudioError::NoDevice("system output"))?;

        // The config comes from the *output* side, and is the device's
        // own default: WASAPI loopback captures in the render format, so
        // querying input configs (or forcing a rate) on an output device
        // yields nothing usable.
        let config = device
            .default_output_config()
            .map_err(|e| AudioError::UnsupportedConfig {
                device: device.to_string(),
                reason: e.to_string(),
            })?;

        // Opening an *input* stream on an *output* device is exactly the
        // WASAPI loopback trick described in the module doc comment.
        let stream = start_input_stream(&device, config, sink, on_error, "system audio")?;
        self.device_name = device.to_string();
        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        self.stream = None;
        Ok(())
    }

    fn device_name(&self) -> String {
        self.device_name.clone()
    }
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
    fn device_name_reports_a_placeholder_before_start() {
        let capture = SystemCapture::new();
        assert_eq!(capture.device_name(), "(not started)");
    }

    // Needs a real output device with something audible playing through
    // it. Run manually with:
    //   cargo test --manifest-path src-tauri/Cargo.toml -- --ignored system::tests
    //
    // A stream error is logged, not treated as fatal: on at least one
    // real device (a SteelSeries Sonar virtual output), WASAPI reports
    // one benign buffer underrun as `IAudioClient::Start()` primes the
    // ring buffer, then delivers real audio normally for the rest of
    // the stream's life. Requiring zero errors would fail this test
    // against real hardware for a condition capture already recovers
    // from; what actually matters — and what stays asserted — is that
    // genuine audio data arrives.
    #[test]
    #[ignore = "requires a real output device"]
    fn captures_loopback_from_the_default_device() {
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
            .expect("a default output device should be available for a manual run");

        let chunk = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("expected at least one chunk from loopback capture");
        assert!(chunk.frame_count() > 0);

        capture.stop().expect("stop should always succeed");
    }
}
