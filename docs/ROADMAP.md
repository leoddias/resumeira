# Roadmap

Scope authority: `PLAN.md`. Check items off as they land (with tests).
If schedule slips: cut UI polish and the local whisper path (→ v0.1.1),
never the privacy rules or the test bar.

## v0.1 — dogfood build (Windows)

### M0 — Scaffold
- [x] Tauri 2 + React + TypeScript + Vite project
- [x] oxlint + Prettier, strict tsconfig
- [x] Vitest wired (2 passing tests) + `cargo test` wired (3 passing tests)
- [x] `.gitignore` covers node/rust/tauri artifacts
- [x] GitHub Actions: lint + both suites + Windows build on push (written;
      unverified until a remote exists)
- [x] Tray icon with Start/Stop items (no-op) and an empty main window
- [x] README: setup, dev + build scripts, permissions, data locations

### M1 — Recording core
- [x] Mic capture via cpal → f32 frames (device selection, error surfacing)
- [x] System capture via WASAPI loopback, behind a `SystemCapture` trait with
      non-Windows stubs
- [x] Resample any input to 16 kHz mono (unit tests on synthetic buffers)
- [x] Opus encoder writing `.opus` incrementally (unit tests: header, flush,
      byte budget for a synthetic minute)
- [x] `recorder.rs`: start/stop session, two tracks, folder creation,
      crash-safe incremental flush (unit tests on temp dirs)
- [x] Tray Start/Stop wired to a real session; recording state pushed to UI
- [x] Mixer: two tracks → one 16 kHz mono buffer for transcription (unit tests)

### M2 — Transcription
- [x] Routing policy `Local | Api` from settings, no implicit fallback
      (unit tests covering: no key + Api selected, local model missing, …)
- [x] Groq + OpenAI Whisper clients (fixture tests incl. error bodies,
      chunking for long meetings)
- [x] whisper-rs local batch transcription with progress events
- [x] Model manager: download `large-v3-turbo` with progress + checksum,
      store in appdata, allow smaller models (unit tests for path/verify)
- [x] Transcript model with timestamps + track attribution

### M3 — Notes & summary
- [x] Prompt builder: fixed template → title, ~5 bullets, decisions, action
      items with owners; answers in the transcript's language (unit tests)
- [x] Anthropic/OpenAI/Groq chat clients (fixture tests incl. error bodies)
- [x] Response parser → `Summary` struct, tolerant of formatting drift (tests)
- [x] `storage.rs`: meeting folder layout + `notes.md` writer (summary,
      divider, transcript) (unit tests on temp dirs)
- [x] SQLite index: upsert on write, list, full-text search, rebuild-from-disk
      (unit tests)
- [x] Audio retention setting: keep (default) or delete after transcription
      (test proving only the intended path is deleted)

### M4 — UI
- [x] Meetings list with search, sorted by date
- [x] Note view: rendered summary + transcript, "open folder", re-run summary
- [x] Recording bar: elapsed time, per-track level, stop
- [x] Settings: notes folder, routing, whisper model, summary provider, key
      entry (write-only, masked, "test key"), audio retention
- [x] Loading / empty / error states for every panel
- [x] First-run onboarding: mic permission, folder choice, key or local model

### M5 — Polish + dogfood gate
- [x] Error surfaces for the failure modes that matter: no mic, no loopback
      device, disk full, provider 401/429, model download interrupted
- [x] `tauri-plugin-updater` + GitHub Releases workflow (unsigned)
- [x] README: permissions, install, SmartScreen note, data locations, backup,
      limitations, telemetry statement
- [ ] **Gate:** use Resumeira for every meeting for 2 weeks

## Validation checkpoint (after M5)

2–4 weeks solo dogfood → installer to 5–10 friendly users → if people reopen
notes unprompted after 2 more weeks, proceed to v0.2. Else: keep as personal
tool, stop investing.

## v0.2 — only if the checkpoint passes

- Meeting auto-detection (Zoom/Meet/Teams running → prompt to record) — the
  highest-value retention feature; "I forgot to hit record" is churn #1
- Real-time streaming transcription with live notes during the meeting
- Diarization from the two tracks ("you" vs "them"), then speaker labels
- macOS build (ScreenCaptureKit capture + permissions) — then Linux (PipeWire)
- Editable summary templates / per-meeting-type prompts
- Code-signing certificate for a public launch

## Backlog (ideas parking lot — not scheduled)

