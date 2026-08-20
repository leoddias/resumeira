# Tasks — in-flight packets

Scratch board for the current fan-out. Cleared after integration.
Format and rules: `docs/PARALLEL.md`.

Contract (read-only for all packets): `src-tauri/src/audio/mod.rs` — `Track`,
`AudioChunk`, `AudioError`, `CaptureSource`, `TrackWriter`, `ChunkConverter`,
`TARGET_SAMPLE_RATE`. Do not modify it; if it is wrong, report and stop.

## In flight — M5 (polish and distribution)

### T-M5-1 — Release workflow and auto-update
- **Goal:** a tester downloads one installer and then receives fixes without
  being told to download anything again.
- **Owns:** `.github/workflows/release.yml`
- **Reads:** `.github/workflows/ci.yml`, `src-tauri/tauri.conf.json`,
  `src-tauri/Cargo.toml`, `package.json`
- **Done when:**
  - A tag push (`v*`) builds the Windows installer and publishes a GitHub
    Release with the updater artifacts attached.
  - `tauri-plugin-updater` is configured against that release feed. The
    plugin, its `Cargo.toml` entry, the `tauri.conf.json` block and the
    capability permission are **requested edits** in your report — those
    files are orchestrator-owned. Give their exact content.
  - The build is unsigned for now (ADR-0014). Say in the report exactly what
    a tester will see on first launch because of that, so the README can be
    accurate.
  - Signing keys are never committed. The updater's public key belongs in
    `tauri.conf.json`; the private key is a repository secret, and your
    report must state which secret names the workflow expects.
  - You cannot run this workflow here, so do not claim it passes. Verify what
    you can (YAML parses, action versions exist, commands match the ones in
    `ci.yml` that do run) and say plainly what remains unverified.
- **Review:** conventions
- **Status:** queued

### T-M5-2 — README for a stranger installing this
- **Goal:** someone who is not the author can install Resumeira, grant the
  right permissions, and understand exactly what leaves their machine.
- **Owns:** `README.md`
- **Reads:** `PLAN.md`, `docs/DECISIONS.md`, `docs/ROADMAP.md`,
  `src-tauri/src/config.rs`, `src-tauri/src/secrets.rs`, and the source
  generally — every claim must be checked against the code.
- **Done when:**
  - Install and first-run instructions for Windows, including the SmartScreen
    warning an unsigned build produces and what to click.
  - The permissions section is accurate: what Windows asks for, when, and why.
  - A data-locations table (notes, settings, index, models, keys) matching
    what `config.rs`, `secrets.rs` and `lib.rs` actually do — read them, do
    not copy the current README, which predates most of the code.
  - A **network activity** section listing every request the app can make,
    verified against the source. The current README claims a list is
    exhaustive; make that claim true or correct it.
  - A telemetry statement (ADR-0010) and a plain statement of what is not
    built yet, so nobody installs it expecting macOS or live transcription.
  - Development instructions matching the real commands, including the CMake
    and MSVC prerequisites the audio stack needs (ADR-0015/0016).
  - **Any claim you cannot verify in the source is removed rather than
    softened.** A privacy promise that is not true in code is the worst
    possible line in this file.
- **Review:** conventions
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
