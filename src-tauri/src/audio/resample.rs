//! Sample-rate and channel conversion to the pipeline's target format.
//!
//! Every captured [`AudioChunk`] is downmixed to mono and resampled to
//! [`TARGET_SAMPLE_RATE`] here, in one tested place, so nothing downstream
//! (writers, the mixer, transcription) needs to reason about the device's
//! native format.

use super::{AudioChunk, TARGET_SAMPLE_RATE};

/// Converts `chunk` to mono samples at [`TARGET_SAMPLE_RATE`].
///
/// Downmixing averages all channels of a frame rather than taking the first
/// one, so a signal that is out of phase or louder on one channel is not
/// silently dropped.
///
/// Resampling uses linear interpolation between neighboring source frames.
/// It is not band-limited (no anti-aliasing filter), so it can introduce
/// mild aliasing on strongly upsampled content; this is an accepted
/// trade-off for M1 — cheap, allocation-light, and more than accurate enough
/// for speech destined for Whisper, which resamples to 16 kHz internally
/// anyway (ADR-0004). A future milestone can swap in a proper sinc resampler
/// behind this same function signature without touching callers.
///
/// Sample rates outside this range are treated as malformed input rather
/// than resampled: no real audio device reports a rate outside it, and
/// honoring a bogus tiny rate (e.g. misreported as `1` Hz against a normal-
/// sized buffer) would blow the output-length computation up to gigabytes,
/// turning one bad chunk into a multi-gigabyte allocation on the audio
/// thread. Range covers everything from telephony (8 kHz) to high-end
/// interfaces (384 kHz) with margin.
const PLAUSIBLE_SAMPLE_RATE_HZ: std::ops::RangeInclusive<u32> = 1_000..=384_000;

/// Never panics. Malformed input (zero channels, an implausible sample rate,
/// empty samples, or a sample count that isn't a multiple of `channels`)
/// returns an empty buffer instead.
pub fn to_target_mono(chunk: &AudioChunk) -> Vec<f32> {
    let channels = chunk.channels as usize;
    if channels == 0
        || !PLAUSIBLE_SAMPLE_RATE_HZ.contains(&chunk.sample_rate)
        || chunk.samples.is_empty()
    {
        return Vec::new();
    }

    let frame_count = chunk.samples.len() / channels;
    if frame_count == 0 {
        // Fewer samples than one full frame (e.g. a truncated interleaved
        // block) — nothing sensible to downmix.
        return Vec::new();
    }

    // Downmix to mono: average every channel of each complete frame. Any
    // trailing samples that don't form a full frame are dropped rather than
    // read out of bounds.
    let mono: Vec<f32> = chunk.samples[..frame_count * channels]
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();

    resample_linear(&mono, chunk.sample_rate, TARGET_SAMPLE_RATE)
}

/// Caller-independent ceiling on how much a resample is allowed to grow the
/// buffer. `to_target_mono` already rejects implausible sample rates before
/// calling this, but this second check is cheap defense in depth: whatever
/// the future caller, one chunk can never demand a runaway allocation.
const MAX_UPSAMPLE_FACTOR: f64 = 32.0;