- Local LLM summaries via Ollama
- Calendar integration for meeting titles and attendees
- Export to Notion/Obsidian/Slack
- Cross-device sync and team sharing (first paid feature)
- Transcription proxy with bundled credits (the paid tier's backend)
- i18n of the UI (pt-BR first)
- Search across transcripts with semantic ranking
- Per-meeting audio bookmarks / clip extraction
- **A running `CaptureSource` has no error channel back to the session.** A
  device disconnected mid-meeting can only be logged from cpal's error
  callback, so the UI keeps showing that track as live. Needs either a
  chunk-liveness timeout or an ADR adding an error path to the trait —
  decide before the dogfood gate, since "my headphones died and Resumeira
  said it was still recording" is a trust-breaking bug.
- Band-limited (sinc) resampling — M1's linear interpolation has no
  anti-aliasing filter; fine for speech into Whisper, revisit if audio is
  ever reused for anything else
- Transcription: chunk audio over the provider's 25 MB limit instead of
  refusing it
- When the Tauri command wiring lands, keep audio paths built from generated
  ids, never from a user-typed meeting title, so a title cannot reach a log

## Manual step: enabling auto-update

The release workflow (`.github/workflows/release.yml`) is in place, but the
updater plugin is deliberately **not** wired yet, because it needs a keypair
that only a human can create and store. Doing it half-way — a placeholder
public key committed to `tauri.conf.json` — would ship a config that fails at
build or, worse, an updater that cannot verify what it downloads.

To enable it:

1. `npm run tauri signer generate -- -w resumeira.key` (keep the file out of
   the repo; `.gitignore` already excludes `*.local`, so name it accordingly
   or store it outside the tree).
2. Put the **private** key's contents in the repository secret
   `TAURI_SIGNING_PRIVATE_KEY`, and its password in
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
3. Put the **public** key into `src-tauri/tauri.conf.json`:
   ```json
   "plugins": {
     "updater": {
       "active": true,
       "endpoints": ["https://github.com/leoddias/resumeira/releases/latest/download/latest.json"],
       "pubkey": "<the public key>"
     }
   }
   ```
4. Add `tauri-plugin-updater = "2"` to `src-tauri/Cargo.toml`,
   `.plugin(tauri_plugin_updater::Builder::new().build())` to the builder
   chain in `src-tauri/src/lib.rs`, and `"updater:default"` to
   `src-tauri/capabilities/default.json`.
5. Push a `v*` tag and confirm the release publishes with `latest.json` and
   the `.sig` artifacts attached.

Until step 5 has actually run once, treat the release pipeline as untested:
the workflow's YAML, action versions and build commands were verified, but
no tag has ever been pushed through it.

## Verified against real hardware

`cargo test --manifest-path src-tauri/Cargo.toml --test end_to_end -- --ignored --nocapture`
records three seconds from the real microphone and loopback device, encodes
to Opus, decodes it back, and drives the real pipeline to a written note
(transcription and summarization stubbed, since a model is 1.6 GB and an LLM
needs a key).

It exists because unit tests have now hidden three separate app-breaking
bugs: capture forcing a sample rate no device offered, a first-error rule
that would have killed the microphone every meeting, and track writers being
opened one directory above the meeting folder so every meeting reported "no
audio was recorded". None of the three was visible to a green suite.

Run it after any change to capture, encoding, the recorder, or the pipeline.

## Backlog found while hardening (2026-08-20)

- **Deleting a meeting has no index path.** `index.rs` can add and rebuild but
  cannot remove a single note, and content now lives in two tables (`notes`
  and `notes_fts`). Whatever adds "delete a meeting" must clear both, or a
  deleted meeting's transcript lingers in `index.sqlite` after the user
  believes it is gone — a privacy problem, not just a tidiness one.
- `index.sqlite` now holds meeting text twice (the table and its FTS5 mirror).
  Worth a line in ADR-0007 for anyone reasoning about where content sits on
  disk, especially when answering "what do I delete to erase a meeting?".
- `write_note` fsyncs the note's bytes but not the parent directory entry, so
  the rename itself is not crash-consistent on every filesystem. The
  guarantee that matters — an existing note survives a failed write — holds
  regardless.
- The end-to-end hardware test has only ever run against a silent microphone
  (peak amplitude 0.0) and an idle loopback device. Run it once while playing
  audio and speaking, to prove audible sound reaches the file.
