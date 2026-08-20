# Tasks — in-flight packets

Scratch board for the current fan-out. Cleared after integration.
Format and rules: `docs/PARALLEL.md`.

Contract (read-only for all packets): `src-tauri/src/audio/mod.rs` — `Track`,
`AudioChunk`, `AudioError`, `CaptureSource`, `TrackWriter`, `ChunkConverter`,
`TARGET_SAMPLE_RATE`. Do not modify it; if it is wrong, report and stop.

## In flight — M4 (UI)

Contract (read-only for all packets): `src/ipc/types.ts`, `src/ipc/meetings.ts`,
`src/ipc/settings.ts`. Do not modify them; if one is wrong, report and stop.
Frontend tests stub the IPC module with `vi.mock` — never call Tauri.

### T-M4-1 — Meetings list with search
- **Goal:** open the window and see every meeting, newest first, and find one.
- **Owns:** `src/views/Meetings.tsx`, `src/state/useMeetings.ts`
- **Reads:** `src/ipc/meetings.ts`, `src/views/format.ts`, `src/index.css`
- **Done when:**
  - The list shows title, date and preview per meeting, newest first, and
    selecting one calls the `onOpen` prop with its folder.
  - A search box filters via `searchMeetings`; a cleared box returns to the
    full list. Debounce so typing does not fire a query per keystroke.
  - Three honest states, all tested: loading, empty ("no meetings yet"), and
    failed (says what failed and offers a retry — never a blank list that
    looks like "you have no meetings").
  - The component takes state via props or a hook you own; IPC is stubbed in
    tests with `vi.mock('../ipc/meetings', ...)`.
- **Review:** conventions
- **Status:** queued

### T-M4-2 — Note view
- **Goal:** read a meeting: the summary first, the transcript under it.
- **Owns:** `src/views/Note.tsx`, `src/notes/**`
- **Reads:** `src/ipc/meetings.ts`, `src/views/format.ts`, `src/index.css`
- **Done when:**
  - Renders the summary Markdown (a small renderer in `src/notes/` — headings,
    lists, bold, paragraphs is enough; **add no dependency**) and the
    transcript below a divider.
  - Transcript lines show their timestamp and are visually distinguishable by
    track: the user's microphone versus everyone else. A line with no track
    still renders.
  - States the engine and model that produced it, so the note is honest about
    where it came from, and says when the audio was deleted.
  - "Open folder" calls `openMeetingFolder`.
  - Loading, empty-transcript and error states are tested. Markdown rendering
    must escape HTML in the source text — a model-generated title is
    untrusted input.
- **Review:** conventions
- **Status:** queued

### T-M4-3 — Settings screen
- **Goal:** choose where notes go, which engine transcribes, which model
  summarizes, and paste API keys.
- **Owns:** `src/views/Settings.tsx`, `src/state/useSettings.ts`
- **Reads:** `src/ipc/settings.ts`, `src/index.css`
- **Done when:**
  - Every field in `Settings` is editable and saved via `saveSettings`.
  - Key fields are **write-only**: show `configured` plus the masked `hint`
    from `KeyStatus`, never a value. A key input clears itself after saving.
  - **The screen states plainly when the current configuration will send
    audio off the machine**, using `sendsAudioToTheCloud`. This is the
    product's central promise; a user must never discover it by accident.
  - Choosing the API engine without a key for that provider warns before
    saving, naming the account from `requiredTranscriptionAccount`.
  - Loading, saving, and save-failed states are tested, plus the cloud
    warning appearing and disappearing with the engine choice.
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
