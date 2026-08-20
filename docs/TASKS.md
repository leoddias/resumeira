# Tasks — in-flight packets

Scratch board for the current fan-out. Cleared after integration.
Format and rules: `docs/PARALLEL.md`.

Contract (read-only for all packets): `src-tauri/src/audio/mod.rs` — `Track`,
`AudioChunk`, `AudioError`, `CaptureSource`, `TrackWriter`, `ChunkConverter`,
`TARGET_SAMPLE_RATE`. Do not modify it; if it is wrong, report and stop.

## In flight — M5 (making it usable by a stranger)

### T-M5-3 — Whisper model manager UI
- **Goal:** a user can install a local model without knowing what a `.bin`
  file is. Without this the local engine — the product's privacy promise —
  is reachable only by the author.
- **Owns:** `src/views/ModelManager.tsx`, `src/state/useModels.ts`
- **Reads:** `src/ipc/models.ts` (the contract), `src/views/RecordingBar.tsx`
  for the project's component/hook pattern
- **Done when:**
  - Lists every catalogue model with its display name, size (`formatSize`)
    and whether it is installed.
  - Download shows real progress from the `model-progress` event, which
    matters: `large-v3-turbo` is ~1.6 GB and a UI that just says "please
    wait" for ten minutes reads as frozen.
  - Download failure is reported with its reason and the model stays
    uninstalled — a half-downloaded model must never look ready.
  - Delete asks for confirmation first; re-downloading 1.6 GB after a misclick
    is a genuine cost.
  - An "open models folder" action, for anyone who would rather place the
    file by hand.
  - Loading, empty-progress, downloading, failed and installed states are
    tested with `vi.mock('../ipc/models', ...)`. Never call Tauri in a test.
- **Review:** conventions
- **Status:** queued

### T-M5-4 — First-run onboarding
- **Goal:** the first launch tells a stranger what to do, instead of showing
  an empty list and a tray icon.
- **Owns:** `src/views/FirstRun.tsx`, `src/state/useFirstRun.ts`
- **Reads:** `src/ipc/settings.ts`, `src/ipc/models.ts`, `src/ipc/meetings.ts`
- **Done when:**
  - Shows only when there are no meetings **and** nothing has been configured
    (no key stored and no local model installed) — a returning user must
    never see it again, and there is a test for that.
  - Explains, in three short steps: where notes will be saved, that recording
    starts from the tray, and that a transcription engine must be chosen —
    either a local model to download or an API key to paste.
  - **States plainly that summaries always call a cloud LLM with the user's
    own key**, because there is no local summarizer. Someone who installed
    this believing nothing ever leaves their machine must learn it here, not
    after their first meeting.
  - Mentions that Windows will ask for microphone permission when the first
    recording starts, so the prompt is not a surprise.
  - Offers a "skip" that dismisses it without configuring anything.
  - Tested with the IPC modules stubbed.
- **Review:** conventions
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
