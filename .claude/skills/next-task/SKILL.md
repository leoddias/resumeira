---
name: next-task
description: Resume work — load project state and start the next unblocked task. Use at session start, after a handoff, or when the user says "continue" / "what's next" without specifying a task.
---

# Next task — resume work

Bootstrap a working session from the persisted state, then actually start
the work (don't just report a plan).

## Steps

1. Read `docs/PROGRESS.md`: *Current state*, *Next up*, *Blockers*.
2. Read the active milestone section in `docs/ROADMAP.md` (the first
   milestone with unchecked items).
3. Sanity-check reality against the docs (cheap checks only):
   - `git status` / `git log --oneline -5` — uncommitted or unrecorded work?
   - If code exists: does the test suite pass? A red suite preempts any new
     task.
4. Pick the task: first item in *Next up* that isn't blocked. If PROGRESS and
   ROADMAP disagree, PROGRESS wins — and fix ROADMAP as part of the session.
5. Announce in one short paragraph: what you're picking up, why it's next,
   and the definition of done (including which tests must exist/pass).
6. Do the work, honoring `docs/CONVENTIONS.md` (especially: audio, providers, storage
   ship with tests in the same change).
7. When done or stopping: run `/handoff`.

## Rules

- Never start a task from milestone N+1 while milestone N has unchecked
  items, unless PROGRESS explicitly says it's blocked.
- If *Next up* is empty or stale, reconstruct it from ROADMAP and say so in
  the session log.
- Discovered-but-out-of-scope work goes to ROADMAP § Backlog, not into the
  current change.
