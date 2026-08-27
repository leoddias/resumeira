//! Ogg/Opus decoder.
//!
//! Reads a track written by [`crate::audio::encoder::OpusTrackWriter`] back
//! into mono `f32` samples at [`TARGET_SAMPLE_RATE`], so local transcription
//! has PCM to work with. Robust to a file that ends mid-recording: a
//! truncated or corrupt Ogg stream returns whatever decoded successfully
//! instead of failing outright, because a crash mid-meeting must never make
//! the recording unrecoverable.

use std::fs::File;
use std::path::Path;

use ogg::reading::PacketReader;
use opus::{Channels, Decoder as OpusDecoder};

use crate::audio::{AudioError, TARGET_SAMPLE_RATE};

/// Ogg Opus granule positions are always expressed in samples at 48 kHz,
/// regardless of the decoder's actual output rate (RFC 7845 section 4).
const GRANULE_RATE_HZ: u64 = 48_000;

/// Length of the `OpusHead` packet's fixed-size fields we read (RFC 7845
/// section 5.1). The packet may be longer for channel mapping family != 0,
/// but our encoder never writes that, so anything shorter is malformed.
const OPUS_HEAD_MIN_LEN: usize = 19;

fn map_io_err(path: &Path, err: std::io::Error) -> AudioError {
    AudioError::Io {
        path: path.display().to_string(),
        source: err,
    }
}

/// Parsed fields of an `OpusHead` packet that matter for decoding.
struct OpusHead {
    channels: u8,
    pre_skip: u16,
}

/// Parses the `OpusHead` packet (RFC 7845 section 5.1).
fn parse_opus_head(data: &[u8]) -> Result<OpusHead, AudioError> {
    if data.len() < OPUS_HEAD_MIN_LEN || &data[0..8] != b"OpusHead" {
        return Err(AudioError::Decode(
            "not an Ogg Opus stream: first packet is not OpusHead".to_string(),
        ));
    }
    let channels = data[9];
    let pre_skip = u16::from_le_bytes([data[10], data[11]]);
    Ok(OpusHead { channels, pre_skip })
}

