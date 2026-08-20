# Resumeira

Local-first meeting notes. Resumeira records your microphone **and** the audio
your computer plays, transcribes the meeting, and turns it into a note with a
summary, the decisions taken, and action items with owners.

It works for in-person, hybrid, and fully remote meetings — no bot joins your
call, and nothing is uploaded unless you configure it to be.

> **Status: pre-alpha (M0).** The scaffold, tray, and window exist; recording
> does not work yet. See `docs/ROADMAP.md`.

## Why it exists

Tools in this category are macOS-only and route your meetings through someone
else's servers. Resumeira runs on Windows (macOS and Linux next), keeps your
notes as plain Markdown files you own, and can transcribe entirely offline.

- **No account.** Nothing to sign up for.
- **No telemetry.** Off by default, opt-in only, and the code is public so you
  can check.
- **Your files.** Notes are Markdown in a folder you choose — open them in
  Obsidian, grep them, back them up by copying the folder.
- **Your choice of engine.** Transcribe locally with Whisper, or with the
  Groq/OpenAI API using your own key. You pick explicitly; there is no silent
  fallback to the cloud.

## Requirements

- Windows 10/11 (macOS and Linux are on the roadmap)
- [Node.js](https://nodejs.org/) 22+
- [Rust](https://rustup.rs/) stable
- WebView2 (preinstalled on Windows 11) and the MSVC build tools

## Development

```bash
npm install          # install frontend dependencies
npm run tauri dev    # run the app (builds Rust on first launch)
```

Checks — both suites must be green before anything is considered done:

```bash
npm test                                   # frontend (Vitest)
cargo test --manifest-path src-tauri/Cargo.toml   # Rust core
npm run lint                               # oxlint
npm run format:check                       # Prettier
```

Build an installer:

```bash
npm run tauri build
```

## Permissions you will need to grant

- **Microphone access** — Windows asks the first time a recording starts
  (Settings → Privacy & security → Microphone must allow desktop apps).
- **System audio** is captured with WASAPI loopback, which needs no extra
  permission on Windows. macOS will require screen-recording permission when
  that platform lands.

Resumeira shows a visible recording indicator whenever a microphone is live.

## Where your data lives

| What | Where |
|---|---|
| Notes and audio | `~/Resumeira/YYYY-MM-DD-HHMM <title>/` (configurable) |
| Settings | `%APPDATA%/resumeira/config.json` |
| Search index | `%APPDATA%/resumeira/index.sqlite` (rebuildable — delete it safely) |
| Whisper models | `%APPDATA%/resumeira/models/` |
| API keys | Windows Credential Manager (never in a file, never in a log) |

Backup = copy the notes folder. There is nothing else to export.

## Network activity

Resumeira makes no network requests except:

1. the transcription/LLM provider you configured, with your key;
2. downloading a Whisper model when you choose the local engine;
3. checking for updates.

That list is exhaustive. If you find a fourth, it's a bug — please report it.

## License

AGPL-3.0-only. See [LICENSE](LICENSE).
