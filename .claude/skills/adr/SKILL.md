---
name: adr
description: Record an architecture/product decision in docs/DECISIONS.md, or supersede an existing one. Use whenever a choice is made that future sessions must not silently reverse (stack, scope, data, safety, money, naming).
---

# ADR — record a decision

Append a new entry to `docs/DECISIONS.md`. The log is append-only: existing
entries are never edited except to mark their status as superseded.

## Steps

1. Read `docs/DECISIONS.md`; take the next ADR number.
2. Check for conflicts: if an existing ADR covers the same ground, this new
   entry supersedes it — update the old entry's status line to
   `**Status:** superseded by ADR-NNNN` (that status change is the only
   permitted edit to old entries).
3. Append using the established format:

   ```markdown
   ## ADR-NNNN — <short imperative title>
   **Date:** YYYY-MM-DD · **Status:** accepted
   **Decision:** <what was decided, one or two sentences>
   **Why:** <the actual reasons, including rejected alternatives>
   **Consequences:** <what this forces or forbids> (omit if none)
   ```

4. If the decision changes scope or ordering, reflect it in
   `docs/ROADMAP.md`; if it changes structure, update `docs/ARCHITECTURE.md`;
   if it changes rules, update `docs/CONVENTIONS.md` or `CLAUDE.md`.
5. Confirm to the user: ADR number, title, and any superseded entries.

## Rules

- One decision per entry. Two decisions = two ADRs.
- "Why" must name what was given up — an ADR without rejected alternatives
  is a note, not a decision.
- Decisions made in conversation but not recorded here do not exist for the
  next session. When in doubt, record it.
