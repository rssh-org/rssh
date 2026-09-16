#[cfg(any(desktop, target_env = "ohos"))]
pub mod pty;
pub mod recorder;
#[cfg(any(desktop, target_env = "ohos"))]
pub mod serial;
// No mobile gate: telnet is plain TCP, so it works on every platform.
pub mod telnet;
