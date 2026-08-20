# Tasks — in-flight packets

Scratch board for the current fan-out. Cleared after integration.
Format and rules: `docs/PARALLEL.md`.

Contract (read-only for all packets): `src-tauri/src/audio/mod.rs` — `Track`,
`AudioChunk`, `AudioError`, `CaptureSource`, `TrackWriter`, `ChunkConverter`,
`TARGET_SAMPLE_RATE`. Do not modify it; if it is wrong, report and stop.

## In flight

### T-M1-5 — Capture the device's native format, not a forced 16 kHz
- **Goal:** both ignored hardware tests pass on a real Windows machine, so
  the recorder actually records.
- **Owns:** `src-tauri/src/audio/capture/**`
- **Reads:** `src-tauri/src/audio/mod.rs`, `src-tauri/src/audio/resample.rs`
- **Context (observed, not hypothetical):** running
  `cargo test --manifest-path src-tauri/Cargo.toml -- --ignored` fails twice.
  Loopback: `supported_input_configs()` on the default *output* device
  reports "device offers no usable input configuration". Microphone: the
  stream dies immediately with "A buffer underrun or overrun occurred".
- **Root cause to fix:** we ask the device to produce `TARGET_SAMPLE_RATE`.
  WASAPI shared mode does not negotiate — it delivers the device's mix
  format. Capture at the device's native format and let
  `resample::to_target_mono` convert, which is why that function exists.
  For loopback the config must come from the *output* side
  (`default_output_config()`), because WASAPI captures in the render format.
- **Done when:**
  - Mic uses `default_input_config()`; system loopback uses
    `default_output_config()`; neither forces a sample rate.
  - `cargo test -- --ignored` passes both hardware tests on this machine,
    with the real output pasted in the report.
  - Existing hardware-free tests still pass; `select_config` is either kept
    with a documented fallback role or removed along with its tests.
  - No `unwrap`/`expect` outside `#[cfg(test)]`.
- **Review:** conventions+privacy
- **Status:** running

### T-M2-1 — Cloud Whisper clients (Groq + OpenAI)
- **Goal:** send a meeting's audio to the configured provider and get a
  `Transcript` back, with every failure mode mapped to a `TranscribeError`.
- **Owns:** `src-tauri/src/transcribe/api.rs`
- **Reads:** `src-tauri/src/transcribe/mod.rs`, `docs/CONVENTIONS.md`
- **Done when:**
  - `pub async fn transcribe(provider, api_key, audio_path, language) -> Result<Transcript, TranscribeError>`
    posts multipart to the provider's OpenAI-compatible
    `/audio/transcriptions` with `response_format=verbose_json` so segments
    and timestamps come back. Groq: `https://api.groq.com/openai/v1`, model
    `whisper-large-v3`. OpenAI: `https://api.openai.com/v1`, model `whisper-1`.
  - **The HTTP call and the parsing are separate functions.** Parsing is a
    pure `fn parse_response(provider, status, retry_after, body) -> Result<Transcript, TranscribeError>`
    so the whole error surface is testable with no network.
  - Status mapping: 401/403 → `Unauthorized`, 429 → `RateLimited` with the
    `Retry-After` header (default when absent, documented), 5xx and malformed
    bodies → `BadResponse`, transport failure → `Network`.
  - A file larger than the provider limit (25 MB) fails with a clear
    `BadResponse` naming the limit — chunking is backlog, not this packet.
  - The key is never logged, never included in an error, and the request is
    built so a `Debug` of it cannot print the key.
- **Review:** conventions+privacy
- **Status:** queued

### T-M2-2 — Whisper model manager
- **Goal:** download, verify and locate local Whisper models so the local
  engine has something to run.
