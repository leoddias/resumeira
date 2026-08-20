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
- [ ] Routing policy `Local | Api` from settings, no implicit fallback
      (unit tests covering: no key + Api selected, local model missing, …)
- [ ] Groq + OpenAI Whisper clients (fixture tests incl. error bodies,
      chunking for long meetings)
- [ ] whisper-rs local batch transcription with progress events
- [ ] Model manager: download `large-v3-turbo` with progress + checksum,
      store in appdata, allow smaller models (unit tests for path/verify)
- [ ] Transcript model with timestamps + track attribution

### M3 — Notes & summary
- [ ] Prompt builder: fixed template → title, ~5 bullets, decisions, action
      items with owners; answers in the transcript's language (unit tests)
- [ ] Anthropic/OpenAI/Groq chat clients (fixture tests incl. error bodies)
- [ ] Response parser → `Summary` struct, tolerant of formatting drift (tests)
- [ ] `storage.rs`: meeting folder layout + `notes.md` writer (summary,
      divider, transcript) (unit tests on temp dirs)
- [ ] SQLite index: upsert on write, list, full-text search, rebuild-from-disk
      (unit tests)
- [ ] Audio retention setting: keep (default) or delete after transcription
      (test proving only the intended path is deleted)

### M4 — UI
- [ ] Meetings list with search, sorted by date
- [ ] Note view: rendered summary + transcript, "open folder", re-run summary
- [ ] Recording bar: elapsed time, per-track level, stop
- [ ] Settings: notes folder, routing, whisper model, summary provider, key
      entry (write-only, masked, "test key"), audio retention
- [ ] Loading / empty / error states for every panel
- [ ] First-run onboarding: mic permission, folder choice, key or local model

### M5 — Polish + dogfood gate
- [ ] Error surfaces for the failure modes that matter: no mic, no loopback
      device, disk full, provider 401/429, model download interrupted
- [ ] `tauri-plugin-updater` + GitHub Releases workflow (unsigned)
- [ ] README: permissions, install, SmartScreen note, data locations, backup,
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
- Model download: drive `download()` end-to-end in the happy-path test (needs
  an injectable catalog/URL seam), and remove a dangling symlink at a model's
  final path instead of no-opping on delete
- Transcription: chunk audio over the provider's 25 MB limit instead of
  refusing it
- When the Tauri command wiring lands, keep audio paths built from generated
  ids, never from a user-typed meeting title, so a title cannot reach a log
