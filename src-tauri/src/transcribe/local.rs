//! Local Whisper transcription via whisper-rs.
//!
//! Everything that touches the model file or the FFI boundary — opening it,
//! running inference — lives in [`transcribe`], the one impure function in
//! this module. Everything that decides *what a result means* is a pure
//! function with no whisper-rs context and no filesystem access:
//! [`has_signal`] decides whether a stretch of audio is worth transcribing at
//! all, and [`map_segment`] decides whether one whisper segment is trustworthy
//! enough to keep. Splitting it this way is what lets the module's most
//! important behaviour be tested without a multi-gigabyte model file on disk
//! (docs/TASKS.md T-M2-4).
//!
//! Whisper is a well-documented hallucinator on silence: fed a quiet stretch
//! it confidently emits fluent nonsense such as "Thank you for watching."
//! rather than nothing. A meeting note containing invented speech is worse
//! than a short one, so this module guards against it twice: the whole input
//! is checked for signal before the model ever runs, and every segment
//! whisper does emit is checked again, both against its own slice of the
//! original audio and against whisper's own no-speech confidence, before
//! being kept.

use std::ffi::c_int;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use super::{Engine, Segment, TranscribeError, Transcript};
use crate::audio::TARGET_SAMPLE_RATE;

/// Whisper reports segment timestamps in centiseconds (hundredths of a
/// second), independent of the model's internal sample rate.
const CENTISECONDS_PER_SECOND: f64 = 100.0;

/// RMS amplitude below which a stretch of 16 kHz mono `f32` samples counts as
/// silence. Set well above digital-zero and ordinary quantization noise, but
/// well below the quietest audible speech, so a genuinely silent recording —
/// or a genuinely silent segment within an otherwise normal one — never
/// reaches, or survives, whisper.
const SILENCE_RMS_THRESHOLD: f32 = 0.01;

/// Segments whisper itself is not confident contain speech at all are
/// dropped. This is the second, independent guard against hallucination:
/// even when a segment's audio window has *some* signal (room tone, a
/// cough), a high no-speech probability is whisper's own admission that the
/// text it attached to the segment was invented, not heard.
const NO_SPEECH_PROB_THRESHOLD: f32 = 0.5;

