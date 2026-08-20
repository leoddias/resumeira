# Conventions

## Code

- TypeScript strict mode; no `any` in `src/state/**` or `src/notes/**`.
- React: function components, hooks; state lives in `src/state`, views stay thin.
- Rust owns the risky work (audio, whisper, providers, storage, secrets) and is
  `clippy` clean. Pure functions wherever the logic allows — encoders, prompt
  builders, response parsers and path builders take inputs and return values,
  so they can be unit-tested without a microphone or a network.
- No `unwrap()`/`expect()` in the audio pipeline or any code reachable from a
  running recording. A panic mid-meeting loses the meeting.
- English everywhere: identifiers, comments, docs, UI strings.

## Testing (the non-negotiable part — ADR-0008)

- A change to the audio pipeline, a provider client, a prompt builder, a
  response parser, or note storage **includes unit tests in the same commit**.
  No exceptions, including "trivial" changes.
- Provider clients are tested against recorded fixtures (captured responses,
  including error bodies) — never against the live API in CI.
- Audio tests use synthetic buffers (generated sine/silence), not real
  recordings. No test may require a microphone, a speaker, or a network.
- Storage tests write into a temp directory and assert the on-disk layout;
  they must pass on a clean Windows machine.
- Run `npm test` and `cargo test` before declaring any task done; a broken
  suite blocks handoff.
- Anything that deletes user data (audio retention cleanup, note deletion)
  needs a test proving it deletes only the intended path.

## Commits

- Conventional Commits: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`,
  `chore:`, `ci:`. Scope optional: `feat(audio): ...`
- Subject line only — no body, no `Co-Authored-By` or other trailers.
  Use the `/commit` skill.
- Small, coherent commits; a milestone checkbox ≈ 1–3 commits.
- Check off the matching `docs/ROADMAP.md` item in the same commit that
  completes it.

## Privacy rules (enforced in review)

- API keys are read from the OS keychain in Rust and used in Rust. They are
  never returned over IPC, never written to a settings file, never logged.
- Logs contain metadata only: durations, byte counts, error kinds, provider
  names. Never transcript text, audio samples, note content, or key material.
- Every outbound request must be attributable to a user-configured provider,
  a model download, or the updater. A new network call needs an ADR.
- Telemetry stays off unless the user opted in explicitly, and the opt-in
  payload is documented in the README.

## Vibe-coding rules (for agents)

- Read `docs/PROGRESS.md` before writing code; update it after (see `/handoff`).
- Don't invent scope: anything not in the current milestone goes to
  ROADMAP § Backlog.
- Generated code gets the same test bar as handwritten code. If you can't
  test it, don't ship it.
- After touching `src-tauri/src/audio/**`, `recorder.rs`, `providers/**`,
  `storage.rs` or `secrets.rs`, run the `privacy-reviewer` agent before handoff.
- Never commit keys, real transcripts, or personal audio in fixtures.
