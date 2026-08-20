# Progress

## Current state

Phase: **M0 — scaffold**. The design interview is done and its outcome is
locked in `PLAN.md` + ADR-0001…0014. The harness (this docs set, skills,
agents) exists. No product code yet — the Tauri project has not been created.

Verified working: nothing yet (no code).
Merely written: all planning docs.

## Next up

1. Scaffold the Tauri 2 + React + TS project in place (M0), including oxlint,
   Prettier, strict tsconfig, Vitest with one passing test, and `cargo test`.
2. Tray icon with Start/Stop entries (no-op) + empty main window; README with
   setup and scripts.
3. GitHub Actions running lint + both suites + a Windows build.
4. Write the M1 shared contracts (audio types, session types) and commit them
   before any fan-out.
5. `/fanout` M1 into disjoint packets (resample+encoder / system capture /
   recorder+storage paths / tray+UI wiring).

## Blockers / open questions

- Rust toolchain and Tauri prerequisites (WebView2, MSVC build tools) must be
  present on this machine; verify before scaffolding.
- Opus encoding crate choice (`audiopus` vs `opus` vs `ogg` muxing) is not
  settled — decide during M1 and record an ADR if it constrains the design.

## Session log

### 2026-08-19/20 — Design interview + harness
- Ran the full `/grill-me` interview: 19 decisions across stack, platform,
  capture scope, transcription, summarization, storage, app shape, licensing,
  monetization, validation, telemetry, secrets, audio format, distribution.
- Wrote `PLAN.md` (product vision, locked decisions, v0.1 scope, safety bar).
- Ported the krakenless harness: `CLAUDE.md`, `docs/{CONVENTIONS,PARALLEL,
  ARCHITECTURE,DECISIONS,ROADMAP,PROGRESS,TASKS}.md`, skills and agents —
  adapted so the "paranoid core" is the audio pipeline, provider clients and
  storage rather than git plumbing, and `safety-reviewer` became
  `privacy-reviewer`.
- Recorded ADR-0001…0014.
- Tests: none exist yet.
