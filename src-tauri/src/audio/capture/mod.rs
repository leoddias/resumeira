//! Platform capture sources: `mic` (default input) and `system` (loopback).
//!
//! Both sources are built on cpal and share the format-conversion and
//! config-selection helpers below, so device handling is unit-testable
//! without ever opening a real stream. See `mic` and `system` for the
//! platform-specific device selection.

pub mod mic;
pub mod system;

mod config;
mod sample;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Data, Device, InputCallbackInfo, Stream};

use crate::audio::{AudioChunk, AudioError, ChunkSink, TARGET_SAMPLE_RATE};

/// Builds and starts an input stream on `device`, converting every delivered
/// buffer into an [`AudioChunk`] and handing it to `sink`.
///
/// `kind` names the source in error and log messages (e.g. `"microphone"`,
/// `"system audio"`). Log lines carry device names and error kinds only,
/// never sample data.
fn start_input_stream(
    device: &Device,
    sink: ChunkSink,
    kind: &'static str,
) -> Result<Stream, AudioError> {
    let device_label = device.to_string();

    let ranges = device
        .supported_input_configs()
        .map_err(|e| AudioError::UnsupportedConfig {
            device: device_label.clone(),
            reason: e.to_string(),
        })?;

    let config = config::select_config(ranges, TARGET_SAMPLE_RATE).ok_or_else(|| {
        AudioError::UnsupportedConfig {
            device: device_label.clone(),
            reason: "device offers no usable input configuration".to_string(),
        }
    })?;

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
    let error_callback = move |err: cpal::Error| {
        // No channel back to the caller exists once the stream is running,
        // so a mid-recording device failure is logged, not propagated.
        log::error!("capture[{kind}]: {}", stream_error(kind, &error_label, err));
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
