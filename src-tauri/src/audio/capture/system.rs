//! System (loopback) capture: whatever the machine is currently playing.
//!
//! On Windows, cpal's WASAPI backend transparently switches an *output*
//! device into loopback mode when it is opened for *input* (see
//! `cpal-0.18.2/src/host/wasapi/mod.rs`, the doc comment on `Host`, and
//! <https://docs.microsoft.com/windows/win32/coreaudio/loopback-recording>).
//! Building an input stream on the default *output* device therefore
//! captures system audio with no extra dependency.
//!
//! Other platforms don't get this for free from cpal yet, so this module is
//! a stub there that reports `AudioError::UnsupportedPlatform`.

#[cfg(target_os = "windows")]
mod windows_impl {
    use cpal::traits::HostTrait;

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

            // Opening an *input* stream on an *output* device is exactly the
            // WASAPI loopback trick described in the module doc comment.
            let stream = start_input_stream(&device, sink, on_error, "system audio")?;
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
                    Box::new(|error| panic!("stream failed during a manual run: {error}")),
                )
                .expect("a default output device should be available for a manual run");

            let chunk = rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("expected at least one chunk from loopback capture");
            assert!(chunk.frame_count() > 0);

            capture.stop().expect("stop should always succeed");
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows_impl::SystemCapture;

#[cfg(not(target_os = "windows"))]
mod stub {
    use crate::audio::{AudioError, CaptureSource, ChunkSink, ErrorSink};

    /// Stub for platforms where cpal has no loopback trick (yet). Always
    /// reports [`AudioError::UnsupportedPlatform`] on `start`.
    #[derive(Default)]
    pub struct SystemCapture;

    impl SystemCapture {
        pub fn new() -> Self {
            Self
        }
    }

    impl CaptureSource for SystemCapture {
        fn start(&mut self, _sink: ChunkSink, _on_error: ErrorSink) -> Result<(), AudioError> {
            Err(AudioError::UnsupportedPlatform)
        }

        fn stop(&mut self) -> Result<(), AudioError> {
            Ok(())
        }

        fn device_name(&self) -> String {
            "system audio (unsupported on this platform)".to_string()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn start_reports_unsupported_platform() {
            let mut capture = SystemCapture::new();
            let err = capture.start(Box::new(|_| {})).unwrap_err();
            assert!(matches!(err, AudioError::UnsupportedPlatform));
        }

        #[test]
        fn stop_is_always_ok_and_idempotent() {
            let mut capture = SystemCapture::new();
            assert!(capture.stop().is_ok());
            assert!(capture.stop().is_ok());
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub use stub::SystemCapture;