- **Owns:** `src-tauri/src/transcribe/model.rs`
- **Reads:** `src-tauri/src/transcribe/mod.rs`, `docs/CONVENTIONS.md`
- **Done when:**
  - A catalog of models (id, display name, download URL, SHA-256, byte size)
    including the default `large-v3-turbo` and at least two smaller options
    for weak machines. Values come from the ggml-org/whisper.cpp Hugging Face
    repo; if you cannot verify a checksum offline, say so in the report
    rather than inventing one.
  - `model_path(models_root, id)`, `is_installed(...)`, and
    `verify(path, expected_sha256)` (streaming, not read-to-end).
  - `download(models_root, id, progress)` writes to a temporary file and
    renames into place **only after** the checksum matches, so an interrupted
    download can never look installed.
  - Tests (temp dirs, synthetic files, no network): path building; verify
    accepts a known-good file and rejects a corrupted one; an interrupted
    download leaves nothing that `is_installed` accepts; deleting a model
    targets only the computed path inside the models root.
- **Review:** conventions+privacy
- **Status:** queued

### T-M2-3 — Ogg/Opus decoder
- **Goal:** read a recorded track back into 16 kHz mono samples, so local
  transcription has PCM to work with. (Found while planning M2: we encode
  Opus but nothing decodes it.)
- **Owns:** `src-tauri/src/audio/decoder.rs`
- **Reads:** `src-tauri/src/audio/mod.rs`, `src-tauri/src/audio/encoder.rs`
- **Done when:**
  - `pub fn decode_opus_file(path: &Path) -> Result<Vec<f32>, AudioError>`
    reads the Ogg stream, decodes Opus packets, honours the `OpusHead`
    pre-skip, and returns mono samples at `TARGET_SAMPLE_RATE`.
  - Uses the `ogg` and `opus` crates already in `Cargo.toml`.
  - A truncated or corrupt file returns the samples decoded so far plus a
    logged warning, or an error if nothing decoded — never a panic. A
    recording that ended in a crash must still be transcribable.
  - Tests: **round-trip against `encoder::OpusTrackWriter`** — encode a known
    signal, decode it back, and assert the sample count and that the signal
    correlates with the original (Opus is lossy, so assert similarity, not
    equality; state the tolerance). Plus: a file truncated mid-page; a file
    that is not Ogg at all; an empty file.
  - You may add `src-tauri/src/audio/decoder.rs` to the module list only by
    REQUESTING the `audio/mod.rs` edit in your report — that file is
    read-only for packets.
- **Review:** conventions+privacy
- **Status:** queued

## Orchestrator-owned (not in any packet)

- `src-tauri/src/lib.rs` — module registration, Tauri command wiring
- `src-tauri/src/tray.rs` — tray wired to real sessions (after integration)
- `src-tauri/Cargo.toml`, `package.json`, `docs/**`
- Frontend recording state UI

## Done this fan-out — M1, all four merged

| Packet | Outcome |
|---|---|
| T-M1-1 resample + mixer | merged; 2 passes (privacy-reviewer caught an unbounded allocation on a bogus sample rate) |
| T-M1-2 Opus writer | merged; 1 pass |
| T-M1-3 capture sources | merged; 1 pass; confirmed cpal does WASAPI loopback natively |
| T-M1-4 recording session | merged; 1 pass; needed an orchestrator commit — the worker left its work uncommitted while waiting on a reviewer |

**Splitting lessons.** The disjoint globs held: no packet touched another's
files and no merge conflicted. Injecting `ChunkConverter` was what made
T-M1-4 independent of T-M1-1 — without it the two would have serialized.
The one gap the split created: T-M1-3 found that `CaptureSource` has no way
to report a mid-recording device loss, which T-M1-4 could not have known it
needed. Contracts should be reviewed for *failure* paths, not just happy
paths, before the next fan-out.

**Worker discipline.** T-M1-4 stopped with its work uncommitted while waiting
on a reviewer, so its branch looked empty at integration time. The next
fan-out prompt should state that committing at the end of each pass comes
*before* asking for review, not after.
