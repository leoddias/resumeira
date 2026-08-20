//! Microphone capture via cpal's default input device.

use cpal::traits::HostTrait;

use crate::audio::{AudioError, CaptureSource, ChunkSink, ErrorSink};

use super::start_input_stream;

/// Captures the system's default input device (the microphone).
pub struct MicCapture {
    stream: Option<cpal::Stream>,
    device_name: String,
}

impl MicCapture {
    pub fn new() -> Self {
        Self {
            stream: None,
            device_name: String::from("(not started)"),
        }
    }
}

impl Default for MicCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureSource for MicCapture {
    fn start(&mut self, sink: ChunkSink, on_error: ErrorSink) -> Result<(), AudioError> {
        // Idempotent restart: drop whatever might already be running first.
        self.stop()?;

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or(AudioError::NoDevice("microphone"))?;

        let stream = start_input_stream(&device, sink, on_error, "microphone")?;
        self.device_name = device.to_string();
        self.stream = Some(stream);
        Ok(())
    }

    fn stop(&mut self) -> Result<(), AudioError> {
        // Dropping the stream stops it; `None` -> `None` is a no-op, so this
        // is safe to call any number of times, including before `start`.
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
        let mut capture = MicCapture::new();
        assert!(capture.stop().is_ok());
        assert!(capture.stop().is_ok());
    }

    #[test]
    fn device_name_reports_a_placeholder_before_start() {
        let capture = MicCapture::new();
        assert_eq!(capture.device_name(), "(not started)");
    }

    // Needs a real microphone. Run manually with:
    //   cargo test --manifest-path src-tauri/Cargo.toml -- --ignored mic::tests
    #[test]
    #[ignore = "requires a real microphone"]
    fn captures_from_the_default_device() {
        let mut capture = MicCapture::new();
        let (tx, rx) = std::sync::mpsc::channel();
        capture
            .start(
                Box::new(move |chunk| {
                    let _ = tx.send(chunk);
                }),
                Box::new(|error| panic!("stream failed during a manual run: {error}")),
            )
            .expect("a default input device should be available for a manual run");

        let chunk = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("expected at least one chunk from the microphone");
        assert!(chunk.frame_count() > 0);

        capture.stop().expect("stop should always succeed");
    }
}
