//! How loud each track is right now, for the UI's activity meter.
//!
//! A recording bar that only says "Recording" cannot distinguish a live
//! microphone from a muted one, and a muted microphone is discovered after
//! the meeting, when nothing can be done about it. So the capture path keeps
//! the most recent peak per track and the UI draws it.
//!
//! Nothing here allocates, blocks or copies samples: [`LevelMeter::record`]
//! runs on the audio callback thread, where a stall is a dropped buffer.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long a peak is held at full height before it starts falling.
const HOLD: Duration = Duration::from_millis(120);

/// How long the fall from a held peak to silence takes.
const FALL: Duration = Duration::from_millis(400);

/// The quietest sound the meter shows at all, in dBFS. Below this the bar is
/// empty: room tone should not look like speech.
const FLOOR_DB: f32 = -60.0;

/// How long after its last block a meter stops claiming the device is
/// delivering anything. Exactly the window the decay takes to reach zero, so
/// "the bar is empty" and "nothing is arriving" become the same moment.
const STALE_AFTER: Duration = HOLD.saturating_add(FALL);

/// The most recent peak of one track, with a decay so the bar falls smoothly
/// and reaches zero when the audio stops arriving at all.
///
/// A dead device stops calling [`LevelMeter::record`], so the meter empties
/// on its own — it can never claim activity that is no longer happening.
#[derive(Debug)]
pub struct LevelMeter {
    origin: Instant,
    /// The most recent reading, as one word: the peak's `f32` bits in the
    /// high half, milliseconds since `origin` in the low half. Packed
    /// together so a reader can never pair a fresh peak with a stale
    /// timestamp, which would draw one wrong frame of bar.
    reading: AtomicU64,
    /// Whether anything has ever been recorded, so "no audio yet" and
    /// "silence" are not the same reading.
    seen: AtomicBool,
    /// Test seam: extra milliseconds added to this meter's clock, so decay
    /// and staleness can be exercised without sleeping.
    #[cfg(test)]
    skew_ms: AtomicU64,
}

impl Default for LevelMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl LevelMeter {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            reading: AtomicU64::new(0),
            seen: AtomicBool::new(false),
            #[cfg(test)]
            skew_ms: AtomicU64::new(0),
        }
    }

    /// Records one block of mono samples. Called from the audio thread.
    ///
    /// Only the peak is kept: it is one pass with no allocation, and it is
    /// what a meter is supposed to show. Samples themselves are never
    /// retained — this type holds one number and a timestamp.
    pub fn record(&self, samples: &[f32]) {
        let mut peak = 0.0_f32;
        for &sample in samples {
            let magnitude = sample.abs();
            // `>` is false for NaN, so a malformed sample is ignored rather
            // than poisoning the meter forever.
            if magnitude > peak {
                peak = magnitude;
            }
        }
        self.store(peak.min(1.0), self.now());
    }

    /// The height the meter should be drawn at, in `0.0..=1.0`.
    ///
    /// Perceptual, not linear: speech peaks well below full scale, and a
    /// linear bar makes a perfectly healthy microphone look dead.
    pub fn level(&self) -> f32 {
        display_scale(self.peak_now())
    }

    /// The decayed raw peak, in `0.0..=1.0`. Amplitude rather than a bar
    /// height, for anything that wants the honest number.
    pub fn peak_now(&self) -> f32 {
        self.peak_at(self.now())
    }

    /// Whether this meter has ever been given a sample.
    ///
    /// Distinguishes a track that has not started delivering audio from one
    /// that is delivering silence — the UI says different things about them.
    pub fn has_data(&self) -> bool {
        self.seen.load(Ordering::Relaxed)
    }

    /// Whether audio is still arriving.
    ///
    /// Read instead of asking the track whether it has died: a device that
    /// stopped delivering *is* a device that stopped delivering, whether it
    /// reported an error or simply went quiet, and answering from the meter
    /// alone keeps this poll free of the locks the audio thread holds while
    /// it writes.
    pub fn receiving(&self) -> bool {
        self.has_data() && self.age(self.now()) < STALE_AFTER
    }

    fn store(&self, peak: f32, at: Duration) {
        let millis = u32::try_from(at.as_millis()).unwrap_or(u32::MAX);
        self.reading.store(
            (u64::from(peak.to_bits()) << 32) | u64::from(millis),
            Ordering::Relaxed,
        );
        self.seen.store(true, Ordering::Relaxed);
    }

    /// The decayed peak as it would read `now` after this meter was created.
    /// Separate from [`LevelMeter::peak_now`] so the decay is testable
    /// without sleeping.
    fn peak_at(&self, now: Duration) -> f32 {
        let (peak, _) = self.unpack();
        decayed(peak, self.age(now))
    }

    /// How long ago the last block arrived, as of `now`.
    fn age(&self, now: Duration) -> Duration {
        let (_, at) = self.unpack();
        now.saturating_sub(at)
    }

    fn unpack(&self) -> (f32, Duration) {
        let reading = self.reading.load(Ordering::Relaxed);
        (
            f32::from_bits((reading >> 32) as u32),
            Duration::from_millis(reading & 0xFFFF_FFFF),
        )
    }

    /// How long this meter has been running.
    #[cfg(not(test))]
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    /// The same, plus whatever [`LevelMeter::advance`] has pretended.
    #[cfg(test)]
    fn now(&self) -> Duration {
        self.origin.elapsed() + Duration::from_millis(self.skew_ms.load(Ordering::Relaxed))
    }

    /// Test seam: pretend `by` more time has passed, so decay and staleness
    /// can be exercised without sleeping.
    #[cfg(test)]
    pub(crate) fn advance(&self, by: Duration) {
        let millis = u64::try_from(by.as_millis()).unwrap_or(u64::MAX);
        self.skew_ms.fetch_add(millis, Ordering::Relaxed);
    }
}

