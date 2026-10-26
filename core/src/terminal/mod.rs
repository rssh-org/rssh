#[cfg(any(windows, macos, linux, ohos))]
pub mod pty;
pub mod recorder;
#[cfg(any(windows, macos, linux, ohos))]
pub mod serial;
// No mobile gate: telnet is plain TCP, so it works on every platform.
pub mod telnet;