/// Runs local Whisper transcription over `samples` (mono `f32` at
/// [`TARGET_SAMPLE_RATE`]) using the model at `model_path`.
///
/// `language` follows whisper-rs's own convention: `None` means auto-detect.
/// Whichever language whisper settles on — requested or detected — is
/// reported back in [`Transcript::language`].
///
/// `on_progress` receives a coarse 0-100 percentage as decoding proceeds, so
/// a caller can move a progress bar. `on_line` receives each segment's text
/// as whisper produces it, so a caller can show the meeting arriving rather
/// than a frozen label — see [`preview_line`] for what it is and is not.
///
/// Empty input, and input with no audible signal anywhere in it, never reach
/// the model at all: both return an empty `Ok` transcript. This is
/// deliberate, not just an optimization — it is the primary defence against
/// whisper hallucinating text over a meeting that recorded silence.
///
/// A missing or unreadable model file is [`TranscribeError::ModelMissing`],
/// never a panic. Any other whisper-rs failure — a corrupt or unsupported
/// model, a failure mid-decode — is [`TranscribeError::LocalEngine`].
pub fn transcribe<F, G>(
    model_path: &Path,
    samples: &[f32],
    language: Option<&str>,
    on_progress: F,
    mut on_line: G,
) -> Result<Transcript, TranscribeError>
where
    F: FnMut(u32) + Send + 'static,
    G: FnMut(&str) + Send + 'static,
{
    if !has_signal(samples) {
        return Ok(Transcript {
            segments: Vec::new(),
            language: None,
            engine: Engine::Local,
        });
    }

    ensure_readable(model_path)?;

    let ctx = cached_context(model_path)?;
    let mut state = ctx
        .create_state()
        .map_err(|err| TranscribeError::LocalEngine(format!("failed to init state: {err}")))?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 5 });
    params.set_language(language);
    // whisper.cpp defaults to 4 threads no matter how wide the CPU is; on a
    // typical 8-core desktop that leaves half the machine idle for the
    // slowest step this app has. Physical cores, not logical: whisper is
    // memory-bandwidth-bound, and hyperthreads contend rather than help.
    params.set_n_threads(decode_threads(num_cpus::get_physical()));
    // Each 30 s window is decoded fresh, without the previous window's text
    // as a prompt. Conditioning on prior text is what turns one hallucinated
    // "Thank you." over a quiet stretch into a run of them: the invented
    // line becomes the prompt for the next window, which dutifully repeats
    // it. Losing cross-window context costs a little fluency and buys an end
    // to the feedback loop.
    params.set_no_context(true);
    // Non-speech tokens ((music), [BLANK_AUDIO], …) are suppressed at the
    // decoder rather than filtered after the fact — the decoder then spends
    // its probability mass on words or on no-speech, which also sharpens the
    // no_speech_probability this module already trusts.
    params.set_suppress_nst(true);
    params.set_print_progress(false);
    params.set_print_special(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    // Both callbacks are caught, because whisper-rs installs them behind a
    // bare `extern "C"` trampoline with no unwind guard: a panic crossing
    // that frame aborts the process outright, and `spawn_blocking` never
    // gets to report it. These run app code now — a sink that reaches Tauri's
    // event system — so a stray panic there would kill the app *after* the
    // meeting, with the note unwritten. Reporting is decoration; losing it
    // is always better than losing the note.
    let mut report_progress = progress_reporter(on_progress);
    params.set_progress_callback_safe(move |percent: i32| {
        let _ = catch_unwind(AssertUnwindSafe(|| report_progress(percent)));
    });
    // The non-lossy variant on purpose: it skips a segment whose bytes are
    // not valid UTF-8, where `set_segment_callback_safe_lossy` in whisper-rs
    // 0.16 declares a trampoline with one argument too many. A preview is
    // not worth an ABI mismatch inside a C callback during a real meeting.
    params.set_segment_callback_safe(move |data: whisper_rs::SegmentCallbackData| {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Some(line) = preview_line(&data.text) {
                on_line(&line);
            }
        }));
    });

    state
        .full(params, samples)
        .map_err(|err| TranscribeError::LocalEngine(format!("transcription failed: {err}")))?;

    let mut segments = Vec::new();
    for raw in state.as_iter() {
        let text = raw
            .to_str_lossy()
            .map_err(|err| TranscribeError::LocalEngine(format!("bad segment text: {err}")))?
            .into_owned();
        let candidate = RawSegment {
            start_centiseconds: raw.start_timestamp(),
            end_centiseconds: raw.end_timestamp(),
            text,
            no_speech_probability: raw.no_speech_probability(),
        };
        if let Some(segment) = map_segment(&candidate, samples, TARGET_SAMPLE_RATE) {
            segments.push(segment);
        }
    }

    let language = language_tag(state.full_lang_id_from_state());

    Ok(Transcript {
        segments: super::collapse_repeated_segments(segments),
        language,
        engine: Engine::Local,
    })
}

/// The longest preview line worth carrying. A recording bar shows a clause,
/// not a paragraph.
const PREVIEW_MAX_CHARS: usize = 200;

/// One segment's text as it should appear in a live preview, or nothing.
///
/// A *preview*, not a transcript: this sees text only, so it cannot apply
/// the checks [`map_segment`] applies with the audio in hand. It drops the
/// obvious noise — blank text and whisper's bracketed placeholders
/// (`[BLANK_AUDIO]`, `(music)`) — and lets everything else through, up to
/// [`PREVIEW_MAX_CHARS`]. A line shown here may still be dropped from the
/// finished note, which is the right way round: the note stays strict, the
/// preview stays alive.
fn preview_line(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let placeholder = (trimmed.starts_with('[') && trimmed.ends_with(']'))
        || (trimmed.starts_with('(') && trimmed.ends_with(')'));
    if placeholder {
        return None;
    }
    // Bounded before it goes anywhere: a preview line crosses IPC and sits
    // in the app's state, and one line is all the bar can show anyway. A
    // pathological segment must not put an unbounded string there.
    Some(trimmed.chars().take(PREVIEW_MAX_CHARS).collect())
}

/// Wraps a caller's progress callback so whisper-rs's `i32` percentage
/// (which can be out of `0..=100` at either end in practice) always reaches
/// the caller as a clamped, unsigned coarse percentage.
fn progress_reporter<F>(mut on_progress: F) -> impl FnMut(i32) + 'static
where
    F: FnMut(u32) + Send + 'static,
{
    move |percent: i32| on_progress(percent.clamp(0, 100) as u32)
}