/// A peak `since` ago, faded by the hold-then-fall envelope.
///
/// Pure so the envelope is a tested fact rather than something only a real
/// recording exercises.
fn decayed(peak: f32, since: Duration) -> f32 {
    if !peak.is_finite() || peak <= 0.0 {
        return 0.0;
    }
    if since <= HOLD {
        return peak.min(1.0);
    }
    let falling = since - HOLD;
    if falling >= FALL {
        return 0.0;
    }
    let remaining = 1.0 - falling.as_secs_f32() / FALL.as_secs_f32();
    (peak * remaining).clamp(0.0, 1.0)
}

/// Maps a raw peak amplitude onto a bar height in `0.0..=1.0`.
///
/// Amplitude is converted to dBFS and laid out linearly from [`FLOOR_DB`] to
/// 0, which is how every audio meter a user has ever seen behaves: normal
/// speech lands around the middle instead of hugging the bottom.
pub fn display_scale(peak: f32) -> f32 {
    if !peak.is_finite() || peak <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * peak.min(1.0).log10();
    ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_meter_reads_zero_and_reports_no_data() {
        let meter = LevelMeter::new();
        assert_eq!(meter.peak_now(), 0.0);
        assert_eq!(meter.level(), 0.0);
        assert!(!meter.has_data());
    }

    #[test]
    fn records_the_loudest_magnitude_in_the_block() {
        let meter = LevelMeter::new();
        meter.record(&[0.1, -0.7, 0.3]);
        assert!((meter.peak_at(Duration::ZERO) - 0.7).abs() < 1e-6);
        assert!(meter.has_data());
    }

    #[test]
    fn silence_records_as_data_but_reads_zero() {
        let meter = LevelMeter::new();
        meter.record(&[0.0; 128]);
        assert_eq!(meter.peak_at(Duration::ZERO), 0.0);
        assert!(
            meter.has_data(),
            "a silent block is still proof the device is delivering audio"
        );
    }

    #[test]
    fn an_empty_block_reads_zero_rather_than_panicking() {
        let meter = LevelMeter::new();
        meter.record(&[]);
        assert_eq!(meter.peak_at(Duration::ZERO), 0.0);
    }

    #[test]
    fn a_malformed_sample_never_poisons_the_meter() {
        let meter = LevelMeter::new();
        meter.record(&[f32::NAN, 0.4, f32::INFINITY, f32::NEG_INFINITY]);
        let peak = meter.peak_at(Duration::ZERO);
        assert!(peak.is_finite(), "peak went non-finite: {peak}");
        assert!((0.0..=1.0).contains(&peak), "peak out of range: {peak}");
    }

    #[test]
    fn out_of_range_samples_are_clamped_to_full_scale() {
        let meter = LevelMeter::new();
        meter.record(&[4.0, -9.0]);
        assert_eq!(meter.peak_at(Duration::ZERO), 1.0);
    }

    // --- decayed(): the hold-then-fall envelope ---

    #[test]
    fn a_peak_is_held_at_full_height_briefly() {
        assert_eq!(decayed(0.8, Duration::ZERO), 0.8);
        assert_eq!(decayed(0.8, HOLD), 0.8);
    }

    #[test]
    fn a_peak_falls_after_the_hold_window() {
        let half = decayed(1.0, HOLD + FALL / 2);
        assert!(
            (half - 0.5).abs() < 0.05,
            "expected a half-height bar: {half}"
        );
    }

    #[test]
    fn a_track_that_stops_delivering_audio_empties_the_meter() {
        assert_eq!(decayed(1.0, HOLD + FALL), 0.0);
        assert_eq!(decayed(1.0, Duration::from_secs(30)), 0.0);
    }

    #[test]
    fn the_meter_empties_on_its_own_without_further_records() {
        let meter = LevelMeter::new();
        meter.record(&[1.0]);
        assert_eq!(meter.peak_at(HOLD + FALL + Duration::from_millis(1)), 0.0);
    }

    // --- receiving(): is audio still arriving? ---

    #[test]
    fn a_meter_being_fed_is_receiving_even_in_silence() {
        let meter = LevelMeter::new();
        meter.record(&[0.0; 128]);
        assert!(meter.receiving());
    }

    #[test]
    fn a_meter_that_stops_being_fed_stops_receiving() {
        let meter = LevelMeter::new();
        meter.record(&[0.5; 128]);
        assert!(meter.receiving());

        meter.advance(STALE_AFTER + Duration::from_millis(1));
        assert!(
            !meter.receiving(),
            "a device that stopped delivering must not look live"
        );
        assert_eq!(meter.level(), 0.0, "and its bar must be empty");
    }

    #[test]
    fn a_meter_nobody_ever_fed_is_not_receiving() {
        let meter = LevelMeter::new();
        assert!(!meter.receiving());
    }

    // --- display_scale(): amplitude to bar height ---

    #[test]
    fn silence_and_full_scale_map_to_the_ends_of_the_bar() {
        assert_eq!(display_scale(0.0), 0.0);
        assert_eq!(display_scale(1.0), 1.0);
    }

    #[test]
    fn speech_level_audio_lands_in_the_visible_middle() {
        // -20 dBFS is an unremarkable speaking level; a linear bar would
        // draw it at 10% and look broken.
        let height = display_scale(0.1);
        assert!(
            (0.55..0.75).contains(&height),
            "speech should read as clearly active: {height}"
        );
    }

    #[test]
    fn anything_below_the_floor_reads_as_empty() {
        assert_eq!(display_scale(0.0001), 0.0);
    }

    #[test]
    fn a_malformed_peak_reads_as_empty_rather_than_full() {
        // An empty bar is the safe reading: a meter pinned at full by a
        // stray NaN would tell the user their microphone is fine.
        assert_eq!(display_scale(f32::NAN), 0.0);
        assert_eq!(display_scale(f32::INFINITY), 0.0);
        assert_eq!(display_scale(-1.0), 0.0);
    }
}
