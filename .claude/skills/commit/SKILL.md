---
name: commit
description: Commit staged/pending changes with this project's commit style — single-line Conventional Commit subject, no body, no Co-Authored-By trailer. Use whenever the user asks to commit.
model: haiku
allowed-tools: Bash, PowerShell, Read, Grep, Glob
---

# Commit

Create a git commit following the Resumeira commit style. Never push.

## Style (strict)

- **Subject only** — exactly one line, no body, no description paragraph.
- **No trailers** — no `Co-Authored-By`, no `Generated with`, nothing after
  the subject.
- Conventional Commits: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`,
  `chore:`, `ci:`. Optional scope: `feat(staging): ...`
- Imperative mood, lowercase after the colon, no trailing period, ≤ 72 chars.

## Steps

1. `git status` and `git diff` (staged + unstaged) to see what's changing.
2. If nothing is staged, stage the relevant files (`git add -A` only when
   everything pending belongs in one coherent commit; otherwise add
   selectively). Never commit `.env` or secrets.
3. If the pending changes are clearly two unrelated concerns, make two
   commits, each following the style above.
4. Commit with a single `-m` flag:

   ```
   git commit -m "docs: add product plan and agent harness"
   ```

   Exactly one `-m`. No heredoc, no second `-m`, no trailer lines.

5. Show the result (`git log --oneline -1`) to the user. Do not push.
