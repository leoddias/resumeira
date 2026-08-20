---
name: handoff
description: End-of-session handoff — persist the session's outcome into docs/PROGRESS.md so the next session or agent can resume with zero context loss. Use when finishing work, before context runs out, or when the user says "handoff" / "wrap up".
---

# Handoff

Update `docs/PROGRESS.md` so a fresh session can resume without re-deriving
anything. Truthfulness beats optimism: record what actually works, not what
was attempted.

## Steps

1. Run both suites (`npm test`, `cargo test`) if code changed this session.
   Record the real results.
2. Edit `docs/PROGRESS.md`:
   - **Current state:** rewrite to reflect reality now (phase, what exists,
     what's verified working vs. merely written).
   - **Next up:** concrete ordered next actions — specific enough that a new
     agent can start within 2 minutes. Reference ROADMAP items by milestone.
   - **Blockers / open questions:** anything unresolved, with what was tried.
   - **Session log:** prepend a dated entry (newest first): what was done,
     decisions made (link ADR numbers), test status, files touched at a
     module level (not exhaustive lists).
3. Check off completed items in `docs/ROADMAP.md`.
4. If any architectural decision was made or changed this session, verify it
   has an ADR in `docs/DECISIONS.md` (use `/adr` if missing).
5. If the module structure changed, update `docs/ARCHITECTURE.md`.
6. Tell the user handoff is written and show the *Next up* list. Suggest
   committing if there are uncommitted changes (do not commit unasked).

## Rules

- Never delete previous session log entries.
- If either suite is failing, the first line of *Next up* is fixing it, and
  *Current state* must say so explicitly.
- Keep *Current state* under ~15 lines; push detail into the session log.
