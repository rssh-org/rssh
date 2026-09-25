#[cfg(any(windows, macos, linux))]
pub mod cli;
pub mod clipboard;
pub mod command_block;
pub mod discovery;
pub mod external;
pub mod files;
pub mod forward;
pub mod group;
pub mod lifecycle;
pub mod plugin;
pub mod profile;
#[cfg(any(windows, macos, linux, ohos))]
pub mod pty;
pub mod runtime;
#[cfg(any(windows, macos, linux, ohos))]
pub mod serial;
pub mod session;
pub mod settings;
pub mod sftp;
pub mod sync;
pub mod telnet;
pub mod update;
#[cfg(any(windows, macos, linux, ohos))]
pub mod window;
