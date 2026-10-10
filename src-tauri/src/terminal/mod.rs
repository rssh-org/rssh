#[cfg(any(windows, macos, linux))]
pub mod pty;
pub mod recorder;
#[cfg(any(windows, macos, linux))]
pub mod serial;
// No mobile gate: telnet is plain TCP, so it works on every platform.
pub mod telnet;
