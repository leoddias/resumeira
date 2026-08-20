# Resumeira

Local-first meeting notes. Resumeira records your microphone **and** the audio
your computer plays, transcribes the meeting, and turns it into a note with a
summary, the decisions taken, and action items with owners.

It is designed for in-person, hybrid, and fully remote meetings — no bot
joins your call, and nothing leaves your machine except through a path you
explicitly configured.

> **Status: early, Windows only, recording works, transcription/summarization
> do not run automatically yet.** Starting and stopping a recording from the
> tray or the window works end to end and saves two Opus audio tracks
> (`mic.opus`, `system.opus`) to disk. The transcription engines (cloud
> Whisper via Groq/OpenAI, local via whisper-rs), the summarization clients
> (Anthropic/OpenAI/Groq), the prompt/response handling, and the note/index
> storage all exist as working, unit-tested Rust modules — but nothing in the
> shipped app calls them yet: stopping a recording does not currently produce
> a transcript or a `notes.md`. That wiring is the next milestone
> (`docs/PROGRESS.md`, `docs/ROADMAP.md` M2/M3). There is also no packaged
> release to download, the eventual build will be unsigned, macOS/Linux are
> not supported, and live/streaming transcription does not exist — see
> [What is not built yet](#what-is-not-built-yet).

## Why it exists

Tools in this category are macOS-only and route your meetings through someone
else's servers. Resumeira runs on Windows (macOS and Linux are on the
roadmap), keeps your notes as plain Markdown files you own, and can
transcribe entirely offline.

- **No account.** Nothing to sign up for.
- **No telemetry.** Off by default. An explicit opt-in for anonymous crash
  reports may be added later, and if it is, the exact payload will be
  documented here — nothing is sent without your consent today.
- **Your files.** Notes are designed to be Markdown in a folder you choose —
  open them in Obsidian, grep them, back them up by copying the folder. (The
  note writer exists and is tested; it is not yet triggered automatically
  after a recording — see the status note above.)
- **Transcription engine is a deliberate choice, not built-in cloud-first.**
  The routing logic and both clients (a downloaded local Whisper model, or
  Groq/OpenAI with your own key) exist and are tested to have no implicit
  cloud fallback. They are not yet reachable from the running app.
- **Summarization is BYOK, and will always be cloud for now.** There is no
  local summarization engine (a local option via Ollama is on the backlog,
  not built), so once this is wired up, every meeting's transcript will be
  sent to whichever LLM provider (Anthropic, OpenAI, or Groq) you configure,
  using your own API key.

## Requirements

Building Resumeira from source requires a full native toolchain — the audio
stack compiles libopus and whisper.cpp from C/C++.

