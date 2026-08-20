# Resumeira — Product Plan

Local-first meeting-notes app (a "Granola for every desktop"): records mic +
system audio, transcribes locally or via API, and turns the transcript into
structured notes with an LLM. Windows/macOS/Linux, commercial vision, open-core.

This document captures the shared understanding from the design interview
(2026-08-19). Change only via ADR.

## Product vision

- **Who it's for:** anyone in meetings — in-person, hybrid, or fully remote —
  who wants reliable notes without a cloud recorder bot joining the call.
- **Core promise:** everything that goes through your mic and speakers becomes
  a readable note on *your* machine. No accounts, no cloud storage, no
  telemetry by default.
- **Commercial posture:** open-core (AGPL-3.0). The core app — record,
  transcribe locally, summarize with your own key — is free forever. Future
  paid tier sells *convenience*: transcription/summary without a key (bundled
  credits via our proxy), cross-device sync, team sharing. We never paywall
  the local pipeline.
- **Validation before monetization:** dogfood in all of the founder's meetings
  for 2–4 weeks, then a Windows installer to 5–10 friendly users. Success
  signal = people reopen their notes unprompted (organic retention), not
  compliments.

## Locked decisions (summary — details go to docs/DECISIONS.md as ADRs)

| Area | Decision |
|---|---|
| Stack | Tauri 2 + React/TypeScript (same as krakenless); Rust side owns audio capture, whisper, storage |
| Platform order | Windows first (dogfood machine, easiest system-audio capture via WASAPI loopback); macOS next (largest Granola-style market); Linux after |
| Capture scope v1 | Mic **and** system audio from day one — without system audio, remote meetings with headphones don't work |
| Tracks | Two separate tracks (mic / system), mixed only for transcription. Preserves "you vs. them" separation for cheap diarization later |
| Audio format | Opus ~24 kbps mono 16 kHz per track (~10 MB/h). Kept next to the note by default; Settings offers "delete audio after transcription" |
| Transcription v1 | Both paths ship in v0.1: cloud API (Groq/OpenAI Whisper) **and** local batch via whisper-rs. Local default model: **large-v3-turbo** (~1.6 GB, downloaded on first use with progress UI); smaller models selectable for weak machines |
| Local vs API routing | Explicit user setting: "Local (private, slower)" vs "API (fast, needs key)". Default: API if a key exists, otherwise local. Never send audio to the cloud implicitly |
| Summarization | BYOK, multi-provider (Anthropic/OpenAI/Groq). No backend of ours in v0.1. Local LLM (Ollama) is roadmap |
| Summary output | Fixed default template in v0.1: generated meeting title, ~5-bullet summary, decisions made, action items with owners. Summary responds in the language of the transcript. Editable templates are roadmap |
| Storage | Human-readable files in a user-configurable folder (`~/Resumeira/<date-time title>/notes.md` + audio), SQLite in appdata as search/list index only — no lock-in, notes open in Obsidian etc. |
| App shape | Tray icon (Start/Stop Recording, Open) **plus** a main window: meeting list, note reader, simple search, settings |
| Secrets | API keys in the OS keychain (Windows Credential Manager / macOS Keychain / Secret Service) — never plain text |
| Language | Code, docs, UI in English. Transcription is multilingual (Whisper); summaries follow the meeting's language. i18n later |
| Telemetry | Zero by default. Minimal anonymous opt-in (crash reports / usage counts) offered explicitly; auditable because the code is open |
| License | AGPL-3.0, public repo |
| Distribution (validation phase) | GitHub Releases (.msi/.exe) + tauri-plugin-updater. No code signing yet — SmartScreen warning acceptable among friendly testers; certificate when going public |
| Harness | Full krakenless-style harness adapted: PLAN / PROGRESS / DECISIONS / ROADMAP / ARCHITECTURE / CONVENTIONS / PARALLEL + skills `/commit`, `/handoff`, `/adr`, `/next-task`, `/task-loop`, `/fanout` |

## v0.1 scope (dogfood, Windows only)

In:
- Tray Start/Stop recording; main window with meeting list, note view, basic search, settings
- Mic + system audio (WASAPI loopback), two Opus tracks
- Transcription: Groq/OpenAI Whisper API **and** local whisper-rs (batch, post-recording)
- Summary via BYOK LLM, fixed template, transcript language
- `notes.md` (summary above a divider, transcript below) + audio in the notes folder; SQLite index
- Keys in OS keychain; settings for routing, model, notes folder, audio retention

Out (roadmap, not v0.1):
- macOS and Linux support
- Real-time streaming transcription (live notes during the meeting)
- Meeting auto-detection (detect Zoom/Meet/Teams and prompt to record) — the #1 retention feature, first thing after validation
- Diarization ("you" vs "them" from the two tracks)
- Editable summary templates / per-meeting-type prompts
- Local LLM summaries (Ollama)
- Sync, sharing, accounts, payments, our transcription proxy
- Code signing, auto-detect languages UI, i18n

## Safety bar (Resumeira flavor)

- Audio capture and encoding code, and every external API call path, ship with
  unit tests in the same change.
- Never log transcripts, audio content, or API keys. Errors are logged with
  metadata only.
- Anything that deletes user data (audio cleanup, note deletion) confirms in
  UI and prefers recoverable forms.
- No network calls except: user-configured transcription/LLM APIs, model
  downloads, and the updater. All enumerable, all visible in code.
