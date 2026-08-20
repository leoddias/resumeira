# Progress

## Current state

Phase: **M1 complete, starting M2 (transcription)**.

Verified working (both suites green: 84 Rust tests + 2 ignored hardware
tests, 21 frontend tests; clippy and fmt clean):

- Tauri 2 app scaffold, tray + main window, CI workflow, README.
- Full recording pipeline in Rust: mic capture and system loopback (cpal,
  WASAPI), format conversion to 16 kHz mono, a track mixer, an incremental
  Ogg/Opus writer, and a session that isolates per-track failures.
- App-level state machine (`session.rs`) whose JSON shape is asserted against
  the TypeScript contract in `src/ipc/types.ts`.
- Tray and window both drive the same start/stop path; the tray tooltip and
  menu reflect real state.

**Not yet verified: the app has never been launched end to end.** Everything
above is proven by unit tests and a clean compile, not by a recorded meeting.
That smoke test is the first item below and gates any claim that M1 works.

## Next up

1. **Smoke-test the real app**: `npm run tauri dev`, start a recording from
   the tray, play audio, stop, and confirm `~/Resumeira/<timestamp>/` holds a
   playable `mic.opus` and `system.opus`. Fix whatever this surfaces before
   starting M2 — no amount of unit tests substitutes for one real recording.
2. Decide the `CaptureSource` error-path question (see Blockers) — it changes
   the audio contract, so it must land before the M2 fan-out.
3. Write the M2 shared contracts (transcript model with timestamps and track
   attribution, routing policy types) and commit them before fanning out.
4. `/fanout` M2 into packets: routing policy · Groq/OpenAI Whisper clients ·
   whisper-rs local + model manager · transcript model.

## Blockers / open questions

- **`CaptureSource` has no error channel while running.** A device lost
  mid-meeting can only be logged from cpal's error callback; the session
  never learns, so the UI keeps showing that track as live. Needs either a
  chunk-liveness timeout in `recorder.rs` or an ADR adding an error path to
  the trait. Found by T-M1-3's reviewer.
- Device names are read from cpal at factory time, so a `TrackStatus` shows
  the default device's name even if capture later opens a different one.
- CI has never run (no remote yet), so the workflow is unverified.

## Session log

### 2026-08-20 — M1 recording core, four packets in parallel
- Fanned out M1 into four disjoint packets, one `task-worker` per worktree:
  resample+mixer, Opus writer, capture sources, recording session. All four
  merged; three passed their gate on the first loop, one took two.
- Integrated each packet one at a time with full suites between merges.
  Diff scope stayed inside owned globs for every packet; no conflicts.
- Orchestrator work: IPC contract + `useRecording` + `RecordingBar` on the
  frontend, `session.rs` state machine, `tracks.rs` factory, `commands.rs`,
  tray wiring.
- Fixed a real infrastructure bug: `.cargo/config.toml` lived under
  `src-tauri/`, where cargo never found it when invoked from the repo root
  (config discovery is cwd-relative, not manifest-relative), so the libopus
  CMake pin silently did not apply on clean builds. Moved to the root
  (ADR-0016).
- Recorded ADR-0015 (Opus via the `opus` crate; CMake + MSVC prerequisite)
  and ADR-0016.
- Tests: `cargo test` 84 passed / 2 ignored; `npm test` 21 passed; clippy and
  fmt clean.

### 2026-08-19/20 — Design interview + harness
- Ran the full `/grill-me` interview: 19 decisions across stack, platform,
  capture scope, transcription, summarization, storage, app shape, licensing,
  monetization, validation, telemetry, secrets, audio format, distribution.
- Wrote `PLAN.md` (product vision, locked decisions, v0.1 scope, safety bar).
- Ported the krakenless harness, adapted so the "paranoid core" is the audio
  pipeline, provider clients and storage rather than git plumbing, and
  `safety-reviewer` became `privacy-reviewer`.
- Recorded ADR-0001…0014. M0 scaffold landed: Tauri 2 + React/TS, tray, both
  suites, CI, README, AGPL-3.0.