/// A missing or unreadable model file must fail as
/// [`TranscribeError::ModelMissing`], never as a whisper-rs panic or an
/// opaque [`TranscribeError::LocalEngine`]. This is checked before
/// whisper-rs ever touches the path: whisper.cpp itself only logs to stderr
/// and returns a null context on failure, with no way to ask it why, so the
/// missing/unreadable-vs-corrupt distinction the contract requires has to be
/// made here.
fn ensure_readable(model_path: &Path) -> Result<(), TranscribeError> {
    let missing = || TranscribeError::ModelMissing {
        model: model_path.display().to_string(),
    };

    // The file *kind* is checked before the handle, because `File::open` is
    // not a portable test for "this is a usable file": on Unix a directory
    // opens successfully and only fails on read, so a path pointing at a
    // folder would sail past the guard and reach whisper.cpp, which reports
    // its own failure as an opaque `LocalEngine` error. Windows refuses the
    // open outright, which is why this only ever failed off-Windows.
    let metadata = std::fs::metadata(model_path).map_err(|_| missing())?;
    if !metadata.is_file() {
        return Err(missing());
    }

    std::fs::File::open(model_path)
        .map(|_| ())
        .map_err(|_| missing())
}

/// How many threads whisper decodes with, given the machine's physical core
/// count. At least one, and never more cores than exist; the count is
/// otherwise the full set of physical cores, because transcription runs
/// after the meeting, when there is no live audio thread to starve.
fn decode_threads(physical_cores: usize) -> c_int {
    physical_cores.clamp(1, c_int::MAX as usize) as c_int
}