/// Linearly resamples `input` (mono, at `from_rate` Hz) to `to_rate` Hz.
///
/// Never panics: a zero rate, an empty input, or an upsample ratio beyond
/// [`MAX_UPSAMPLE_FACTOR`] returns an empty buffer instead of allocating an
/// unbounded output.
fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if input.is_empty() || from_rate == 0 || to_rate == 0 {
        return Vec::new();
    }

    if from_rate == to_rate {
        return input.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    if ratio > 0.0 && 1.0 / ratio > MAX_UPSAMPLE_FACTOR {
        return Vec::new();
    }

    let out_len = ((input.len() as f64) / ratio).round() as usize;
    if out_len == 0 {
        return Vec::new();
    }

    let last_index = (input.len() - 1) as f64;
    let mut output = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = (i as f64 * ratio).min(last_index);
        let base = src_pos.floor() as usize;
        let frac = (src_pos - base as f64) as f32;
        let a = input[base];
        let b = input[(base + 1).min(input.len() - 1)];
        output.push(a + (b - a) * frac);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic sine wave, interleaved across `channels` identical
    /// channels, at `sample_rate` Hz for `duration_secs` seconds.
    fn sine_chunk(freq_hz: f32, sample_rate: u32, channels: u16, duration_secs: f32) -> AudioChunk {
        let frame_count = (sample_rate as f32 * duration_secs) as usize;
        let mut samples = Vec::with_capacity(frame_count * channels as usize);
        for n in 0..frame_count {
            let t = n as f32 / sample_rate as f32;
            let value = (2.0 * std::f32::consts::PI * freq_hz * t).sin();
            for _ in 0..channels {
                samples.push(value);
            }
        }
        AudioChunk {
            samples,
            sample_rate,
            channels,
        }
    }

    /// Counts sign changes as a cheap, tolerant proxy for frequency: a sine
    /// at `freq_hz` over `duration_secs` crosses zero `2 * freq_hz *
    /// duration_secs` times.
    fn zero_crossings(samples: &[f32]) -> usize {
        samples
            .windows(2)
            .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0))
            .count()
    }

    #[test]
    fn sine_stays_approximately_440hz_after_conversion() {
        let chunk = sine_chunk(440.0, 48_000, 1, 1.0);
        let out = to_target_mono(&chunk);

        let crossings = zero_crossings(&out);
        let expected = 2 * 440; // 1 second of audio
        let tolerance = (expected as f64 * 0.05) as usize; // 5%
        assert!(
            (crossings as i64 - expected as i64).unsigned_abs() as usize <= tolerance,
            "expected ~{expected} zero crossings, got {crossings}"
        );
    }

    #[test]
    fn output_length_matches_rate_ratio() {
        let chunk = sine_chunk(200.0, 48_000, 1, 1.0);
        let out = to_target_mono(&chunk);

        let expected_len = chunk.samples.len() * TARGET_SAMPLE_RATE as usize / 48_000;
        let tolerance = (expected_len as f64 * 0.02).ceil() as usize; // 2%
        assert!(
            (out.len() as i64 - expected_len as i64).unsigned_abs() as usize <= tolerance,
            "expected ~{expected_len} samples, got {}",
            out.len()
        );
    }

    #[test]
    fn stereo_out_of_phase_downmixes_to_near_zero() {
        let frame_count = 1000;
        let mut samples = Vec::with_capacity(frame_count * 2);
        for _ in 0..frame_count {
            samples.push(1.0);
            samples.push(-1.0);
        }
        let chunk = AudioChunk {
            samples,
            sample_rate: 16_000,
            channels: 2,
        };
        let out = to_target_mono(&chunk);
        assert!(!out.is_empty());
        assert!(out.iter().all(|&s| s.abs() < 1e-6));
    }

    #[test]
    fn already_target_format_passes_through() {
        let chunk = sine_chunk(300.0, TARGET_SAMPLE_RATE, 1, 0.1);
        let out = to_target_mono(&chunk);
        assert_eq!(out.len(), chunk.samples.len());
        for (a, b) in out.iter().zip(chunk.samples.iter()) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn zero_channels_returns_empty() {
        let chunk = AudioChunk {
            samples: vec![0.0; 100],
            sample_rate: 48_000,
            channels: 0,
        };
        assert!(to_target_mono(&chunk).is_empty());
    }

    #[test]
    fn zero_sample_rate_returns_empty() {
        let chunk = AudioChunk {
            samples: vec![0.0; 100],
            sample_rate: 0,
            channels: 1,
        };
        assert!(to_target_mono(&chunk).is_empty());
    }

    #[test]
    fn implausibly_low_sample_rate_returns_empty_without_huge_allocation() {
        // A misreported near-zero rate against a normal-sized buffer would,
        // without a bound, demand an output hundreds of millions of samples
        // long. This must be rejected as malformed, not allocated.
        let chunk = AudioChunk {
            samples: vec![0.0; 48_000],
            sample_rate: 1,
            channels: 1,
        };
        assert!(to_target_mono(&chunk).is_empty());
    }

    #[test]
    fn implausibly_high_sample_rate_returns_empty() {
        let chunk = AudioChunk {
            samples: vec![0.0; 100],
            sample_rate: u32::MAX,
            channels: 1,
        };
        assert!(to_target_mono(&chunk).is_empty());
    }

    #[test]
    fn empty_samples_returns_empty() {
        let chunk = AudioChunk {
            samples: Vec::new(),
            sample_rate: 48_000,
            channels: 2,
        };
        assert!(to_target_mono(&chunk).is_empty());
    }

    #[test]
    fn sample_count_not_multiple_of_channels_does_not_panic() {
        let chunk = AudioChunk {
            samples: vec![0.5; 5], // 5 samples, 2 channels: 2 full frames + 1 orphan
            sample_rate: 48_000,
            channels: 2,
        };
        let out = to_target_mono(&chunk);
        // 2 complete frames survive; must not panic and must not read OOB.
        assert!(out.len() <= 2);
    }

    #[test]
    fn fewer_samples_than_one_frame_returns_empty() {
        let chunk = AudioChunk {
            samples: vec![0.5; 1],
            sample_rate: 48_000,
            channels: 4,
        };
        assert!(to_target_mono(&chunk).is_empty());
    }
}
