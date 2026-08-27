//! Fallback for a platform with no loopback backend.
//!
//! Windows, macOS and Linux all have one, so nothing reaches this today. It
//! exists so that adding a target does not silently produce a recorder whose
//! second track is a file full of zeros: `start` fails, the recorder reports
//! the track as failed, and the user is told before the meeting rather than
//! after it.

use crate::audio::{AudioError, CaptureSource, ChunkSink, ErrorSink};

/// Always reports [`AudioError::UnsupportedPlatform`] on `start`.
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
        let err = capture
            .start(Box::new(|_| {}), Box::new(|_| {}))
            .unwrap_err();
        assert!(matches!(err, AudioError::UnsupportedPlatform));
    }

    #[test]
    fn stop_is_always_ok_and_idempotent() {
        let mut capture = SystemCapture::new();
        assert!(capture.stop().is_ok());
        assert!(capture.stop().is_ok());
    }
}
