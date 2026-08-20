//! Audio pipeline contracts.
//!
//! Every implementation in this module tree is written against the types and
//! traits declared here, so capture, encoding and the recorder can be built
//! and tested independently. Nothing in this file talks to hardware.

pub mod capture;
pub mod encoder;
pub mod mixer;
pub mod resample;

/// The rate everything converges on before transcription: Whisper resamples
/// its input to 16 kHz mono anyway, so producing it earlier saves work and
/// keeps the stored audio small (ADR-0004).
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Which side of the meeting a stream came from.
///
/// The two are kept apart end to end — mixing at capture time would throw
/// away the only cheap "who was speaking" signal we get (ADR-0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Track {
    /// The user's microphone.
    Mic,
    /// What the machine played: everyone else on the call.
    System,
}

impl Track {
    /// File name (without extension) this track is stored under.
    pub fn file_stem(self) -> &'static str {
        match self {
            Track::Mic => "mic",
            Track::System => "system",
        }
    }

    /// Both tracks, in a stable order.
    pub fn all() -> [Track; 2] {
        [Track::Mic, Track::System]
    }
}

/// A block of interleaved `f32` samples exactly as the OS delivered it.
///
/// Capture reports whatever format the device runs at; conversion to the
/// target format happens later, in one tested place.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioChunk {
    /// Interleaved samples, nominally in `[-1.0, 1.0]`.
    pub samples: Vec<f32>,
    /// Sample rate of `samples`, in Hz.
    pub sample_rate: u32,
    /// Interleaved channel count. Never zero.
    pub channels: u16,
}

impl AudioChunk {
    /// Number of frames (samples per channel) in this chunk.
    ///
    /// Returns 0 for a malformed chunk rather than dividing by zero: a bad
    /// chunk must not be able to kill a recording in progress.
    pub fn frame_count(&self) -> usize {
        if self.channels == 0 {
            return 0;
        }
        self.samples.len() / self.channels as usize
    }

    /// How long this chunk lasts, in seconds.
    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.frame_count() as f64 / self.sample_rate as f64
    }
}

/// Anything that can go wrong in the audio pipeline.
///
/// Variants carry device names and error kinds, never sample data — an error
/// that gets logged must not be able to leak meeting content.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no {0} device available")]
    NoDevice(&'static str),

    #[error("device '{device}' rejected the requested configuration: {reason}")]
    UnsupportedConfig { device: String, reason: String },

    #[error("capture stream failed: {0}")]
    Stream(String),

    #[error("encoder failed: {0}")]
    Encode(String),

    #[error("writing '{path}' failed: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("system audio capture is not implemented on this platform yet")]
    UnsupportedPlatform,
}

/// Callback a capture source pushes chunks into.
///
/// Called on the OS audio thread: implementations must not block, allocate
/// unboundedly, or panic.
pub type ChunkSink = Box<dyn FnMut(AudioChunk) + Send + 'static>;

/// Callback a capture source reports a mid-stream failure through.
///
/// Errors that happen *before* the stream is running come back from
/// [`CaptureSource::start`]; this is the only route for a device that dies
/// while a meeting is being recorded. Without it the session would keep
/// reporting a dead track as live, which is the one thing this product must
/// never do. Called on the OS audio thread: must not block or panic.
pub type ErrorSink = Box<dyn FnMut(AudioError) + Send + 'static>;

/// A hardware (or virtual) source of audio for exactly one track.
pub trait CaptureSource: Send {
    /// Begin delivering chunks to `sink`, reporting mid-stream failures to
    /// `on_error`. Returns once the stream is running.
    fn start(&mut self, sink: ChunkSink, on_error: ErrorSink) -> Result<(), AudioError>;

    /// Stop delivering chunks. Must be safe to call more than once.
    fn stop(&mut self) -> Result<(), AudioError>;

    /// Human-readable device name, for the UI and for error messages.
    fn device_name(&self) -> String;
}

/// Converts a captured chunk into mono samples at [`TARGET_SAMPLE_RATE`].
///
/// Injected rather than called directly so the recorder can be tested without
/// the resampler and vice versa. `resample::to_target_mono` is the real one.
pub type ChunkConverter = std::sync::Arc<dyn Fn(&AudioChunk) -> Vec<f32> + Send + Sync>;

/// A sink that persists one track's audio.
///
/// Receives mono samples already at [`TARGET_SAMPLE_RATE`].
pub trait TrackWriter: Send {
    /// Append samples. Implementations flush incrementally so that a crash
    /// costs seconds of audio rather than the whole meeting.
    fn write(&mut self, samples: &[f32]) -> Result<(), AudioError>;

    /// Flush everything buffered and close the container.
    fn finish(self: Box<Self>) -> Result<(), AudioError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_file_stems_are_distinct_and_stable() {
        assert_eq!(Track::Mic.file_stem(), "mic");
        assert_eq!(Track::System.file_stem(), "system");
    }

    #[test]
    fn all_lists_both_tracks() {
        assert_eq!(Track::all(), [Track::Mic, Track::System]);
    }

    #[test]
    fn frame_count_divides_by_channels() {
        let chunk = AudioChunk {
            samples: vec![0.0; 480],
            sample_rate: 48_000,
            channels: 2,
        };
        assert_eq!(chunk.frame_count(), 240);
    }

    #[test]
    fn duration_uses_frames_not_samples() {
        let chunk = AudioChunk {
            samples: vec![0.0; 96_000],
            sample_rate: 48_000,
            channels: 2,
        };
        assert!((chunk.duration_secs() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn malformed_chunks_report_zero_instead_of_panicking() {
        let no_channels = AudioChunk {
            samples: vec![0.0; 10],
            sample_rate: 48_000,
            channels: 0,
        };
        assert_eq!(no_channels.frame_count(), 0);
        assert_eq!(no_channels.duration_secs(), 0.0);

        let no_rate = AudioChunk {
            samples: vec![0.0; 10],
            sample_rate: 0,
            channels: 1,
        };
        assert_eq!(no_rate.duration_secs(), 0.0);
    }
}
