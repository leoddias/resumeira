# Tasks — in-flight packets

Scratch board for the current fan-out. Cleared after integration.
Format and rules: `docs/PARALLEL.md`.

Contract (read-only for all packets): `src-tauri/src/audio/mod.rs` — `Track`,
`AudioChunk`, `AudioError`, `CaptureSource`, `TrackWriter`, `ChunkConverter`,
`TARGET_SAMPLE_RATE`. Do not modify it; if it is wrong, report and stop.

## In flight

### T-M1-5 — Capture the device's native format, not a forced 16 kHz
- **Goal:** both ignored hardware tests pass on a real Windows machine, so
  the recorder actually records.
- **Owns:** `src-tauri/src/audio/capture/**`
- **Reads:** `src-tauri/src/audio/mod.rs`, `src-tauri/src/audio/resample.rs`
- **Context (observed, not hypothetical):** running
  `cargo test --manifest-path src-tauri/Cargo.toml -- --ignored` fails twice.
  Loopback: `supported_input_configs()` on the default *output* device
  reports "device offers no usable input configuration". Microphone: the
  stream dies immediately with "A buffer underrun or overrun occurred".
- **Root cause to fix:** we ask the device to produce `TARGET_SAMPLE_RATE`.
  WASAPI shared mode does not negotiate — it delivers the device's mix
  format. Capture at the device's native format and let
  `resample::to_target_mono` convert, which is why that function exists.
  For loopback the config must come from the *output* side
  (`default_output_config()`), because WASAPI captures in the render format.
- **Done when:**
  - Mic uses `default_input_config()`; system loopback uses
    `default_output_config()`; neither forces a sample rate.
  - `cargo test -- --ignored` passes both hardware tests on this machine,
    with the real output pasted in the report.
  - Existing hardware-free tests still pass; `select_config` is either kept
    with a documented fallback role or removed along with its tests.
  - No `unwrap`/`expect` outside `#[cfg(test)]`.
- **Review:** conventions+privacy
- **Status:** running

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
