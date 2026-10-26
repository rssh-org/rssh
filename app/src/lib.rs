//! RSSH application adapters over the shared engine.

pub use rssh_core::{
    ai, commands, crypto, db, emitter, error, migration, models, redaction, resource, secret, ssh,
    state, sync, telnet_profile, terminal, CLI_VERSION,
};
#[cfg(any(windows, macos, linux))]
mod cli;
mod files;
mod platform;
#[cfg(feature = "tauri")]
#[path = "tauri/mod.rs"]
mod tauri_host;
#[cfg(feature = "tauri")]
pub use tauri_host::run;
#[cfg(all(feature = "tauri", any(ohos, test)))]
#[path = "../../ohos/src/mod.rs"]
mod ohos;
#[cfg(all(feature = "server", any(windows, macos, linux)))]
pub mod server;
