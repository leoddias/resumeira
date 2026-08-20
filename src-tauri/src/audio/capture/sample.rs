//! Converts one captured buffer, in whatever format cpal handed it to us,
//! into `f32` samples in `[-1.0, 1.0]`.
//!
//! Every conversion here is a pure function of a slice of the concrete
//! sample type, so each one is unit-tested directly — no `Data`, no device,
//! no hardware. `convert_chunk` is the only piece that touches cpal's
//! dynamically typed `Data` buffer, dispatching to the pure functions below.

use cpal::{Data, SampleFormat, SizedSample, I24, U24};

use crate::audio::AudioError;

/// Converts `data` to interleaved `f32` samples in `[-1.0, 1.0]`.
///
/// `device` is used only to name the device in the error if the format is
/// one this crate doesn't convert (never for logging sample content).
pub(crate) fn convert_chunk(data: &Data, device: &str) -> Result<Vec<f32>, AudioError> {
    match data.sample_format() {
        SampleFormat::F32 => as_slice::<f32>(data).map(|s| s.to_vec()),
        SampleFormat::F64 => {
            as_slice::<f64>(data).map(|s| s.iter().map(|&v| f64_to_f32(v)).collect())
        }
        SampleFormat::I8 => as_slice::<i8>(data).map(|s| s.iter().map(|&v| i8_to_f32(v)).collect()),
        SampleFormat::I16 => {
            as_slice::<i16>(data).map(|s| s.iter().map(|&v| i16_to_f32(v)).collect())
        }
        SampleFormat::I32 => {
            as_slice::<i32>(data).map(|s| s.iter().map(|&v| i32_to_f32(v)).collect())
        }
        SampleFormat::I64 => {
            as_slice::<i64>(data).map(|s| s.iter().map(|&v| i64_to_f32(v)).collect())
        }
        SampleFormat::U8 => as_slice::<u8>(data).map(|s| s.iter().map(|&v| u8_to_f32(v)).collect()),
        SampleFormat::U16 => {
            as_slice::<u16>(data).map(|s| s.iter().map(|&v| u16_to_f32(v)).collect())
        }
        SampleFormat::U32 => {
            as_slice::<u32>(data).map(|s| s.iter().map(|&v| u32_to_f32(v)).collect())
        }
        SampleFormat::U64 => {
            as_slice::<u64>(data).map(|s| s.iter().map(|&v| u64_to_f32(v)).collect())
        }
        SampleFormat::I24 => {
            as_slice::<I24>(data).map(|s| s.iter().map(|&v| i24_to_f32(v)).collect())
        }
        SampleFormat::U24 => {
            as_slice::<U24>(data).map(|s| s.iter().map(|&v| u24_to_f32(v)).collect())
        }
        // DSD and any future variant: never silently produce zeros.
        other => Err(AudioError::UnsupportedConfig {
            device: device.to_string(),
            reason: format!("sample format '{other}' is not supported"),
        }),
    }
}

/// Views `data` as `&[T]`, mapping a format mismatch to a `Stream` error
/// instead of panicking. In practice this never mismatches: `convert_chunk`
/// only calls it with the format `data` itself reports.
fn as_slice<T: SizedSample>(data: &Data) -> Result<&[T], AudioError> {
    data.as_slice::<T>().ok_or_else(|| {
        AudioError::Stream("captured buffer did not match its declared sample format".to_string())
    })
}

pub(crate) fn i8_to_f32(v: i8) -> f32 {
    v as f32 / 128.0
}

pub(crate) fn i16_to_f32(v: i16) -> f32 {
    v as f32 / 32_768.0
}

pub(crate) fn i32_to_f32(v: i32) -> f32 {
    (f64::from(v) / 2_147_483_648.0) as f32
}

pub(crate) fn i64_to_f32(v: i64) -> f32 {
    (v as f64 / 9_223_372_036_854_775_808.0) as f32
}

pub(crate) fn u8_to_f32(v: u8) -> f32 {
    (f32::from(v) - 128.0) / 128.0
}

pub(crate) fn u16_to_f32(v: u16) -> f32 {
    (f32::from(v) - 32_768.0) / 32_768.0
}

pub(crate) fn u32_to_f32(v: u32) -> f32 {
    ((f64::from(v) - 2_147_483_648.0) / 2_147_483_648.0) as f32
}

pub(crate) fn u64_to_f32(v: u64) -> f32 {
    (((v as f64) - 9_223_372_036_854_775_808.0) / 9_223_372_036_854_775_808.0) as f32
}

pub(crate) fn f64_to_f32(v: f64) -> f32 {
    v as f32
}

pub(crate) fn i24_to_f32(v: I24) -> f32 {
    v.inner() as f32 / 8_388_608.0
}

