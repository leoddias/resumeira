# Resumeira

Local-first meeting notes. Resumeira records your microphone **and** the audio
your computer plays, transcribes the meeting, and turns it into a note with a
summary, the decisions taken, and action items with owners.

It works for in-person, hybrid, and fully remote meetings — no bot joins your
call. Recording and local transcription never leave your machine unless you
explicitly configure a cloud provider. Summarization currently always uses a
cloud LLM you bring your own key for — see [Network activity](#network-activity)
below before you trust it with a sensitive meeting.

> **Status: pre-release, Windows only.** Recording, transcription, and
> summarization work end to end, but the app has not been through its
> dogfood gate yet, the install is unsigned, and there is no packaged release
> to download yet. macOS and Linux are not supported. Live/streaming
> transcription does not exist — transcription and summarization run after
> you stop recording, not during the meeting. See `docs/ROADMAP.md` for the
> full milestone breakdown.

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
  Obsidian, grep them, back them up by copying the folder.
- **Your choice of transcription engine.** Transcribe locally with a
  downloaded Whisper model, or send the audio to Groq/OpenAI using your own
  key. You pick explicitly in Settings; there is no silent fallback.
- **Summarization is BYOK and always cloud, for now.** There is no local
  summarization engine yet, so every meeting's transcript is sent to
  whichever LLM provider (Anthropic, OpenAI, or Groq) you configure, using
  your own API key. A local option (Ollama) is on the backlog, not built.

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
| Notes and audio | `<notes folder>/YYYY-MM-DD-HHMM/` — holds `notes.md`, `mic.opus`, `system.opus`. Default notes folder is `~/Resumeira`, configurable in Settings. A same-minute collision gets a `-2`, `-3`, ... suffix; the meeting title lives inside `notes.md`, never in the folder or file name |
| Settings | `%APPDATA%\dev.resumeira.app\config.json` — plain JSON, no secrets, safe to open or attach to a bug report |
| Search index | `%APPDATA%\dev.resumeira.app\index.sqlite` (rebuildable from the notes on disk — delete it safely) |
| Whisper models | `%APPDATA%\dev.resumeira.app\models\` |
| API keys | Windows Credential Manager, service name `resumeira` (never written to a file, never returned to the app's UI, never logged) |

Backup = copy the notes folder. There is nothing else to export.

Audio retention is configurable: the default keeps `mic.opus`/`system.opus`
next to the note after transcription; Settings offers "delete audio after
transcription" for anyone who wants the smaller footprint.

## Network activity

Every outbound request Resumeira can make is one of the three below, built in
Rust; none of them run unless the corresponding step is reached:

1. **Cloud transcription** — if you set the transcription engine to the API
   route, the meeting's audio is sent to `api.groq.com` or `api.openai.com`'s
   Whisper endpoint, with your key in the request header.
2. **Cloud summarization** — every meeting's transcript is sent to whichever
   summary provider you configured: `api.anthropic.com`, `api.openai.com`, or
   `api.groq.com`, with your key in the request header. There is currently no
   local summarization path, so this call happens for every meeting, not only
   when you pick the cloud transcription engine.
3. **Whisper model download** — choosing the local transcription engine
   downloads the selected model (`tiny` through `large-v3-turbo`, up to
   ~1.6 GB) once from `huggingface.co/ggerganov/whisper.cpp`, verified by
   SHA-256 before it is used.

That list is exhaustive as of this version: there is no updater, no
telemetry endpoint, and no request that is not one of the three above. If you
find a fourth, it's a bug — please report it. (An auto-updater is planned per
ADR-0014 but is not implemented yet; this section will be updated when it
ships.)

## What is not built yet

- macOS and Linux support (Windows only today)
- Live/streaming transcription — transcription and summarization both run
  after you stop recording, not during the meeting
- Local (offline) summarization — summarization always calls a cloud LLM
  today (see [Network activity](#network-activity))
- A signed installer — the build is unsigned; expect a SmartScreen warning
- An auto-updater and a published release to download

See `docs/ROADMAP.md` for the full milestone breakdown.

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
