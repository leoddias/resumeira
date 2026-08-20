# Parallel work protocol

How several agents work on Resumeira at the same time without corrupting each
other's work, and how each unit of work is driven to *done* (built, tested,
reviewed) instead of *written*.

Two mechanisms, used together:

- **Fan-out** (`/fanout`): the orchestrator splits a milestone into disjoint
  task packets and runs one worker per packet in its own git worktree.
- **Loop engineering** (`/task-loop`): every packet is executed as a closed
  loop — implement → test → review → fix — that exits only on green, or
  escalates.

## Roles

| Role | Who | Does |
|---|---|---|
| Orchestrator | the main session | decomposes, spawns, integrates, handles conflicts, runs `/handoff` |
| Worker | `task-worker` agent, one per packet | executes one packet's loop to green inside its worktree |
| Reviewer | `conventions-reviewer`, `privacy-reviewer` agents | read-only verdicts inside the loop |

Workers never spawn workers. Only the orchestrator merges.

## Task packet

A packet is the unit of parallelism. It lives in `docs/TASKS.md` while active.

```
### T-<milestone>-<n> — <short title>
- **Goal:** one sentence, observable outcome
- **Owns:** file globs this packet may create/modify (exclusive)
- **Reads:** globs it may read but must not modify
- **Done when:** bullet list of acceptance criteria, incl. which tests exist and pass
- **Review:** conventions | conventions+privacy (privacy required if it touches
  `src-tauri/src/audio/**`, `recorder.rs`, `providers/**`, `storage.rs`, `secrets.rs`)
- **Status:** queued | running | needs-review | blocked | merged
```

### Splitting rules

1. **Disjoint ownership.** Two active packets must not share an owned glob.
   Shared files (`package.json`, `Cargo.toml`, `src-tauri/src/lib.rs`,
   `src/state/**`, barrel/index files, ROADMAP checkboxes) belong to the
   orchestrator — a worker requests the edit in its report instead of making it.
2. **Vertical, not layered.** Prefer "the Opus encoder + its tests + its types"
   over "all of audio" / "all tests". A packet that can't be tested alone is
   too thin.
3. **Contracts first.** If packets depend on a shared type or function
   signature, the orchestrator writes that contract (types only) and commits it
   *before* fan-out. Workers treat it as read-only.
4. **Cap at 4 concurrent packets.** More than that costs more integration than
   it saves.
5. **No packet crosses a milestone boundary.**

## Isolation

Each worker runs with `isolation: "worktree"` — its own checkout, its own
branch, no interference with the user's working tree. A worker must not:
`git push`, touch the user's other worktrees, rewrite `main`, or commit
outside its owned globs.

## The loop (per packet)

```
1 plan     read PROGRESS/CONVENTIONS + packet; state the definition of done
2 build    implement the smallest complete slice
3 test     run both suites; risky-core code ships tests in the same change
4 review   conventions-reviewer (+ privacy-reviewer when the packet says so)
5 fix      address every critical/major finding
6 gate     tests green AND no unresolved critical/major → exit; else back to 2
```

- **Iteration cap: 3 passes through the gate.** Still red on the 4th? Stop,
  set `Status: blocked`, report what fails and the shortest hypothesis. A
  looping agent that can't converge is a design problem, not an effort problem.
- **Never** weaken, skip, or delete a test to make the gate pass. Deleting a
  test is a blocking finding by itself.
- Each pass ends with a commit in the worker's branch (Conventional Commits,
  subject only).

## Integration (orchestrator)

Merge one packet at a time, in dependency order:

1. Pull the worker's branch, rebase on `main`.
2. Run the **full** suites — not just the packet's tests.
3. Verify the diff stays inside the packet's owned globs. Anything outside is
   reverted and re-requested.
4. Apply the shared-file edits the worker asked for (ROADMAP checkboxes,
   `package.json` / `Cargo.toml` deps, command registration in `lib.rs`).
5. Conflict between two packets = a splitting bug: record it in the session
   log so the next split avoids it.

After all packets merge: full suites once more, then `/handoff`.

## When not to parallelize

Sequential work is the default. Fan out only when there are ≥2 packets that
are genuinely disjoint and each is ≥30 min of work. Scaffolding, refactors
that touch everything, and anything with an unsettled shared contract are
single-agent jobs.