/// The identity of a model file at load time. If any of it changes — the
/// path, the size, the mtime — the cached context is stale and reloaded.
#[derive(Clone)]
struct ModelIdentity {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

impl ModelIdentity {
    fn of(model_path: &Path) -> std::io::Result<Self> {
        let meta = std::fs::metadata(model_path)?;
        Ok(Self {
            path: model_path.to_owned(),
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }

    /// Whether a cached context loaded under `self` still stands for the
    /// file described by `current`. A filesystem that cannot report mtime
    /// (`modified` is `None` on either side) never matches: reloading a
    /// model needlessly is cheap, serving a stale one is wrong.
    fn matches(&self, current: &Self) -> bool {
        match (self.modified, current.modified) {
            (Some(a), Some(b)) => self.path == current.path && self.len == current.len && a == b,
            _ => false,
        }
    }
}

/// The most recently loaded model, kept resident between tracks and between
/// meetings.
///
/// Loading a ggml model reads hundreds of megabytes to gigabytes from disk
/// and was previously done once *per track* — a two-track meeting paid it
/// twice, back to back, for the same file. The cache holds exactly one
/// context (the app has exactly one configured model at a time), so the
/// steady-state memory cost is the model the user chose to run locally,
/// which they already accepted by choosing the local route. A changed or
/// replaced model file is detected by [`ModelIdentity`] and reloaded.
static MODEL_CACHE: Mutex<Option<(ModelIdentity, Arc<WhisperContext>)>> = Mutex::new(None);

/// Returns a context for the model at `model_path`, loading it only if the
/// cache does not already hold this exact file.
fn cached_context(model_path: &Path) -> Result<Arc<WhisperContext>, TranscribeError> {
    let identity = ModelIdentity::of(model_path).map_err(|_| TranscribeError::ModelMissing {
        model: model_path.display().to_string(),
    })?;

    let mut cache = MODEL_CACHE.lock().unwrap_or_else(|poisoned| {
        // A panic while holding the lock can only have happened between
        // plain assignments; the Option inside is still coherent. Clearing
        // it below on miss makes recovery safe either way.
        poisoned.into_inner()
    });
    if let Some((cached_identity, ctx)) = cache.as_ref() {
        if cached_identity.matches(&identity) {
            return Ok(Arc::clone(ctx));
        }
    }
    // Drop the stale context before loading the new one, so two models are
    // never resident at once.
    *cache = None;
    let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
        .map_err(|err| TranscribeError::LocalEngine(format!("failed to load model: {err}")))?;
    let ctx = Arc::new(ctx);
    *cache = Some((identity, Arc::clone(&ctx)));
    Ok(ctx)
}

/// Whether `samples` carries anything above the noise floor.
///
/// Used twice: once over the whole input, to skip whisper entirely on a
/// silent recording, and once per segment, to decide whether that segment's
/// own slice of the original audio had anything in it worth trusting the
/// text whisper attached to it.
fn has_signal(samples: &[f32]) -> bool {
    if samples.is_empty() {
        return false;
    }
    let sum_squares: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    let rms = (sum_squares / samples.len() as f64).sqrt();
    rms as f32 > SILENCE_RMS_THRESHOLD
}

/// One whisper segment in its raw, whisper.cpp-native shape: timestamps in
/// centiseconds, text and no-speech confidence exactly as whisper produced
/// them. Exists so [`map_segment`] can be exercised with hand-built,
/// whisper-style values in tests, without a real whisper run.
#[derive(Debug, Clone, PartialEq)]
struct RawSegment {
    start_centiseconds: i64,
    end_centiseconds: i64,
    text: String,
    no_speech_probability: f32,
}

/// Maps one raw whisper segment onto the contract's [`Segment`], or drops it
/// entirely.
///
/// A segment is dropped — never kept with empty or placeholder text — when
/// any of: its text is blank after trimming; whisper itself was not
/// confident the audio contained speech
/// ([`NO_SPEECH_PROB_THRESHOLD`]); or the segment's own slice of `samples`
/// has no signal ([`has_signal`]). That last check is the one that matters
/// most: it is exactly the case where whisper stays "confident" (a low
/// no-speech probability) while inventing fluent text over a quiet stretch
/// it was fed, which the no-speech-probability check alone would miss.
fn map_segment(raw: &RawSegment, samples: &[f32], sample_rate: u32) -> Option<Segment> {
    let text = raw.text.trim();
    if text.is_empty() {
        return None;
    }
    if raw.no_speech_probability >= NO_SPEECH_PROB_THRESHOLD {
        return None;
    }

    let start_secs = raw.start_centiseconds as f64 / CENTISECONDS_PER_SECOND;
    let end_secs = raw.end_centiseconds as f64 / CENTISECONDS_PER_SECOND;

    let window = segment_sample_window(samples, sample_rate, start_secs, end_secs);
    if !has_signal(window) {
        return None;
    }

    Some(Segment {
        start: start_secs,
        end: end_secs,
        text: text.to_owned(),
        track: None,
        speaker: None,
    })
}

/// The slice of `samples` a segment's `[start_secs, end_secs)` window covers,
/// clamped to what actually exists. A segment's own timestamps are trusted
/// only as far as the samples we were actually given.
fn segment_sample_window(
    samples: &[f32],
    sample_rate: u32,
    start_secs: f64,
    end_secs: f64,
) -> &[f32] {
    let rate = sample_rate as f64;
    let start = ((start_secs * rate).round().max(0.0) as usize).min(samples.len());
    let end = ((end_secs * rate).round().max(0.0) as usize).clamp(start, samples.len());
    &samples[start..end]
}

/// Maps a whisper language id to the tag stored in [`Transcript::language`].
///
/// whisper.cpp uses a negative id for "no language decided". Every
/// non-negative id is a lookup into whisper's own static language table
/// (`whisper_lang_str`), which needs no loaded model or context to resolve —
/// it is why this half of language handling can be unit-tested on its own.
fn language_tag(lang_id: c_int) -> Option<String> {
    if lang_id < 0 {
        return None;
    }
    whisper_rs::get_lang_str(lang_id).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn sine(seconds: f64, freq: f64) -> Vec<f32> {
        let n = (TARGET_SAMPLE_RATE as f64 * seconds) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / TARGET_SAMPLE_RATE as f64;
                (2.0 * PI as f64 * freq * t).sin() as f32 * 0.5
            })
            .collect()
    }

    fn silence(seconds: f64) -> Vec<f32> {
        vec![0.0f32; (TARGET_SAMPLE_RATE as f64 * seconds) as usize]
    }

