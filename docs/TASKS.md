# Tasks — in-flight packets

Scratch board for the current fan-out. Cleared after integration.
Format and rules: `docs/PARALLEL.md`.

Contract (read-only for all packets): `src-tauri/src/audio/mod.rs` — `Track`,
`AudioChunk`, `AudioError`, `CaptureSource`, `TrackWriter`, `ChunkConverter`,
`TARGET_SAMPLE_RATE`. Do not modify it; if it is wrong, report and stop.

## In flight — M1 (Recording core)

### T-M1-1 — Format conversion: resample + mixer
- **Goal:** turn any captured chunk into 16 kHz mono, and combine the two
  tracks into the single buffer the transcriber will consume.
- **Owns:** `src-tauri/src/audio/resample.rs`, `src-tauri/src/audio/mixer.rs`
- **Reads:** `src-tauri/src/audio/mod.rs`, `docs/CONVENTIONS.md`
- **Done when:**
  - `resample::to_target_mono(&AudioChunk) -> Vec<f32>` downmixes interleaved
    channels to mono (average, not "take the first channel") and resamples to
    `TARGET_SAMPLE_RATE`. Linear interpolation is acceptable; document the
    choice. Never panics: zero channels, zero rate, empty samples, and a
    sample count that isn't a multiple of `channels` all return sensibly.
  - `mixer::mix_tracks(mic: &[f32], system: &[f32]) -> Vec<f32>` sums two
    already-converted mono streams of possibly different lengths (result is
    the longer one), attenuating so the result stays within `[-1.0, 1.0]`
    without hard clipping artifacts.
  - Tests: a 440 Hz sine at 48 kHz stays ~440 Hz after conversion (assert via
    zero-crossing count, tolerance stated); output length ≈ `input_len *
    16000 / 48000`; stereo L=+1.0/R=-1.0 downmixes to ~0.0; passthrough when
    the input is already 16 kHz mono; every malformed-input case above;
    mixer length, mixer sum, mixer bounds, mixer with one empty side.
- **Review:** conventions+privacy
- **Status:** merged

### T-M1-2 — Opus track writer
- **Goal:** persist one track incrementally as Ogg/Opus, so a crash mid-meeting
  costs seconds rather than the meeting.
- **Owns:** `src-tauri/src/audio/encoder.rs`
- **Reads:** `src-tauri/src/audio/mod.rs`, `docs/CONVENTIONS.md`
- **Done when:**
  - `OpusTrackWriter::create(path: &Path) -> Result<Self, AudioError>`
    implements `TrackWriter` for 16 kHz mono, targeting ~24 kbps VOIP.
  - Input is buffered into whole 20 ms frames (320 samples); a `write` call
    with a partial frame keeps the remainder for the next call. `finish`
    pads the final partial frame with silence and writes the end-of-stream
    page.
  - Pages are flushed to disk as they complete, not held until `finish`.
  - Uses the `opus` and `ogg` crates already in `Cargo.toml`. Emits a valid
    `OpusHead` and `OpusTags` header per RFC 7845.
  - Tests (temp dir, synthetic buffers, no hardware): the file begins with a
    well-formed `OpusHead` (channel count, pre-skip, input sample rate);
    writing 1 s of a sine in odd-sized calls produces the same bytes as one
    call with the same total samples; a file written and then dropped without
    `finish` still contains the pages flushed so far; 60 s of audio lands
    within a stated byte budget (~24 kbps ±50%); `write` after an I/O failure
    returns `AudioError::Io` rather than panicking.
- **Review:** conventions+privacy
- **Status:** running

### T-M1-3 — Capture sources (mic + system loopback)
- **Goal:** deliver real audio from the microphone and from whatever the
  machine is playing, behind `CaptureSource`.
- **Owns:** `src-tauri/src/audio/capture/**`
- **Reads:** `src-tauri/src/audio/mod.rs`, `docs/CONVENTIONS.md`
- **Done when:**
  - `capture::mic::MicCapture` captures the default input device via `cpal`.
  - `capture::system::SystemCapture` captures system output. **cpal 0.18
    already does WASAPI loopback**: building an *input* stream on an *output*
    device transparently enables loopback (see `cpal/src/host/wasapi/mod.rs`).
    Use that; do not add a new dependency. Non-Windows targets compile to a
    stub returning `AudioError::UnsupportedPlatform`.
  - All sample formats cpal can hand us (`i16`, `u16`, `f32`, and the other
    `SampleFormat` variants you encounter) are converted to `f32` in
    `[-1.0, 1.0]`; an unsupported format is an error, never a silent zero.
  - Stream errors are reported through the error callback and mapped to
    `AudioError::Stream`; a device disappearing mid-recording must not panic.
  - `stop()` is idempotent. No `unwrap`/`expect` anywhere in this packet.
  - Tests without hardware: sample-format conversion for each variant
    (including the `u16` midpoint mapping to ~0.0); config selection picking
    the closest supported config from a hand-built list; error mapping.
    Anything that needs a real device goes behind `#[ignore]` with a comment
    saying how to run it manually — but the packet is not done on ignored
    tests alone.
- **Review:** conventions+privacy
- **Status:** running

### T-M1-4 — Recording session
- **Goal:** one call starts a meeting recording, one call ends it, and both
  tracks land in a timestamped folder.
- **Owns:** `src-tauri/src/recorder.rs`
- **Reads:** `src-tauri/src/audio/mod.rs`, `docs/ARCHITECTURE.md`
- **Done when:**
  - `RecordingSession::start(...)` takes the notes root, a
    `Vec<(Track, Box<dyn CaptureSource>, Box<dyn TrackWriter>)>` and a
    `ChunkConverter`, creates the meeting folder, and pipes every chunk
    through the converter into that track's writer.
  - Folder name is `YYYY-MM-DD-HHMM` (local time, via `chrono`); a collision
    with an existing folder is resolved deterministically (documented
    suffix), never overwritten.
  - `stop()` stops every source, finishes every writer, returns the folder
    path and the per-track sample counts. Idempotent.
  - **A failure on one track stops that track and leaves the other
    recording** — this is the packet's most important test.
  - No `unwrap`/`expect`/panic on any path reachable while recording.
  - Tests use fake `CaptureSource`/`TrackWriter` implementations and
    `tempfile`: start/stop happy path writes both tracks; a writer that
    errors on the 3rd write does not stop the other track and is reported;
    a source that fails to start is reported without aborting the session;
    `stop()` twice is safe; folder-collision naming; the converter is applied
    exactly once per chunk.
- **Review:** conventions+privacy
- **Status:** merged

## Orchestrator-owned (not in any packet)

- `src-tauri/src/lib.rs` — module registration, Tauri command wiring
- `src-tauri/src/tray.rs` — tray wired to real sessions (after integration)
- `src-tauri/Cargo.toml`, `package.json`, `docs/**`
- Frontend recording state UI

## Done this fan-out

_(none yet)_
