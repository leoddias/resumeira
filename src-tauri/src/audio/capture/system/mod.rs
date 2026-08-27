//! System (loopback) capture: whatever the machine is currently playing.
//!
//! There is no portable way to record a computer's own output — each OS
//! solves it somewhere else in the stack, so each gets its own backend
//! behind [`crate::audio::CaptureSource`] (ADR-0024):
//!
//! | Platform | Mechanism | Permission |
//! |---|---|---|
//! | Windows | WASAPI loopback through cpal | none |
//! | macOS | ScreenCaptureKit audio (13.0+) | Screen Recording (TCC) |
//! | Linux | the default sink's PulseAudio monitor source | none |
//!
//! The three differ in more than plumbing, and the difference is visible to
//! the user, so it is written down rather than smoothed over:
//!
//! * **Windows** captures the *default output device*. Audio playing on a
//!   different device is not recorded.
//! * **macOS** captures the whole system mix, and cannot start until the
//!   user grants Screen Recording — a permission whose name has nothing to
//!   do with audio, which is why the error says exactly where to grant it.
//! * **Linux** captures the monitor of whichever sink is default *at the
//!   moment the stream opens*. It needs PulseAudio or PipeWire; a machine
//!   running bare ALSA has no monitor source and reports no device.
//!
//! Anything else — a BSD, a platform added to Rust later — gets
//! [`unsupported`], which fails loudly at `start` rather than recording
//! silence.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::SystemCapture;
#[cfg(target_os = "macos")]
pub use macos::SystemCapture;
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub use unsupported::SystemCapture;
#[cfg(target_os = "windows")]
pub use windows::SystemCapture;
