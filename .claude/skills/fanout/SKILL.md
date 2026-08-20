---
name: fanout
description: Split the current milestone into disjoint task packets and run them in parallel — one task-worker agent per packet in its own git worktree — then integrate the results. Use when the next work has 2+ genuinely independent pieces.
---

# Fan-out — parallel packets

Protocol and rules: `docs/PARALLEL.md`. You are the orchestrator; you
decompose, spawn, integrate. You do not implement packets yourself while
workers run.

## Steps

1. **State check.** Read `docs/PROGRESS.md` (Current state, Next up) and the
   active milestone in `docs/ROADMAP.md`. Confirm both suites are green and
   the working tree is clean — never fan out from a red or dirty base.
2. **Decide it's worth it.** Need ≥2 packets that are disjoint and each ≥30
   min. Otherwise say so and just do the work sequentially. Cap: 4 concurrent.
3. **Settle contracts first.** If packets share a type or signature (audio
   frame formats, session ids, transcript/summary structs), write it (types
   only, no implementations) and commit it before spawning. Workers treat it
   as read-only.
4. **Write the packets** into `docs/TASKS.md` using the packet format:
   goal, owned globs (exclusive, disjoint — verify no glob overlaps another),
   reads, done-when with named tests, review level
   (`conventions+privacy` for anything under `src-tauri/src/audio/**`,
   `recorder.rs`, `providers/**`, `transcribe/**`, `summarize/**`,
   `storage.rs`, `secrets.rs`), status `queued`.
   Show the split to the user before spawning.
5. **Spawn** one `task-worker` per packet, all in a single message so they run
   in parallel, each with `isolation: "worktree"`. The prompt gives: packet id,
   the full packet text, the branch base, and the reminder that shared-file
   edits (`package.json`, `Cargo.toml`, `src-tauri/src/lib.rs`, ROADMAP
   checkboxes) must be *requested*, not made.
6. **Wait.** Do not poll or start other edits in the repo while workers run —
   you'll be notified as each finishes. Mark statuses in `docs/TASKS.md`.
7. **Integrate one at a time** (dependency order): rebase the worker's branch
   on `main`, run both full suites, verify the diff stayed inside owned globs,
   apply the worker's requested shared-file edits, check off the ROADMAP item.
   A packet that fails integration goes back to its worker via `SendMessage`
   with the failure output — don't fix it yourself unless it's a one-liner.
8. **Close.** Full suites once more, clear `docs/TASKS.md` § In flight, then
   run `/handoff`. Record any packet collision in the session log as a
   splitting lesson.

## Rules

- A blocked packet is a normal outcome. Report it in PROGRESS with the failing
  output; don't re-spawn the same packet a second time unchanged.
- Never let a worker's branch merge with a red suite, and never merge two
  packets in one go to "save time" — you lose the ability to attribute a break.
- If two packets need the same file, the split was wrong: re-split, don't
  coordinate at runtime.
