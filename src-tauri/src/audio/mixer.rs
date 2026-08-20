//! Combines the mic and system tracks into the single buffer the
//! transcriber consumes.

/// Sums two already-converted mono streams (both expected at
/// [`super::TARGET_SAMPLE_RATE`]) into one buffer.
///
/// The result is as long as the longer input; the shorter one is treated as
/// silence past its end. Before summing, each side is attenuated by half so
/// that two full-scale (`[-1.0, 1.0]`) inputs summing in phase still lands
/// within `[-1.0, 1.0]` without hard clipping — a simple, cheap mix that
/// trades a little headroom for never distorting.
pub fn mix_tracks(mic: &[f32], system: &[f32]) -> Vec<f32> {
    const ATTENUATION: f32 = 0.5;

    let len = mic.len().max(system.len());
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let m = mic.get(i).copied().unwrap_or(0.0);
        let s = system.get(i).copied().unwrap_or(0.0);
        out.push((m * ATTENUATION + s * ATTENUATION).clamp(-1.0, 1.0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_length_is_the_longer_input() {
        let mic = vec![0.1; 100];
        let system = vec![0.2; 250];
        assert_eq!(mix_tracks(&mic, &system).len(), 250);
        assert_eq!(mix_tracks(&system, &mic).len(), 250);
    }

    #[test]
    fn sums_both_tracks_attenuated() {
        let mic = vec![0.2];
        let system = vec![0.4];
        let out = mix_tracks(&mic, &system);
        assert_eq!(out.len(), 1);
        assert!((out[0] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn stays_within_bounds_on_full_scale_input() {
        let mic = vec![1.0; 1000];
        let system = vec![1.0; 1000];
        let out = mix_tracks(&mic, &system);
        assert!(out.iter().all(|&s| (-1.0..=1.0).contains(&s)));
        // Two full-scale in-phase signals at 0.5 attenuation should land at
        // the boundary, not clip below it.
        assert!(out.iter().all(|&s| (s - 1.0).abs() < 1e-6));

        let mic_neg = vec![-1.0; 1000];
        let system_neg = vec![-1.0; 1000];
        let out_neg = mix_tracks(&mic_neg, &system_neg);
        assert!(out_neg.iter().all(|&s| (-1.0..=1.0).contains(&s)));
    }

    #[test]
    fn one_empty_side_passes_the_other_through_attenuated() {
        let mic = vec![0.6, -0.6];
        let system: Vec<f32> = Vec::new();
        let out = mix_tracks(&mic, &system);
        assert_eq!(out.len(), 2);
        assert!((out[0] - 0.3).abs() < 1e-6);
        assert!((out[1] - (-0.3)).abs() < 1e-6);
    }

    #[test]
    fn both_empty_returns_empty() {
        let out = mix_tracks(&[], &[]);
        assert!(out.is_empty());
    }
}