- Windows 10/11 (macOS and Linux are on the roadmap, not usable today)
- [Node.js](https://nodejs.org/) 22+
- [Rust](https://rustup.rs/) stable
- **CMake** and the **MSVC C++ build tools** (Visual Studio Build Tools with
  the "Desktop development with C++" workload). Without these the first
  build fails with an opaque CMake error from the vendored libopus/whisper.cpp
  sources (ADR-0015).
- WebView2 (preinstalled on Windows 11)

The repository pins `CMAKE_POLICY_VERSION_MINIMUM=3.5` in the root
`.cargo/config.toml` so the vendored C libraries configure under CMake 4
(ADR-0015, ADR-0016). This file must stay at the repository root — cargo
discovers it by walking up from the current working directory, not from the
manifest path, so a copy next to `src-tauri/Cargo.toml` would be invisible to
`cargo test --manifest-path src-tauri/Cargo.toml` run from the repo root.

## Installing a pre-built release

There is no packaged release yet. Once one exists it will ship as an unsigned
`.msi`/`.exe` through GitHub Releases (ADR-0014) — no code-signing
certificate has been purchased for this validation phase. When you run an
unsigned installer, Windows SmartScreen will show **"Windows protected your
PC."** Click **"More info"**, then **"Run anyway"** to proceed. This is
expected for an unsigned build from a small, auditable open-source project;
it is not a sign the installer is broken.

## Development

```bash
npm install          # install frontend dependencies
npm run tauri dev    # run the app (builds Rust on first launch)
```

Checks — both suites must be green before anything is considered done:

```bash
npm test                                            # frontend (Vitest)
cargo test --manifest-path src-tauri/Cargo.toml     # Rust core
npm run lint                                        # oxlint
npm run format:check                                # Prettier
```

Build an installer:

```bash
npm run tauri build
```

## Permissions you will need to grant

- **Microphone access** — Windows asks the first time a recording starts
  (Settings → Privacy & security → Microphone must allow desktop apps).
- **System audio** is captured with WASAPI loopback (opening the default
  *output* device as an input), which needs no extra permission on Windows.
  macOS will require screen-recording permission when that platform lands;
  loopback capture has no equivalent implemented for macOS/Linux today, which
  is part of why those platforms are not supported yet.

## Where your data lives

All paths below use Resumeira's app identifier, `dev.resumeira.app`; on
Windows both the config and data directories resolve under `%APPDATA%`.

| What | Where |
|---|---|
| Recorded audio (and, once wired, notes) | `<notes folder>/YYYY-MM-DD-HHMM/` — today holds `mic.opus` and `system.opus`; the writer for `notes.md` next to them exists but is not yet called after a recording stops. Default notes folder is `~/Resumeira`, configurable in Settings. A same-minute collision gets a `-2`, `-3`, ... suffix; a meeting title is never part of the folder or file name, only of `notes.md`'s own content |
| Settings | `%APPDATA%\dev.resumeira.app\config.json` — plain JSON, no secrets, safe to open or attach to a bug report |
| Search index | `%APPDATA%\dev.resumeira.app\index.sqlite` (rebuildable from the notes on disk — delete it safely) |
| API keys | Windows Credential Manager, service name `resumeira` (never written to a file, never returned to the app's UI, never logged) |

Backup = copy the notes folder. There is nothing else to export.

Audio retention is configurable in Settings (default: keep `mic.opus`/
`system.opus` after transcription, or delete them once a transcript exists).
The setting is stored and tested, but has no observable effect yet, since
nothing currently deletes audio after a recording — see the status note
above.

Downloaded local Whisper models will need a location once the local
transcription engine is wired up; no code path in this build currently
resolves or writes to one, so no location is listed here yet.

## Network activity

**As shipped today, the running app makes zero network requests.** Recording
audio, browsing the meetings list, and managing settings and API keys all
happen without a network call. That is a consequence of the transcription and
summarization pipeline not being wired up yet (see the status note above),
not a policy the app enforces once it is.

The Rust modules for cloud calls exist, are unit-tested against recorded
fixtures, and will run once integrated. Every outbound request the code is
capable of making is one of these three, so this is the complete list to
audit as the pipeline gets wired in:

1. **Cloud transcription** — sending the meeting's audio to `api.groq.com` or
   `api.openai.com`'s Whisper endpoint, with your key in the request header,
   only when you have set the transcription engine to the API route.
2. **Cloud summarization** — sending a meeting's transcript to whichever
   summary provider you configured (`api.anthropic.com`, `api.openai.com`, or
   `api.groq.com`), with your key in the request header. There is no local
   summarization engine, so once wired up this call will happen for every
   meeting, not only when you pick the cloud transcription engine.
3. **Whisper model download** — downloading the selected local model (`tiny`
   through `large-v3-turbo`, up to ~1.6 GB) from
   `huggingface.co/ggerganov/whisper.cpp`, verified by SHA-256 before use,
   only when you choose the local transcription engine.

There is no updater and no telemetry endpoint anywhere in the source. If you
find a fourth kind of outbound request, or find that one of the three above
fires when the app is just idling or browsing your meetings, that's a bug —
please report it. (An auto-updater is planned per ADR-0014 but is not
implemented yet; this section will be updated when it ships.)

## What is not built yet

- **Stopping a recording does not produce a note.** Transcription,
  summarization, and note writing exist as tested library code but are not
  yet called after a recording stops; today a recording leaves you with
  `mic.opus`/`system.opus` and nothing else. This is the current top-of-list
  work (`docs/PROGRESS.md`).
- macOS and Linux support (Windows only today)
- Live/streaming transcription — once wired up, transcription and
  summarization will run after you stop recording, not during the meeting
- Local (offline) summarization — summarization will always call a cloud LLM
  once wired up (see [Network activity](#network-activity))
- A signed installer — the eventual build will be unsigned; expect a
  SmartScreen warning
- An auto-updater and a published release to download

See `docs/ROADMAP.md` for the full milestone breakdown.

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
