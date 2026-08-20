# Decision log (ADRs)

Append-only. To change a decision, add a new entry that supersedes the old
one (use the `/adr` skill). Format: number, date, status, decision, why,
consequences.

---

## ADR-0001 — Product identity: local-first meeting notes, commercial intent
**Date:** 2026-08-19 · **Status:** accepted
**Decision:** Resumeira records mic + system audio and produces structured
meeting notes on the user's machine. It is built as a product from day one
(open-core, paid convenience tier later), not as a personal script.
**Why:** The Granola-style category is validated but macOS-only and
cloud-bound; "works on Windows/Linux and never uploads your meeting" is an
unserved wedge. Building it as a personal tool first would bake in choices
(plain-text keys, no updater, no settings) that a product has to undo.
**Consequences:** Every milestone must end in something a stranger could
install and use, not just something the author can run.

## ADR-0002 — Stack: Tauri 2 + React + TypeScript (Vite)
**Date:** 2026-08-19 · **Status:** accepted
**Decision:** Tauri 2 shell with a Rust core and a React+TS frontend.
**Why:** The hard parts (system-audio capture, local whisper, keychain) are
native work that Rust does well via cpal/wasapi/whisper-rs, and Tauri ships a
~10 MB binary with a real tray, code signing and an updater — the shape a
commercial desktop app needs. Electron would need a native addon for the same
capture work while costing ~150 MB and more RAM. Per-platform native (Swift +
Kotlin + …) means three codebases, which contradicts fast iteration. Flutter's
desktop tray/audio story still requires native plugins. Reusing the krakenless
stack also reuses the author's fluency and this harness.
**Consequences:** Audio and provider tests run under `cargo test`; the project
carries two suites.

## ADR-0003 — Windows first; macOS then Linux
**Date:** 2026-08-19 · **Status:** accepted
**Decision:** v0.1 builds and is tested on Windows only. macOS next (largest
market for this category), Linux after.
**Why:** System-audio capture is per-OS code regardless of stack, so it must be
sequenced. Windows is the author's dogfood machine — the only platform where
the product can be used daily during validation — and WASAPI loopback is the
least painful of the three (macOS needs ScreenCaptureKit and a permission
dance; Linux needs PipeWire/PulseAudio detection).
**Consequences:** `capture_system.rs` is written behind a trait with a Windows
implementation and compile-time stubs elsewhere, so the port is additive.

## ADR-0004 — Capture mic *and* system audio from v0.1, as two tracks
**Date:** 2026-08-19 · **Status:** accepted
**Decision:** Record the microphone and the system loopback as two separate
Opus tracks, mixed only when feeding the transcriber.
**Why:** Without system audio, a remote meeting taken with headphones records
only the user — the product fails its core promise on the most common setup.
Keeping the tracks separate costs almost nothing now and preserves the
"you vs. them" signal that makes cheap diarization possible later; mixing at
capture time destroys it irreversibly.
**Consequences:** Two encoder instances and two files per meeting; the mixer
is its own tested unit.