    fn segment(
        start_secs: f64,
        end_secs: f64,
        text: &str,
        no_speech_probability: f32,
    ) -> RawSegment {
        RawSegment {
            start_centiseconds: (start_secs * CENTISECONDS_PER_SECOND) as i64,
            end_centiseconds: (end_secs * CENTISECONDS_PER_SECOND) as i64,
            text: text.to_owned(),
            no_speech_probability,
        }
    }

    // --- transcribe(): model-missing and silence guards, no model needed ---

    #[test]
    fn a_missing_model_file_is_model_missing_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.bin");
        // Non-silent input, so the silence guard doesn't short-circuit before
        // the model is even looked at.
        let samples = sine(1.0, 440.0);

        let err = transcribe(&missing, &samples, None, |_| {}, |_| {}).unwrap_err();
        assert!(matches!(err, TranscribeError::ModelMissing { .. }));
    }

    #[test]
    fn an_unreadable_model_path_is_model_missing_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        // A directory is not a model file. Note this is only a real test
        // off-Windows: there `File::open` on a directory *succeeds*, so the
        // guard has to reject it by file kind rather than by open failure.
        let not_a_file = dir.path().join("a-directory");
        std::fs::create_dir(&not_a_file).unwrap();
        let samples = sine(1.0, 440.0);

