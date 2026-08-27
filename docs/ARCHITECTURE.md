# Architecture

> Status: **starting** — nothing has landed yet. The layout below is the
> target for v0.1. Diverging from this doc without an ADR is a bug.

## Big picture

```
┌────────────────────────────────────────────────────┐
│ Tauri 2 · tray icon + main window (WebView2 on Win) │
│                                                     │
│  React + TypeScript UI (Vite)                       │
│   ├─ views: Meetings · Note · Settings · Recording  │
│   ├─ state: recording session, meeting list         │
│   └─ invokes Tauri commands (IPC) — never sees keys │
├────────────────────────────────────────────────────┤
│ Rust core (all the risky work)                      │
│   ├─ audio/    capture (mic + system loopback),     │
│   │            resample, Opus encode, 2 tracks      │
│   ├─ recorder  session lifecycle, disk writer       │
│   ├─ transcribe/  local (whisper-rs) + API clients  │
│   ├─ summarize/   prompt build + LLM clients        │
│   ├─ storage   notes folder layout, SQLite index    │
│   ├─ secrets   OS keychain (keys never leave Rust)  │
│   └─ tray      start/stop, status glyph             │
├────────────────────────────────────────────────────┤
│ OS audio (WASAPI now; CoreAudio/PipeWire later)     │
│ Network: user-configured providers · model download │
│          · updater. Nothing else.                   │
└────────────────────────────────────────────────────┘
```

## Where logic lives

- **Rust holds the core.** Audio capture, encoding, whisper, provider HTTP
  calls, prompt construction, note writing and the search index are all Rust.
  Two reasons: the audio work is only possible there, and keeping the provider
  clients in Rust means **API keys never cross IPC into the WebView** — a
  compromised or injected frontend cannot read them.
- **TypeScript is the UI.** Views, state, formatting notes for display,
  settings forms. It asks Rust to start/stop a recording, to transcribe, to
  summarize, and to list/search meetings. It receives text and metadata, never
  secrets.
- Anything in Rust that could be a pure function should be one: the Opus
  encoder wrapper, the prompt builder, provider response parsers, and the
  meeting-folder path builder are all testable without hardware or network.

## Target layout

```
src/                      # React + TS
  views/
    Meetings.tsx          # list + search
    Note.tsx              # rendered note (summary + transcript)
    Settings.tsx          # routing, model, folder, retention, keys
    RecordingBar.tsx      # live state, elapsed time, levels
  state/                  # session store, meeting list store
  notes/                  # note markdown → display model (+ tests)
  ipc/                    # typed wrappers around invoke()
src-tauri/
  src/
    lib.rs                # command registration (orchestrator-owned)
    audio/
      capture_mic.rs      # cpal input stream
      capture_system.rs   # WASAPI loopback (win), stubs elsewhere
      resample.rs         # → 16 kHz mono (+ tests)
      encoder.rs          # Opus writer (+ tests)
      mixer.rs            # 2 tracks → 1 buffer for transcription (+ tests)
      level.rs            # per-track peak + decay for the UI meter (+ tests)
    recorder.rs           # session lifecycle, paths, crash-safe flush
    transcribe/
      mod.rs              # routing policy (+ tests)
      local.rs            # whisper-rs, model download/verify
      api.rs              # Groq/OpenAI Whisper clients (+ fixture tests)
    summarize/
      prompt.rs           # template → messages (+ tests)
      providers.rs        # Anthropic/OpenAI/Groq clients (+ fixture tests)
      parse.rs            # response → Summary struct (+ tests)
    storage.rs            # meeting folder layout, notes.md writer (+ tests)
    index.rs              # SQLite index for list/search (+ tests)
    secrets.rs            # keychain get/set/delete
    tray.rs
tests/                    # cross-cutting Rust integration tests
docs/                     # this harness
```

## Invariants (safety & privacy)

1. A recording in progress never panics the process: the audio path returns
   `Result`, and a failure on one track stops that track, not the session.
2. Audio is flushed to disk incrementally, so a crash costs seconds, not the
   whole meeting.
3. API keys are read from the keychain inside Rust, used, and dropped. No
   command returns a key; no key is written to a settings file or a log.
4. Transcription routing is explicit (`Local` | `Api`). There is no automatic
   fallback from local to cloud — a local failure surfaces as an error.
5. Notes are plain files the user owns. SQLite is a rebuildable index; losing
   it must never lose a note. A "rebuild index" path exists from day one.
6. Deleting user data (audio retention, note deletion) targets exactly one
   computed path, is confirmed in UI, and is covered by a test.

## Data locations

- Notes + audio: user-configured folder, default `~/Resumeira/`
  `YYYY-MM-DD-HHMM <title>/notes.md` + `mic.opus` + `system.opus`
- App config: `%APPDATA%/resumeira/config.json` (human-readable, no secrets)
- Search index: `%APPDATA%/resumeira/index.sqlite` (rebuildable)
- Whisper models: `%APPDATA%/resumeira/models/`
- API keys: OS keychain, service `resumeira`, one entry per provider
