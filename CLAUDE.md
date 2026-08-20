# Resumeira — Agent Harness

Local-first meeting-notes app: records mic + system audio, transcribes locally
or via API, turns the transcript into structured notes with an LLM. Tauri 2 +
React/TS. Windows first, macOS/Linux later. Open-core, AGPL-3.0.
This file is the entry point for every session — human or agent.

## Session protocol (mandatory)

1. **Start:** read `docs/PROGRESS.md` (sections *Current state* and *Next up*).
   That is the single source of truth for where work stands. Do not re-derive
   state from git history if PROGRESS.md answers it.
2. **Before architectural choices:** check `docs/DECISIONS.md`. Locked
   decisions are not renegotiated casually — changing one requires a new ADR
   entry superseding the old (use the `/adr` skill).
3. **During work:** follow `docs/CONVENTIONS.md` (style, tests, commits). Every
   non-trivial task runs through `/task-loop`; work is *done* only when both
   suites are green and review findings are resolved, never when the code is
   merely written. Parallel work follows `docs/PARALLEL.md` (`/fanout`).
4. **End of session / task done:** run the `/handoff` skill — update
   `docs/PROGRESS.md` (Current state, Next up, Session log). A session that
   doesn't update PROGRESS.md loses its context forever.

## Document map

| File | Role | Mutability |
|---|---|---|
| `PLAN.md` | Product vision & shared understanding from the design interview | Stable; change only via ADR |
| `docs/PROGRESS.md` | Living state: what's done, in flight, next; session log | Every session |
| `docs/ROADMAP.md` | Milestones M0–M5 for v0.1 with task checklists | Check off as done; reshape via ADR |
| `docs/DECISIONS.md` | ADR log — every locked decision and its why | Append-only (supersede, don't edit) |
| `docs/ARCHITECTURE.md` | Module layout, data flow, audio/transcription design | Update when structure changes |
| `docs/CONVENTIONS.md` | Code style, testing bar, commit format | Update via ADR |
| `docs/PARALLEL.md` | How agents work in parallel and how a task is looped to done | Update via ADR |
| `docs/TASKS.md` | Scratch board of in-flight task packets | Cleared after each fan-out |

## Non-negotiable rules

- **Safety bar ("paranoid core"):** any code in the audio pipeline
  (`src-tauri/src/audio/**`, `recorder.rs`), the provider clients
  (`src-tauri/src/providers/**`), or note storage (`storage.rs`) ships with
  unit tests in the same change. A dropped recording is unrecoverable — the
  user cannot re-run the meeting. See `docs/CONVENTIONS.md`.
- **Privacy is the product:** no telemetry by default; no network calls except
  the user-configured transcription/LLM APIs, model downloads, and the
  updater. Never log transcripts, audio content, or API keys. API keys live in
  the OS keychain and **never cross IPC into the WebView**.
- **Explicit routing:** audio or transcripts leave the machine only through a
  path the user explicitly configured. No implicit cloud fallback, ever.
- **Scope discipline:** v0.1 scope is fixed in `PLAN.md`. Feature ideas go to
  `docs/ROADMAP.md` § Backlog, not into the sprint.
- **Changes that alter behavior get committed with tests passing.** Run both
  suites (`npm test`, `cargo test`) before declaring anything done.
- Language: code, docs, and UI in **English**. Conversation with the user may
  be in Portuguese.

## Skills & agents available here

- `/commit` — commit in project style: single-line subject, no body, no trailers
- `/handoff` — write the end-of-session state into PROGRESS.md
- `/adr` — record or supersede a decision in DECISIONS.md
- `/next-task` — pick up the next unblocked task from ROADMAP/PROGRESS
- `/task-loop` — drive one task to done: build → test → review → fix, capped at
  3 passes, gate = green suites + no critical/major findings
- `/fanout` — split a milestone into disjoint packets and run one `task-worker`
  per packet in its own worktree, then integrate
- Agent `task-worker` — executes a single packet's loop in isolation
- Agent `conventions-reviewer` — read-only review against CONVENTIONS + the
  packet's definition of done
- Agent `privacy-reviewer` — reviews the audio pipeline, provider clients and
  storage for data loss and privacy leaks; use after changing any of them
