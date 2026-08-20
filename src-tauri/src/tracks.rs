//! The real [`TrackFactory`]: platform capture paired with Opus writers.
//!
//! Kept apart from [`crate::session`] so the state machine stays testable
//! without hardware — this is the only place the two concrete halves meet.

use crate::audio::capture::{mic::MicCapture, system::SystemCapture};
use crate::audio::encoder::OpusTrackWriter;
use crate::audio::{resample, AudioChunk, AudioError, CaptureSource, ChunkConverter, Track};
use crate::recorder::TrackSpec;
use crate::session::TrackFactory;
use cpal::traits::{DeviceTrait, HostTrait};
use std::path::Path;
use std::sync::Arc;

/// Builds a microphone track and a system-audio track in the meeting folder.
pub struct LiveTrackFactory;

impl TrackFactory for LiveTrackFactory {
    fn build(&self, folder: &Path) -> Result<Vec<(TrackSpec, String)>, AudioError> {
        let mut tracks = Vec::with_capacity(2);

        for (track, source, device_name) in [
            (
                Track::Mic,
                Box::new(MicCapture::new()) as Box<dyn CaptureSource>,
                default_input_name(),
            ),
            (
                Track::System,
                Box::new(SystemCapture::new()) as Box<dyn CaptureSource>,
                default_output_name(),
            ),
        ] {
            let path = folder.join(format!("{}.opus", track.file_stem()));
            match OpusTrackWriter::create(&path) {
                Ok(writer) => tracks.push(((track, source, Box::new(writer) as _), device_name)),
                Err(error) => {
                    // One track that cannot be written must not cost the
                    // other one: a machine with no loopback device still
                    // records the microphone.
                    log::error!("cannot record {track:?}: {error}");
                }
            }
        }

        Ok(tracks)
    }

    fn converter(&self) -> ChunkConverter {
        Arc::new(|chunk: &AudioChunk| resample::to_target_mono(chunk))
    }
}

/// Name of the default input device, or a readable stand-in.
///
/// Falls back rather than failing: a missing name is a cosmetic problem, and
/// the capture attempt itself reports a genuinely absent device.
fn default_input_name() -> String {
    cpal::default_host()
        .default_input_device()
        .and_then(|device| device.description().ok())
        .map(|description| description.name().to_owned())
        .unwrap_or_else(|| fallback_name(Track::Mic))
}

/// Name of the default output device (what loopback captures from).
fn default_output_name() -> String {
    cpal::default_host()
        .default_output_device()
        .and_then(|device| device.description().ok())
        .map(|description| description.name().to_owned())
        .unwrap_or_else(|| fallback_name(Track::System))
}

fn fallback_name(track: Track) -> String {
    match track {
        Track::Mic => "Default microphone".to_owned(),
        Track::System => "System audio".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::TARGET_SAMPLE_RATE;

    #[test]
    fn fallback_names_are_human_readable_not_debug_output() {
        assert_eq!(fallback_name(Track::Mic), "Default microphone");
        assert_eq!(fallback_name(Track::System), "System audio");
    }

    #[test]
    fn the_converter_produces_target_format_mono() {
        let converter = LiveTrackFactory.converter();
        let chunk = AudioChunk {
            samples: vec![0.5; 9_600],
            sample_rate: 48_000,
            channels: 2,
        };
        let mono = converter(&chunk);
        // 9600 interleaved stereo samples = 4800 frames at 48 kHz = 0.1 s,
        // which is 1600 samples once resampled to 16 kHz mono.
        assert_eq!(mono.len(), (TARGET_SAMPLE_RATE / 10) as usize);
    }

    #[test]
    fn track_files_are_named_after_their_track() {
        let folder = Path::new("meeting");
        assert_eq!(
            folder.join(format!("{}.opus", Track::Mic.file_stem())),
            Path::new("meeting").join("mic.opus")
        );
        assert_eq!(
            folder.join(format!("{}.opus", Track::System.file_stem())),
            Path::new("meeting").join("system.opus")
        );
    }

    #[test]
    fn a_folder_that_cannot_be_written_yields_no_tracks_instead_of_panicking() {
        // A path that is not a directory: every writer creation fails, and
        // the factory must report an empty track list rather than unwinding.
        let missing = Path::new("does-not-exist-\u{1F4A5}").join("nested");
        let tracks = LiveTrackFactory
            .build(&missing)
            .expect("build must not fail");
        assert!(tracks.is_empty());
    }
}
