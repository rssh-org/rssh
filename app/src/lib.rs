//! RSSH application adapters over the shared engine.

pub use rssh_core::{
    ai, commands, crypto, db, emitter, error, migration, models, redaction, resource, secret, ssh,
    state, sync, telnet_profile, terminal, CLI_VERSION,
};
#[cfg(any(windows, macos, linux))]
mod cli;
pub mod files;
pub mod platform;
#[cfg(feature = "tauri")]
#[path = "tauri/mod.rs"]
mod tauri_host;
#[cfg(all(feature = "tauri", not(ohos)))]
pub use tauri_host::run;
#[cfg(feature = "tauri")]
pub use tauri_host::run_with;
#[cfg(all(feature = "server", any(windows, macos, linux)))]
pub mod server;