/// Downmixes an interleaved multi-channel buffer to mono by averaging
/// channels, or returns it unchanged if it is already mono.
fn downmix_to_mono(interleaved: &[f32], channels: u8) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let channels = channels as usize;
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Reads an Ogg Opus file written by [`crate::audio::encoder::OpusTrackWriter`]
/// and returns its decoded audio as mono samples at [`TARGET_SAMPLE_RATE`].
///
/// Honours the `OpusHead` pre-skip (algorithmic delay introduced by the
/// encoder) by dropping that many samples from the start, and trims trailing
/// padding using the final page's granule position.
///
/// A file truncated or corrupted partway through returns the samples
/// decoded before the failure point (with a logged warning) rather than an
/// error, so a recording that ended in a crash is still transcribable. Only
/// a file that yields no decodable audio at all (empty, not Ogg, or failing
/// before any audio packet) returns `Err`.
pub fn decode_opus_file(path: &Path) -> Result<Vec<f32>, AudioError> {
    let file = File::open(path).map_err(|e| map_io_err(path, e))?;
    let mut reader = PacketReader::new(file);

    let head_packet = match reader.read_packet() {
        Ok(Some(packet)) => packet,
        Ok(None) => {
            return Err(AudioError::Decode(format!(
                "'{}' is empty: no OpusHead packet found",
                path.display()
            )));
        }
        Err(e) => {
            return Err(AudioError::Decode(format!(
                "'{}' is not a readable Ogg stream: {e}",
                path.display()
            )));
        }
    };
    let stream_serial = head_packet.stream_serial();
    let head = parse_opus_head(&head_packet.data)?;

    if head.channels != 1 && head.channels != 2 {
        return Err(AudioError::Decode(format!(
            "unsupported Opus channel count: {}",
            head.channels
        )));
    }
    let channels_enum = if head.channels == 1 {
        Channels::Mono
    } else {
        Channels::Stereo
    };

    // OpusTags is expected next; a missing or malformed one doesn't stop
    // decoding, since it carries no audio.
    match reader.read_packet() {
        Ok(Some(packet)) if packet.data.starts_with(b"OpusTags") => {}
        Ok(Some(_)) => log::warn!(
            "'{}': expected OpusTags after OpusHead, decoding anyway",
            path.display()
        ),
        Ok(None) => {
            return Err(AudioError::Decode(format!(
                "'{}' ends after OpusHead: no audio data",
                path.display()
            )));
        }
        Err(e) => {
            log::warn!(
                "'{}': failed reading OpusTags ({e}), decoding anyway",
                path.display()
            );
        }
    }

    let mut decoder = OpusDecoder::new(TARGET_SAMPLE_RATE, channels_enum)
        .map_err(|e| AudioError::Decode(format!("failed to create Opus decoder: {e}")))?;

    let mut samples: Vec<f32> = Vec::new();
    let mut last_granule: u64 = 0;
    let channels = head.channels as usize;

    loop {
        let packet = match reader.read_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(e) => {
                log::warn!(
                    "'{}': Ogg stream ended early ({e}), keeping samples decoded so far",
                    path.display()
                );
                break;
            }
        };
        if packet.stream_serial() != stream_serial {
            // Belongs to a different logical stream multiplexed into the
            // same file; we never write those, but ignore rather than fail.
            continue;
        }
        if packet.data.is_empty() {
            // The writer's end-of-stream marker: no audio, just a granule
            // position update.
            last_granule = packet.absgp_page();
            continue;
        }

        let nb_samples = match opus::packet::get_nb_samples(&packet.data, TARGET_SAMPLE_RATE) {
            Ok(n) => n,
            Err(e) => {
                log::warn!(
                    "'{}': corrupt Opus packet ({e}), keeping samples decoded so far",
                    path.display()
                );
                break;
            }
        };
        let mut buf = vec![0f32; nb_samples * channels];
        match decoder.decode_float(&packet.data, &mut buf, false) {
            Ok(decoded_frames) => {
                buf.truncate(decoded_frames * channels);
                samples.extend_from_slice(&downmix_to_mono(&buf, head.channels));
                last_granule = packet.absgp_page();
            }
            Err(e) => {
                log::warn!(
                    "'{}': failed to decode Opus packet ({e}), keeping samples decoded so far",
                    path.display()
                );
                break;
            }
        }
    }

    if samples.is_empty() {
        return Err(AudioError::Decode(format!(
            "'{}': no audio decoded",
            path.display()
        )));
    }

    // Drop the algorithmic delay the encoder's lookahead introduced.
    let pre_skip_samples =
        ((head.pre_skip as u64 * TARGET_SAMPLE_RATE as u64) / GRANULE_RATE_HZ) as usize;
    let pre_skip_samples = pre_skip_samples.min(samples.len());
    let samples = samples.split_off(pre_skip_samples);

    // Trim trailing padding using the final page's granule position, which
    // the writer sets to the exact number of original input samples
    // (see `OpusTrackWriter::finish`). Only applied when we saw a granule
    // position and it doesn't ask us to keep more than we decoded, which
    // would indicate a truncated file where trimming should not happen.
    let ratio = GRANULE_RATE_HZ / TARGET_SAMPLE_RATE as u64;
    let total_valid = (last_granule / ratio) as usize;
    let samples = if total_valid > pre_skip_samples {
        let valid_len = total_valid - pre_skip_samples;
        if valid_len < samples.len() {
            let mut samples = samples;
            samples.truncate(valid_len);
            samples
        } else {
            samples
        }
    } else {
        samples
    };

    Ok(samples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::encoder::OpusTrackWriter;
    use crate::audio::TrackWriter;
    use std::f32::consts::PI;

    fn sine_samples(seconds: f64, freq: f64) -> Vec<f32> {
        let n = (TARGET_SAMPLE_RATE as f64 * seconds) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / TARGET_SAMPLE_RATE as f64;
                (2.0 * PI as f64 * freq * t).sin() as f32 * 0.5
            })
            .collect()
    }

    /// Pearson correlation coefficient between two equal-length signals,
    /// used to assert similarity after lossy Opus round-tripping instead of
    /// exact equality.
    fn correlation(a: &[f32], b: &[f32]) -> f64 {
        let n = a.len().min(b.len());
        assert!(n > 0);
        let (a, b) = (&a[..n], &b[..n]);
        let mean_a = a.iter().map(|&x| x as f64).sum::<f64>() / n as f64;
        let mean_b = b.iter().map(|&x| x as f64).sum::<f64>() / n as f64;
        let mut cov = 0.0;
        let mut var_a = 0.0;
        let mut var_b = 0.0;
        for i in 0..n {
            let da = a[i] as f64 - mean_a;
            let db = b[i] as f64 - mean_b;
            cov += da * db;
            var_a += da * da;
            var_b += db * db;
        }
        if var_a == 0.0 || var_b == 0.0 {
            return 0.0;
        }
        cov / (var_a.sqrt() * var_b.sqrt())
    }

    #[test]
    fn round_trips_against_the_real_encoder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roundtrip.opus");
        let original = sine_samples(2.0, 440.0);

        let mut writer = OpusTrackWriter::create(&path).unwrap();
        writer.write(&original).unwrap();
        Box::new(writer).finish().unwrap();

        let decoded = decode_opus_file(&path).unwrap();

        // Opus is lossy and the encoder pads the last partial frame, so
        // allow a small slack on sample count rather than exact equality.
        let diff = (decoded.len() as i64 - original.len() as i64).unsigned_abs();
        assert!(
            diff <= 320,
            "decoded {} samples, expected close to {}",
            decoded.len(),
            original.len()
        );

        let corr = correlation(&original, &decoded);
        assert!(corr > 0.9, "correlation too low: {corr}");
    }

    #[test]
    fn round_trip_with_silence_is_similar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("silence.opus");
        let original = vec![0.0f32; TARGET_SAMPLE_RATE as usize]; // 1s of silence

        let mut writer = OpusTrackWriter::create(&path).unwrap();
        writer.write(&original).unwrap();
        Box::new(writer).finish().unwrap();

        let decoded = decode_opus_file(&path).unwrap();
        let diff = (decoded.len() as i64 - original.len() as i64).unsigned_abs();
        assert!(diff <= 320, "decoded {} samples", decoded.len());
        // Silence stays near zero; correlation is undefined for a
        // zero-variance signal, so assert amplitude instead.
        let max_abs = decoded.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(max_abs < 0.05, "silence round-trip too loud: {max_abs}");
    }

    #[test]
    fn empty_file_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.opus");
        std::fs::write(&path, []).unwrap();

        let result = decode_opus_file(&path);
        assert!(matches!(result, Err(AudioError::Decode(_))));
    }

    #[test]
    fn non_ogg_file_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not_ogg.opus");
        std::fs::write(&path, b"this is definitely not an ogg stream, just text").unwrap();

        let result = decode_opus_file(&path);
        assert!(matches!(result, Err(AudioError::Decode(_))));
    }

    #[test]
    fn truncated_mid_page_returns_partial_audio_instead_of_failing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("full.opus");
        let original = sine_samples(3.0, 440.0);

        let mut writer = OpusTrackWriter::create(&path).unwrap();
        writer.write(&original).unwrap();
        Box::new(writer).finish().unwrap();

        let full_bytes = std::fs::read(&path).unwrap();
        assert!(
            full_bytes.len() > 200,
            "fixture too small to truncate meaningfully"
        );

        let truncated_path = dir.path().join("truncated.opus");
        // Cut well into the audio data, past the header pages, so some
        // whole packets exist before the cut.
        let cut_at = full_bytes.len() * 2 / 3;
        std::fs::write(&truncated_path, &full_bytes[..cut_at]).unwrap();

        let decoded = decode_opus_file(&truncated_path).unwrap();
        assert!(!decoded.is_empty(), "expected partial audio, got none");
        assert!(
            decoded.len() < original.len(),
            "truncated file should decode fewer samples than the full one"
        );
    }
}