        let err = transcribe(&not_a_file, &samples, None, |_| {}, |_| {}).unwrap_err();
        assert!(matches!(err, TranscribeError::ModelMissing { .. }));
    }

    #[test]
    fn empty_input_yields_an_empty_transcript_never_an_error() {
        let dir = tempfile::tempdir().unwrap();
        // Deliberately a path that does not exist: an empty input must never
        // even look at the model.
        let missing = dir.path().join("does-not-exist.bin");

        let transcript =
            transcribe(&missing, &[], None, |_| {}, |_| {}).expect("empty input is Ok");
        assert!(transcript.segments.is_empty());
        assert_eq!(transcript.engine, Engine::Local);
    }

    #[test]
    fn all_silence_input_yields_an_empty_transcript_never_fabricated_text() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.bin");
        let samples = silence(5.0);

        let transcript =
            transcribe(&missing, &samples, None, |_| {}, |_| {}).expect("silent input is Ok");
        assert!(
            transcript.segments.is_empty(),
            "a silent recording must never produce hallucinated segments"
        );
    }

    // --- decode_threads(): the pure thread-count decision ---

    #[test]
    fn decode_threads_uses_every_physical_core() {
        assert_eq!(decode_threads(8), 8);
        assert_eq!(decode_threads(16), 16);
    }

    #[test]
    fn decode_threads_never_goes_below_one() {
        // `num_cpus::get_physical` cannot return 0, but a lie from a weird
        // VM must not become a 0-thread whisper call.
        assert_eq!(decode_threads(0), 1);
    }

    // --- has_signal(): the pure silence detector ---

    #[test]
    fn has_signal_is_false_for_empty_and_silent_input() {
        assert!(!has_signal(&[]));
        assert!(!has_signal(&silence(1.0)));
    }

    #[test]
    fn has_signal_is_true_for_audible_input() {
        assert!(has_signal(&sine(1.0, 440.0)));
    }

    // --- map_segment(): the pure segment mapper/filter ---

    #[test]
    fn a_segment_with_real_signal_and_low_no_speech_probability_is_kept() {
        let samples = sine(2.0, 440.0);
        let raw = segment(0.0, 1.0, "  hello there  ", 0.1);

        let mapped = map_segment(&raw, &samples, TARGET_SAMPLE_RATE).expect("kept");
        assert_eq!(mapped.start, 0.0);
        assert_eq!(mapped.end, 1.0);
        assert_eq!(mapped.text, "hello there");
        assert_eq!(mapped.track, None);
    }

    #[test]
    fn a_segment_whose_own_audio_window_is_silent_is_dropped_even_with_fluent_text() {
        // Exactly the documented failure mode: whisper stays "confident"
        // (low no_speech_probability) while inventing text over a quiet
        // stretch of the recording it was fed.
        let mut samples = sine(1.0, 440.0);
        samples.extend(silence(1.0));
        let hallucinated = segment(1.0, 2.0, "Thank you for watching.", 0.05);

        assert_eq!(
            map_segment(&hallucinated, &samples, TARGET_SAMPLE_RATE),
            None
        );
    }

    #[test]
    fn a_segment_with_high_no_speech_probability_is_dropped_even_over_signal() {
        let samples = sine(1.0, 440.0);
        let raw = segment(0.0, 1.0, "maybe something", 0.9);

        assert_eq!(map_segment(&raw, &samples, TARGET_SAMPLE_RATE), None);
    }

    #[test]
    fn a_blank_text_segment_is_dropped() {
        let samples = sine(1.0, 440.0);
        let raw = segment(0.0, 1.0, "   ", 0.05);

        assert_eq!(map_segment(&raw, &samples, TARGET_SAMPLE_RATE), None);
    }

    #[test]
    fn segment_timestamps_outside_the_sample_buffer_are_clamped_not_a_panic() {
        let samples = sine(1.0, 440.0);
        // A segment claiming to run well past the audio we actually have.
        let raw = segment(0.5, 10.0, "trailing off", 0.1);

        // Must not panic; the clamped window still has signal (it overlaps
        // the sine wave), so the segment is kept with its reported times.
        let mapped = map_segment(&raw, &samples, TARGET_SAMPLE_RATE).expect("kept");
        assert_eq!(mapped.end, 10.0);
    }

    // --- language_tag(): the pure id-to-tag mapper, no model needed ---

    #[test]
    fn language_tag_resolves_a_known_id_without_a_model() {
        let id = whisper_rs::get_lang_id("en").expect("whisper knows 'en'");
        assert_eq!(language_tag(id), Some("en".to_owned()));
    }

    #[test]
    fn language_tag_is_none_for_no_language_decided() {
        assert_eq!(language_tag(-1), None);
    }

    // --- tests needing a real model: run manually only ---
    //
    // These are skipped by default because they need a real ggml model on
    // disk, which CI does not have. To run them:
    //   1. Download a small model, e.g. with the app's own downloader or:
    //      curl -L -o /tmp/ggml-tiny.bin \
    //        https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin
    //   2. Set WHISPER_TEST_MODEL=/tmp/ggml-tiny.bin
    //   3. cargo test --manifest-path src-tauri/Cargo.toml -- --ignored transcribe::local

    #[test]
    #[ignore = "needs a real ggml model on disk; see comment above for how to run it"]
    fn a_real_model_transcribes_spoken_audio() {
        let model_path = std::env::var("WHISPER_TEST_MODEL")
            .expect("set WHISPER_TEST_MODEL to a downloaded ggml model path");
        let samples = sine(2.0, 440.0); // not real speech, just exercises the path
        let transcript = transcribe(Path::new(&model_path), &samples, None, |_| {}, |_| {})
            .expect("transcription against a real model succeeds");
        assert_eq!(transcript.engine, Engine::Local);
    }

    // --- preview_line(): what a live preview is allowed to show ---

    #[test]
    fn a_spoken_line_is_previewed_trimmed() {
        assert_eq!(
            preview_line("  hello there  "),
            Some("hello there".to_owned())
        );
    }

    #[test]
    fn blank_text_is_never_previewed() {
        assert_eq!(preview_line(""), None);
        assert_eq!(
            preview_line(
                "   
 "
            ),
            None
        );
    }

    #[test]
    fn whispers_bracketed_placeholders_are_never_previewed() {
        // These are whisper saying "I heard nothing"; showing them would
        // read as the meeting having said them.
        assert_eq!(preview_line("[BLANK_AUDIO]"), None);
        assert_eq!(preview_line(" (music) "), None);
    }

    #[test]
    fn a_runaway_segment_is_cut_before_it_crosses_ipc() {
        let long = "a".repeat(PREVIEW_MAX_CHARS * 3);
        let previewed = preview_line(&long).expect("still a line");
        assert_eq!(previewed.chars().count(), PREVIEW_MAX_CHARS);
    }

    #[test]
    fn the_cut_never_splits_a_character() {
        // Truncating by bytes would panic or produce invalid text; the cap
        // counts characters.
        let long = "é".repeat(PREVIEW_MAX_CHARS * 2);
        let previewed = preview_line(&long).expect("still a line");
        assert_eq!(previewed.chars().count(), PREVIEW_MAX_CHARS);
    }

    #[test]
    fn a_line_that_merely_contains_brackets_is_still_previewed() {
        assert_eq!(
            preview_line("we shipped [finally] last night"),
            Some("we shipped [finally] last night".to_owned())
        );
    }
}