pub(crate) fn u24_to_f32(v: U24) -> f32 {
    (v.inner() as f32 - 8_388_608.0) / 8_388_608.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_data<T: SizedSample>(samples: &[T]) -> Data {
        // SAFETY: `samples` outlives every use of `data` in these tests, the
        // pointer/len describe exactly that slice, and `T::FORMAT` is the
        // format the slice actually holds.
        unsafe { Data::from_parts(samples.as_ptr() as *mut (), samples.len(), T::FORMAT) }
    }

    const TOLERANCE: f32 = 1e-4;

    fn assert_close(a: f32, b: f32) {
        assert!((a - b).abs() < TOLERANCE, "{a} not close to {b}");
    }

    #[test]
    fn f32_passes_through_unchanged() {
        let samples = [-1.0f32, 0.0, 0.5, 1.0];
        let data = make_data(&samples);
        let out = convert_chunk(&data, "test").unwrap();
        assert_eq!(out, samples);
    }

    #[test]
    fn f64_narrows_to_f32() {
        let samples = [-1.0f64, 0.0, 1.0];
        let data = make_data(&samples);
        let out = convert_chunk(&data, "test").unwrap();
        assert_close(out[0], -1.0);
        assert_close(out[1], 0.0);
        assert_close(out[2], 1.0);
    }

    #[test]
    fn i8_extremes_and_midpoint_map_into_range() {
        assert_close(i8_to_f32(i8::MIN), -1.0);
        assert_close(i8_to_f32(0), 0.0);
        assert!(i8_to_f32(i8::MAX) < 1.0 && i8_to_f32(i8::MAX) > 0.99);
    }

    #[test]
    fn i16_extremes_and_midpoint_map_into_range() {
        assert_close(i16_to_f32(i16::MIN), -1.0);
        assert_close(i16_to_f32(0), 0.0);
        assert!(i16_to_f32(i16::MAX) < 1.0 && i16_to_f32(i16::MAX) > 0.99);
    }

    #[test]
    fn i32_extremes_and_midpoint_map_into_range() {
        assert_close(i32_to_f32(i32::MIN), -1.0);
        assert_close(i32_to_f32(0), 0.0);
        assert!(i32_to_f32(i32::MAX) <= 1.0 && i32_to_f32(i32::MAX) > 0.99);
    }

    #[test]
    fn i64_extremes_and_midpoint_map_into_range() {
        assert_close(i64_to_f32(i64::MIN), -1.0);
        assert_close(i64_to_f32(0), 0.0);
        assert!(i64_to_f32(i64::MAX) <= 1.0 && i64_to_f32(i64::MAX) > 0.99);
    }

    /// The requirement the packet calls out explicitly: an unsigned format's
    /// midpoint (its silence value) must map to ~0.0, not to 1.0 or a
    /// silent-but-wrong value.
    #[test]
    fn u16_midpoint_maps_to_zero() {
        assert_close(u16_to_f32(32_768), 0.0);
        assert_close(u16_to_f32(0), -1.0);
        assert!(u16_to_f32(u16::MAX) < 1.0 && u16_to_f32(u16::MAX) > 0.99);
    }

    #[test]
    fn u8_midpoint_maps_to_zero() {
        assert_close(u8_to_f32(128), 0.0);
        assert_close(u8_to_f32(0), -1.0);
        assert!(u8_to_f32(u8::MAX) < 1.0 && u8_to_f32(u8::MAX) > 0.99);
    }

    #[test]
    fn u32_midpoint_maps_to_zero() {
        assert_close(u32_to_f32(1 << 31), 0.0);
        assert_close(u32_to_f32(0), -1.0);
        assert!(u32_to_f32(u32::MAX) <= 1.0 && u32_to_f32(u32::MAX) > 0.99);
    }

    #[test]
    fn u64_midpoint_maps_to_zero() {
        assert_close(u64_to_f32(1u64 << 63), 0.0);
        assert_close(u64_to_f32(0), -1.0);
        assert!(u64_to_f32(u64::MAX) <= 1.0 && u64_to_f32(u64::MAX) > 0.99);
    }

    #[test]
    fn i24_extremes_and_midpoint_map_into_range() {
        let min = I24::new(-8_388_608).unwrap();
        let mid = I24::new(0).unwrap();
        let max = I24::new(8_388_607).unwrap();
        assert_close(i24_to_f32(min), -1.0);
        assert_close(i24_to_f32(mid), 0.0);
        assert!(i24_to_f32(max) < 1.0 && i24_to_f32(max) > 0.99);
    }

    #[test]
    fn u24_midpoint_maps_to_zero() {
        let min = U24::new(0).unwrap();
        let mid = U24::new(8_388_608).unwrap();
        let max = U24::new(16_777_215).unwrap();
        assert_close(u24_to_f32(mid), 0.0);
        assert_close(u24_to_f32(min), -1.0);
        assert!(u24_to_f32(max) < 1.0 && u24_to_f32(max) > 0.99);
    }

    #[test]
    fn i16_slice_conversion_matches_scalar_conversion() {
        let samples = [i16::MIN, -1, 0, 1, i16::MAX];
        let data = make_data(&samples);
        let out = convert_chunk(&data, "test").unwrap();
        for (v, s) in out.iter().zip(samples.iter()) {
            assert_close(*v, i16_to_f32(*s));
        }
    }

    #[test]
    fn u16_slice_conversion_matches_scalar_conversion() {
        let samples = [0u16, 32_768, 65_535];
        let data = make_data(&samples);
        let out = convert_chunk(&data, "test").unwrap();
        for (v, s) in out.iter().zip(samples.iter()) {
            assert_close(*v, u16_to_f32(*s));
        }
    }

    /// Unsupported formats must be an explicit error, never a silent zero.
    #[test]
    fn dsd_formats_are_rejected_explicitly() {
        let samples = [0u8; 4];
        // SAFETY: DSD-u8 has the same 1-byte layout as u8, and the slice
        // outlives this call; we never read it as anything but bytes.
        let data = unsafe { Data::from_parts(samples.as_ptr() as *mut (), 4, SampleFormat::DsdU8) };
        let err = convert_chunk(&data, "dsd-device").unwrap_err();
        match err {
            AudioError::UnsupportedConfig { device, reason } => {
                assert_eq!(device, "dsd-device");
                assert!(reason.contains("dsdu8"));
            }
            other => panic!("expected AudioError::UnsupportedConfig, got {other:?}"),
        }
    }
}
