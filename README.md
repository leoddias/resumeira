# Resumeira

Local-first meeting notes. Resumeira records your microphone **and** the audio
your computer plays, transcribes the meeting, and turns it into a note with a
summary, the decisions taken, and action items with owners.

It is designed for in-person, hybrid, and fully remote meetings — no bot
joins your call, and nothing leaves your machine except through a path you
explicitly configured.

> **Status: pre-release, Windows only, and never yet used for a real
> meeting.** The whole path is wired: stopping a recording saves two Opus
> tracks, then transcription, summarization, note writing and indexing run in
> the background. Every part is covered by unit tests, and microphone and
> loopback capture are verified against real hardware — but nobody has sat
> through an actual meeting with it and read the note that came out. Until
> that happens, treat any claim about note *quality* as untested. The
> builds are unsigned, the macOS and Linux builds record the microphone only
> (system audio is Windows-only), and live/streaming transcription does not
> exist — see [What is not built yet](#what-is-not-built-yet).

## Why it exists

Tools in this category are macOS-only and route your meetings through someone
else's servers. Resumeira runs on Windows (macOS and Linux are on the
roadmap), keeps your notes as plain Markdown files you own, and can
transcribe entirely offline.

- **No account.** Nothing to sign up for.
- **No telemetry.** Off by default. An explicit opt-in for anonymous crash
  reports may be added later, and if it is, the exact payload will be
  documented here — nothing is sent without your consent today.
- **Your files.** Notes are Markdown in a folder you choose — open them in
  Obsidian, grep them, back them up by copying the folder. The database is
  only a search index and can be rebuilt from those files.
- **Transcription engine is a deliberate choice, not cloud-first.** You pick
  a downloaded local Whisper model or Groq/OpenAI with your own key, and
  there is no implicit fallback in either direction: a missing local model is
  an error you see, never a silent upload.
- **Summarization is BYOK, and is always cloud for now.** There is no local
  summarization engine (a local option via Ollama is on the backlog), so
  every meeting you summarize sends its transcript to whichever LLM provider
  you configure, using your own API key. If that is not acceptable to you,
  this app cannot yet write your notes.

## Requirements

Building Resumeira from source requires a full native toolchain — the audio
stack compiles libopus and whisper.cpp from C/C++.

- Windows 10/11 to *use* it; macOS and Linux build and run, but capture the
  microphone only (ADR-0003, ADR-0023)
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

Every tag published on GitHub Releases carries builds for all three
platforms (ADR-0023):

| Platform | Artefacts |
|---|---|
| Windows | `.msi`, an NSIS `.exe` installer, and a portable `.exe` that installs nothing |
| macOS (Apple Silicon) | `.dmg` |
| Linux (x86-64) | `.AppImage`, `.deb` |

**Only the Windows build is a supported product.** System-audio capture is
Windows-only (ADR-0003); the macOS and Linux builds record the microphone and
report the second track as unsupported. They exist so the port stays
compilable and honest, not because the product works there yet.

Every build is **unsigned** (ADR-0014) — no code-signing certificate has
been purchased for this validation phase. Windows SmartScreen will show
**"Windows protected your PC."**; click **"More info"**, then **"Run anyway"**.
macOS Gatekeeper will refuse the first launch until you allow the app in
*System Settings → Privacy & Security*. Both are expected for an unsigned
build from a small, auditable open-source project; neither means the
installer is broken.

There is also a landing page in `site/`, published to GitHub Pages by
`.github/workflows/pages.yml` on any push that touches it.

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
| Notes and recorded audio | `<notes folder>/YYYY-MM-DD-HHMM/` holding `notes.md`, `mic.opus` and `system.opus`. Default notes folder is `~/Resumeira`, configurable in Settings. A same-minute collision gets a `-2`, `-3`, ... suffix; a meeting title is never part of a folder or file name, only of `notes.md`'s own content |
| Settings | `%APPDATA%\dev.resumeira.app\config.json` — plain JSON, no secrets, safe to open or attach to a bug report |
| Search index | `%APPDATA%\dev.resumeira.app\index.sqlite` (rebuildable from the notes on disk — delete it safely) |
| Whisper models | `%APPDATA%\dev.resumeira.app\models\` — downloaded on request, verified by SHA-256 before use |
| API keys | Windows Credential Manager, service name `resumeira` (never written to a file, never returned to the app's UI, never logged) |

Backup = copy the notes folder. There is nothing else to export.

Audio retention is configurable in Settings: keep the tracks (the default),
or delete them once a transcript exists. Deletion happens only after the note
has been written successfully, so a failed summary can never leave you with
neither a note nor the audio.

## Network activity

Recording audio, browsing your meetings, and managing settings and API keys
all happen without a single network call. Every outbound request the app can
make is one of these three:

1. **Cloud transcription** — sending the meeting's audio to `api.groq.com` or
   `api.openai.com`'s Whisper endpoint, with your key in the request header,
   only when you have set the transcription engine to the API route.
2. **Cloud summarization** — sending a meeting's transcript to whichever
   summary provider you configured (`api.anthropic.com`, `api.openai.com`, or
   `api.groq.com`), with your key in the request header. There is no local
   summarization engine, so this happens for every meeting you summarize, not
   only when you pick the cloud transcription engine. With **Identify who
   spoke each line** on (Settings, on by default), the same transcript is sent
   to that same provider a second time, to work out who was speaking. Turning
   it off removes that second request; it adds no other destination.
3. **Whisper model download** — downloading the selected local model (`tiny`
   through `large-v3-turbo`, up to ~1.6 GB) from
   `huggingface.co/ggerganov/whisper.cpp`, verified by SHA-256 before use,
   only when you ask for that model in Settings.

There is no updater and no telemetry endpoint anywhere in the source. If you
find a fourth kind of outbound request, or find that one of the three above
fires when the app is just idling or browsing your meetings, that's a bug —
please report it. (An auto-updater is planned per ADR-0014 but is not
implemented yet; this section will be updated when it ships.)

## What is not built yet

- **Nobody has used this for a real meeting yet.** Every part is unit-tested
  and capture is verified against real hardware, but the end-to-end question
  — is the note actually worth reading? — is unproven.
- macOS and Linux support (Windows only today)
- Live/streaming transcription — transcription and summarization run after
  you stop recording, not during the meeting
- Local (offline) summarization — summarization always calls a cloud LLM
  (see [Network activity](#network-activity))
- A signed installer — the eventual build will be unsigned; expect a
  SmartScreen warning
- An auto-updater and a published release to download

See `docs/ROADMAP.md` for the full milestone breakdown.

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
