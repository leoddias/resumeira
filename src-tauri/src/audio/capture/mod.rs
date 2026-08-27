//! Platform capture sources: `mic` (default input) and `system` (loopback).
//!
//! Both sources are built on cpal and share the format-conversion helpers
//! below, so device handling is unit-testable without ever opening a real
//! stream. See `mic` and `system` for the platform-specific device and
//! config selection.
//!
//! Neither source asks the device to produce [`TARGET_SAMPLE_RATE`]: WASAPI
//! shared mode does not negotiate a format, it delivers the device's mix
//! format regardless of what is requested, so forcing a rate here causes
//! buffer underruns instead of resampling. Each caller passes in the
//! device's own default config (`default_input_config` for the microphone,
//! `default_output_config` for loopback, since WASAPI loopback captures in
//! the render format) and [`crate::audio::resample::to_target_mono`]
//! converts it downstream.

pub mod mic;
pub mod system;

mod sample;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Data, Device, InputCallbackInfo, Stream, SupportedStreamConfig};

use crate::audio::{AudioChunk, AudioError, ChunkSink, ErrorSink};

/// Builds and starts an input stream on `device` using `config` — the
/// device's own native format, never a forced rate — converting every
/// delivered buffer into an [`AudioChunk`] and handing it to `sink`.
///
/// `kind` names the source in error and log messages (e.g. `"microphone"`,
/// `"system audio"`). Log lines carry device names and error kinds only,
/// never sample data.
fn start_input_stream(
    device: &Device,
    config: SupportedStreamConfig,
    sink: ChunkSink,
    on_error: ErrorSink,
    kind: &'static str,
) -> Result<Stream, AudioError> {
    let device_label = device.to_string();

    let sample_format = config.sample_format();
    let channels = config.channels();
    let sample_rate = config.sample_rate();
    let stream_config = config.config();

    let mut sink = sink;
    let conversion_label = device_label.clone();
    let data_callback = move |data: &Data, _: &InputCallbackInfo| {
        match sample::convert_chunk(data, &conversion_label) {
            Ok(samples) => sink(AudioChunk {
                samples,
                sample_rate,
                channels,
            }),
            // The chunk is dropped, not the recording: one bad buffer must
            // never bring down the stream.
            Err(err) => log::error!("capture[{kind}]: {err}"),
        }
    };

    let error_label = device_label.clone();
    let mut on_error = on_error;
    let error_callback = move |err: cpal::Error| {
        // A device lost mid-recording is reported to the session, not just
        // logged: the UI must stop claiming this track is live.
        let mapped = stream_error(kind, &error_label, err);
        log::error!("capture[{kind}]: {mapped}");
        on_error(mapped);
    };

    let stream = device
        .build_input_stream_raw(
            stream_config,
            sample_format,
            data_callback,
            error_callback,
            None,
        )
        .map_err(|e| stream_error(kind, &device_label, e))?;

    stream
        .play()
        .map_err(|e| stream_error(kind, &device_label, e))?;

    Ok(stream)
}

/// Maps a cpal error into this crate's [`AudioError::Stream`], with a
/// message that names the source and device but never sample data.
fn stream_error(kind: &'static str, device: &str, err: cpal::Error) -> AudioError {
    AudioError::Stream(format!("{kind} device '{device}': {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_error_names_kind_and_device_without_leaking_samples() {
        let err = cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable);
        let mapped = stream_error("microphone", "Headset Mic", err);
        match mapped {
            AudioError::Stream(msg) => {
                assert!(msg.contains("microphone"));
                assert!(msg.contains("Headset Mic"));
            }
            other => panic!("expected AudioError::Stream, got {other:?}"),
        }
    }

    #[test]
    fn stream_error_preserves_the_underlying_error_kind_text() {
        let err = cpal::Error::new(cpal::ErrorKind::DeviceBusy);
        let mapped = stream_error("system audio", "Speakers", err);
        let AudioError::Stream(msg) = mapped else {
            panic!("expected AudioError::Stream");
        };
        assert!(msg.to_lowercase().contains("busy"));
    }
}