## ADR-0005 — Both transcription paths ship in v0.1, routed explicitly
**Date:** 2026-08-19 · **Status:** accepted
**Decision:** v0.1 includes cloud transcription (Groq/OpenAI Whisper) *and*
local batch transcription (whisper-rs, default model `large-v3-turbo`,
downloaded on first use). A Settings choice — "Local (private, slower)" vs
"API (fast, needs key)" — decides which runs. Default: API when a key exists,
otherwise local. There is no implicit fallback in either direction.
**Why:** The local path *is* the pitch ("your meeting never leaves the
machine"); shipping cloud-only first would validate a different product. The
cloud path keeps the first-run experience fast and is what makes the app
usable before a 1.6 GB download finishes. Implicit fallback is rejected
outright: silently uploading a meeting because the local model failed would
be the single worst bug this product could have.
**Consequences:** This is the largest v0.1 cost and the most likely thing to
slip. If the schedule breaks, the local path moves to v0.1.1 — never the
privacy rules.

## ADR-0006 — Summaries are BYOK, multi-provider, Rust-side
**Date:** 2026-08-19 · **Status:** accepted
**Decision:** The user supplies an API key (Anthropic/OpenAI/Groq); the
transcript is summarized with a fixed default template producing a generated
title, ~5 summary bullets, decisions, and action items with owners, answered
in the transcript's language. Provider calls happen in Rust.
**Why:** BYOK means zero backend and zero per-user cost during validation.
Local LLM summaries (Ollama) are rejected for v0.1 because small local models
summarize noticeably worse, and the summary *is* the first impression.
Editable templates are retention, not validation, so they wait.
**Consequences:** Prompt building and response parsing are unit-tested pure
functions. A paid tier later replaces "your key" with "our proxy", not the
pipeline.

## ADR-0007 — Storage: user-owned Markdown + audio, SQLite as a rebuildable index
**Date:** 2026-08-19 · **Status:** accepted
**Decision:** Each meeting is a folder in a user-configured directory
(default `~/Resumeira/`) containing `notes.md` (summary above a divider,
transcript below) and the audio tracks. A SQLite database in appdata indexes
meetings for listing and search and can be rebuilt from the folder at any time.
**Why:** Files-only makes search and listing degrade as meetings accumulate;
SQLite-only locks the user's notes inside a database, which contradicts the
local-first pitch and blocks Obsidian-style workflows. Making the database
purely derived gets both properties, and the rebuild path doubles as the
recovery story.
**Consequences:** The writer is the source of truth; every index write follows
a successful file write, never the reverse. "Rebuild index" ships in v0.1.

## ADR-0008 — Testing bar: paranoid core (audio, providers, storage)
**Date:** 2026-08-19 · **Status:** accepted
**Decision:** The audio pipeline, provider clients, prompt builders, response
parsers and note storage ship unit tests in the same change. Provider tests
use recorded fixtures; audio tests use synthetic buffers. No test may require
a microphone, a speaker, or the network.
**Why:** A lost recording cannot be recreated — the meeting is over. That
makes the capture path more like a destructive git command than like UI code.
Fixtures keep the suite runnable offline and in CI, which is what makes the
bar survive contact with a schedule.
**Consequences:** Two suites (`npm test`, `cargo test`), both green before any
handoff. UI gets a light bar; no window-driving e2e in v0.1.

## ADR-0009 — Secrets in the OS keychain; never over IPC
**Date:** 2026-08-19 · **Status:** accepted
**Decision:** API keys are stored in the OS keychain (Windows Credential
Manager, macOS Keychain, Secret Service) and are read and used only inside
Rust. No Tauri command returns a key to the WebView; Settings shows presence
and a masked hint, not the value.
**Why:** A product whose pitch is privacy cannot keep credentials in a plain
`.env` next to the binary. Keeping them out of the WebView removes an entire
class of frontend-injection exfiltration.
**Consequences:** Settings can only test a key (call the provider) or replace
it, never reveal it.

## ADR-0010 — Zero telemetry by default; explicit minimal opt-in
**Date:** 2026-08-19 · **Status:** accepted
**Decision:** Nothing leaves the machine without user configuration. An
explicit opt-in may later enable anonymous crash reports and usage counts,
with the exact payload documented in the README.
**Why:** The target user checks. Because the code is AGPL and public, a claim
of "no telemetry" is auditable and therefore worth making honestly. Flying
blind on crashes during validation is the accepted cost, mitigated by a small
tester group in direct contact.

## ADR-0011 — Open-core under AGPL-3.0; the local pipeline is never paywalled
**Date:** 2026-08-19 · **Status:** accepted
**Decision:** Public AGPL repo. Free forever: record, transcribe (local or
own key), summarize with own key. Paid later: bundled credits via our proxy,
cross-device sync, sharing/teams.
**Why:** The privacy-minded audience this product targets rewards auditable
code with organic distribution, which is the cheapest marketing available at
this stage. Quotas on the free tier are rejected: in an open-source binary
they are trivially removed, so they create friction without protection.
Selling convenience rather than capability keeps the pitch honest.
**Consequences:** Anything that would require paywalling the local pipeline is
out of bounds; monetization work must attach at the edges (proxy, sync).

## ADR-0012 — Validation gate before commercial investment
**Date:** 2026-08-19 · **Status:** accepted
**Decision:** Dogfood in every meeting for 2–4 weeks, then ship a Windows
installer to 5–10 friendly users. Proceed to the commercial track only if
people reopen their notes unprompted. Otherwise keep it as a personal tool.
**Why:** Compliments are free; reopening a note is a revealed preference.
The paid tier requires infrastructure and accounts that are wasted work if
the core note isn't good enough to return to.

## ADR-0013 — Work is executed as capped loops, parallelized by worktree packets
**Date:** 2026-08-19 · **Status:** accepted
**Decision:** Every non-trivial task runs the loop build → test → review → fix
with a hard gate (green suites + no unresolved critical/major findings) and a
cap of 3 passes before it must escalate as blocked. Parallel work splits a
milestone into task packets with *disjoint owned file globs*, one `task-worker`
agent per packet in its own git worktree, integrated one at a time by the
orchestrator. Protocol: `docs/PARALLEL.md`.
**Why:** Agent-written code is cheap to produce and expensive to trust; the
bottleneck is verification, so verification belongs inside the unit of work.
Worktree isolation with exclusive file ownership makes concurrency safe
without runtime coordination — the alternative (several agents in one tree)
trades review time for merge archaeology. The cap exists because a loop that
can't converge in 3 passes signals a mis-specified task, not insufficient
effort.
**Consequences:** Splitting cost is paid up front (contracts committed before
fan-out); shared files are orchestrator-only, so workers *request* those edits.
Max 4 concurrent packets. Sequential single-agent work remains the default.

## ADR-0014 — Distribution: GitHub Releases + Tauri updater, unsigned for now
**Date:** 2026-08-19 · **Status:** accepted
**Decision:** Validation builds ship as `.msi`/`.exe` through GitHub Releases
with `tauri-plugin-updater`. No code-signing certificate yet.
**Why:** Testers must receive fixes without a "download it again" message, or
iteration speed dies. A signing certificate costs money and paperwork that
only matter once strangers install the app; among friendly testers a
SmartScreen warning is a documented step in the README.
**Consequences:** The README explains the warning. Signing is a prerequisite
for any public launch and is tracked in the roadmap.

## ADR-0015 — Opus via the `opus` crate; the build needs CMake + MSVC
**Date:** 2026-08-20 · **Status:** accepted
**Decision:** Encode with the `opus` crate (which vendors libopus through
`audiopus_sys` and builds it with CMake), muxed into Ogg with the `ogg` crate.
Capture uses `cpal`. `src-tauri/.cargo/config.toml` pins
`CMAKE_POLICY_VERSION_MINIMUM=3.5` so the vendored libopus configures under
CMake 4, and the README lists CMake and the MSVC build tools as prerequisites.
**Why:** There is no mature pure-Rust Opus *encoder*, and Opus is what makes an
hour of meeting fit in ~10 MB (ADR-0004). The alternatives were worse: FLAC is
pure Rust but ~6× larger, and WAV is ~60×. The CMake pin is needed because
libopus's bundled CMakeLists predates CMake 3.5 and CMake 4 rejects it rather
than warning — without the pin the failure is an opaque build-script panic.
**Consequences:** Contributors need CMake and a C toolchain, not just Rust.
First build is slower. If libopus ever becomes a packaging problem for
signed installers, revisit — the encoder sits behind the `TrackWriter` trait
precisely so it can be swapped.

## ADR-0016 — Cargo config lives at the repo root
**Date:** 2026-08-20 · **Status:** accepted
**Decision:** `.cargo/config.toml` sits at the repository root, not beside
`src-tauri/Cargo.toml`. Supersedes the file location named in ADR-0015; the
CMake pin itself is unchanged.
**Why:** Cargo discovers configuration by walking up from the *current
working directory*, ignoring `--manifest-path` entirely. With the file under
`src-tauri/`, `cargo test --manifest-path src-tauri/Cargo.toml` from the repo
root — how CI and every agent invokes it — never saw the pin, and the first
build on a clean `target/` failed with the opaque libopus CMake error while
subsequent builds passed from cache. A failure that only appears on fresh
clones and CI runners is exactly the kind that gets misdiagnosed for hours.
At the root, both invocation styles find it, since cwd inside `src-tauri/`
still walks up.
**Consequences:** Any future cargo configuration goes in the root file.
