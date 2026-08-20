//! Picks a concrete stream configuration from a device's supported ranges.
//!
//! Pure and hardware-free: it operates on the `SupportedStreamConfigRange`
//! values cpal reports, never on a device itself, so the selection logic can
//! be exercised with a hand-built list in tests.

use cpal::{SampleFormat, SampleRate, SupportedStreamConfig, SupportedStreamConfigRange};

/// How directly a sample format converts to `f32`: lower ranks are
/// preferred. `F32` needs no conversion at all; everything else is ranked by
/// how much precision or complexity the conversion costs.
fn format_rank(format: SampleFormat) -> u8 {
    match format {
        SampleFormat::F32 => 0,
        SampleFormat::I16 => 1,
        SampleFormat::I32 => 2,
        SampleFormat::U16 => 3,
        SampleFormat::I8 => 4,
        SampleFormat::U8 => 5,
        SampleFormat::I24 => 6,
        SampleFormat::U24 => 7,
        SampleFormat::I64 => 8,
        SampleFormat::U32 => 9,
        SampleFormat::U64 => 10,
        SampleFormat::F64 => 11,
        // DSD and any future variant this crate doesn't convert: last resort.
        _ => u8::MAX,
    }
}

/// Picks the supported config whose achievable sample rate is closest to
/// `preferred_rate`, breaking ties by [`format_rank`].
///
/// Returns `None` if `ranges` is empty — the caller turns that into
/// `AudioError::UnsupportedConfig`.
pub(crate) fn select_config(
    ranges: impl IntoIterator<Item = SupportedStreamConfigRange>,
    preferred_rate: SampleRate,
) -> Option<SupportedStreamConfig> {
    ranges
        .into_iter()
        .map(|range| {
            let candidate_rate =
                preferred_rate.clamp(range.min_sample_rate(), range.max_sample_rate());
            let distance = candidate_rate.abs_diff(preferred_rate);
            let rank = format_rank(range.sample_format());
            (distance, rank, range.with_sample_rate(candidate_rate))
        })
        .min_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)))
        .map(|(_, _, config)| config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::SupportedBufferSize;

    fn range(
        channels: u16,
        min: u32,
        max: u32,
        format: SampleFormat,
    ) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(channels, min, max, SupportedBufferSize::Unknown, format)
    }

    #[test]
    fn empty_ranges_select_nothing() {
        assert!(select_config(Vec::new(), 16_000).is_none());
    }

    #[test]
    fn exact_preferred_rate_is_used_when_available() {
        let ranges = vec![
            range(1, 8_000, 12_000, SampleFormat::I16),
            range(1, 16_000, 16_000, SampleFormat::I16),
            range(2, 44_100, 48_000, SampleFormat::F32),
        ];
        let config = select_config(ranges, 16_000).unwrap();
        assert_eq!(config.sample_rate(), 16_000);
        assert_eq!(config.channels(), 1);
    }

    #[test]
    fn closest_achievable_rate_wins_when_nothing_matches_exactly() {
        let ranges = vec![
            range(1, 8_000, 12_000, SampleFormat::I16),
            range(2, 44_100, 48_000, SampleFormat::F32),
        ];
        // Distance to 12_000 is 4_000; distance to 44_100 is 28_100.
        let config = select_config(ranges, 16_000).unwrap();
        assert_eq!(config.sample_rate(), 12_000);
    }

    #[test]
    fn ties_on_rate_prefer_the_format_that_converts_most_directly() {
        let ranges = vec![
            range(1, 16_000, 16_000, SampleFormat::I16),
            range(1, 16_000, 16_000, SampleFormat::F32),
        ];
        let config = select_config(ranges, 16_000).unwrap();
        assert_eq!(config.sample_format(), SampleFormat::F32);
    }

    #[test]
    fn a_single_range_clamps_the_preferred_rate_into_bounds() {
        let ranges = vec![range(2, 44_100, 48_000, SampleFormat::F32)];
        let config = select_config(ranges, 16_000).unwrap();
        assert_eq!(config.sample_rate(), 44_100);
    }
}
