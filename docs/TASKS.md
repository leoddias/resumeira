# Tasks — in-flight packets

Scratch board for the current fan-out. Cleared after integration.
Format and rules: `docs/PARALLEL.md`.

Contract (read-only for all packets): `src-tauri/src/audio/mod.rs` — `Track`,
`AudioChunk`, `AudioError`, `CaptureSource`, `TrackWriter`, `ChunkConverter`,
`TARGET_SAMPLE_RATE`. Do not modify it; if it is wrong, report and stop.

## In flight — backlog hardening

### T-B-1 — Durable note writes and better search
- **Goal:** close two known weaknesses in storage that were deliberately
  deferred, both recorded in `docs/ROADMAP.md` § Backlog.
- **Owns:** `src-tauri/src/storage.rs`, `src-tauri/src/index.rs`
- **Reads:** `docs/DECISIONS.md` (ADR-0007), `src-tauri/src/pipeline.rs`
- **Done when:**
  - `write_note` calls `sync_all()` on the temp file before the rename, so a
    hard power loss (not just a process crash) cannot lose a brand-new note.
    The existing guarantee — an old note survives a failed write — must keep
    its test and keep passing.
  - `index::search` uses SQLite FTS5 instead of `LIKE`, so results tokenize
    and rank instead of matching raw substrings. `rusqlite`'s `bundled`
    feature already includes FTS5; if a `Cargo.toml` feature change turns out
    to be needed, REQUEST it rather than making it.
  - `rebuild_from_disk` still works, and a search for a word that appears
    only in a transcript still finds the meeting. Add a test showing FTS5
    beats the old behaviour on at least one realistic query (for example,
    matching a word regardless of surrounding punctuation).
  - If FTS5 turns out to cost more than it gives here — say, if it forces a
    schema migration with no upgrade path for existing users — stop and say
    so in your report rather than shipping it. A worse search that ships is
    better than a broken index.
- **Review:** conventions+privacy
- **Status:** queued

### T-B-2 — Remove the panic path from the Anthropic client
- **Goal:** delete an `unreachable!()` that sits on a path processing network
  input, replacing it with a shape where the case cannot arise.
- **Owns:** `src-tauri/src/summarize/providers.rs`
- **Reads:** `src-tauri/src/summarize/mod.rs`, `src-tauri/src/summarize/prompt.rs`
- **Done when:**
  - `AnthropicRequest::build` no longer contains `unreachable!()` (or any
    other panic). Its own reviewer called this out: the guard is provably
    safe today only because of the order of a filter and a map, which is
    exactly the kind of reasoning that stops being true after a refactor.
  - The fix is structural, not a swallowed error: split system messages from
    conversation messages so the type makes the impossible case impossible,
    rather than mapping it to a silent default.
  - Existing tests keep passing unchanged, and a test covers the previously
    unreachable input shape.
- **Review:** conventions+privacy
- **Status:** queued

## Orchestrator-owned (not in any packet)

- `src-tauri/src/lib.rs` — module registration, Tauri command wiring
- `src-tauri/src/tray.rs` — tray wired to real sessions (after integration)
- `src-tauri/Cargo.toml`, `package.json`, `docs/**`
- Frontend recording state UI

## Done this fan-out — M1, all four merged

| Packet | Outcome |
|---|---|
| T-M1-1 resample + mixer | merged; 2 passes (privacy-reviewer caught an unbounded allocation on a bogus sample rate) |
| T-M1-2 Opus writer | merged; 1 pass |
| T-M1-3 capture sources | merged; 1 pass; confirmed cpal does WASAPI loopback natively |
| T-M1-4 recording session | merged; 1 pass; needed an orchestrator commit — the worker left its work uncommitted while waiting on a reviewer |

**Splitting lessons.** The disjoint globs held: no packet touched another's
files and no merge conflicted. Injecting `ChunkConverter` was what made
T-M1-4 independent of T-M1-1 — without it the two would have serialized.
The one gap the split created: T-M1-3 found that `CaptureSource` has no way
to report a mid-recording device loss, which T-M1-4 could not have known it
needed. Contracts should be reviewed for *failure* paths, not just happy
paths, before the next fan-out.

**Worker discipline.** T-M1-4 stopped with its work uncommitted while waiting
on a reviewer, so its branch looked empty at integration time. The next
fan-out prompt should state that committing at the end of each pass comes
*before* asking for review, not after.
