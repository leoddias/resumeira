# Tasks — in-flight packets

Scratch board for the current fan-out. Cleared after integration.
Format and rules: `docs/PARALLEL.md`.

Contract (read-only for all packets): `src-tauri/src/audio/mod.rs` — `Track`,
`AudioChunk`, `AudioError`, `CaptureSource`, `TrackWriter`, `ChunkConverter`,
`TARGET_SAMPLE_RATE`. Do not modify it; if it is wrong, report and stop.

## In flight

### T-M2-4 — Local Whisper transcription
- **Goal:** transcribe a meeting on this machine, with no network at all.
- **Owns:** `src-tauri/src/transcribe/local.rs`
- **Reads:** `src-tauri/src/transcribe/{mod,model}.rs`, `src-tauri/src/audio/decoder.rs`
- **Done when:**
  - `pub fn transcribe(model_path, samples: &[f32], language: Option<&str>, on_progress) -> Result<Transcript, TranscribeError>`
    runs whisper-rs over 16 kHz mono samples and maps its segments onto the
    contract's `Segment`/`Transcript`, setting `engine: Engine::Local`.
  - A missing or unreadable model file is `TranscribeError::ModelMissing`,
    not a panic. whisper-rs failures map to `LocalEngine`.
  - `language: None` means auto-detect, and the detected language lands in
    `Transcript::language`.
  - Progress is reported as a coarse percentage so the UI can move a bar.
  - Empty or all-silence input yields an empty `Transcript`, never an error
    and never a hallucinated segment — Whisper invents text on silence, so
    drop segments whose audio window has no signal, and test that.
  - Tests run without a model: model-missing, empty input, the segment
    mapping (given whisper-style values), and language passthrough. Any test
    needing a real model is `#[ignore]`d with a comment on how to run it.
- **Review:** conventions+privacy
- **Status:** queued

### T-M3-1 — Summary prompt and response parsing
- **Goal:** turn a transcript into the messages we send, and a model reply
  into a `Summary`.
- **Owns:** `src-tauri/src/summarize/prompt.rs`, `src-tauri/src/summarize/parse.rs`
- **Reads:** `src-tauri/src/summarize/mod.rs`, `src-tauri/src/transcribe/mod.rs`
- **Done when:**
  - `prompt::build(transcript, options) -> Vec<ChatMessage>` produces a system
    prompt and a user message. The prompt asks for a title, ~5 bullets,
    decisions taken, and action items with owners, and **instructs the model
    to answer in the language of the transcript** (ADR-0006).
  - The prompt tells the model to leave an owner absent rather than guess it,
    and to distinguish decisions actually taken from topics merely discussed.
  - Requests JSON so parsing is not prose-scraping; define the schema in the
    prompt and mirror it in `parse`.
  - `parse::parse_summary(provider, model, body) -> Result<Summary, SummarizeError>`
    is pure and tolerant of the drift real models produce: fenced ```json
    blocks, prose before or after the JSON, missing optional fields, a
    single-string field where a list was asked for.
  - A reply with no usable content is `SummarizeError::EmptySummary`, never a
    hollow note (`Summary::is_usable` exists for this).
  - Tests: the prompt names the language rule and the no-guessing rule; the
    parser handles clean JSON, fenced JSON, JSON with surrounding prose,
    missing owners, empty replies, and outright malformed replies.
- **Review:** conventions
- **Status:** merged

### T-M3-2 — Chat clients (Anthropic, OpenAI, Groq)
- **Goal:** send the prompt to the user's chosen provider and return the raw
  reply text.
- **Owns:** `src-tauri/src/summarize/providers.rs`
- **Reads:** `src-tauri/src/summarize/mod.rs`
- **Done when:**
  - `pub async fn complete(provider, api_key, model, messages) -> Result<String, SummarizeError>`
    speaks each provider's chat API: Anthropic `/v1/messages` (with the
    `anthropic-version` header and the system prompt as a top-level field),
    OpenAI and Groq `/v1/chat/completions`.
  - **The HTTP call and the response handling are separate functions**, so
    every status and body shape is testable with no network — same split as
    `transcribe/api.rs`, which you should read first and mirror.
  - Status mapping: 401/403 → `Unauthorized`, 429 → `RateLimited` honouring
    `Retry-After`, context-length errors → `TooLong`, other 4xx/5xx and
    malformed bodies → `BadResponse`, transport failure → `Network`.
  - The key never appears in a log, an error, or any `Debug` output.
  - Use the model ids from `SummaryProvider::default_model()` when the caller
    passes none. If a default looks wrong to you, report it — do not silently
    substitute another.
- **Review:** conventions+privacy
- **Status:** queued

### T-M3-3 — Note storage and the search index
- **Goal:** a meeting on disk as a Markdown note the user owns, plus a
  rebuildable index that makes listing and search fast.
- **Owns:** `src-tauri/src/storage.rs`, `src-tauri/src/index.rs`
- **Reads:** `src-tauri/src/summarize/mod.rs`, `src-tauri/src/transcribe/mod.rs`,
  `src-tauri/src/recorder.rs`
- **Done when:**
  - `storage::write_note(folder, &Summary, &Transcript) -> Result<PathBuf, ...>`
    writes `notes.md`: the summary first, then a divider, then the full
    transcript (ADR-0007). It states which engine and model produced it.
  - `storage::read_note(folder)` parses that file back into its parts, so the
    index can be rebuilt from disk alone.
  - The note is written atomically (temp file then rename): a crash must
    never leave a half-written note where a good one was.
  - `index::open(db_path)` creates the schema; `upsert`, `list` (newest
    first), `search` (full text over title, summary and transcript), and
    **`rebuild_from_disk(notes_root)`** which is the recovery path — losing
    the database must never lose a note.
  - The index write always follows a successful file write, never precedes it.
  - Tests on temp dirs: round-trip write/read; a note with unicode, emoji and
    newlines in the title; atomic write leaves the old note intact when the
    write fails; list ordering; search hits in title and in transcript;
    rebuild reconstructs an index deleted mid-life.
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
