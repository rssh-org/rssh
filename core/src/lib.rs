//! Shared RSSH business logic, storage, and connection engine.

pub mod ai;
pub mod commands;
pub mod crypto;
pub mod db;
pub mod emitter;
pub mod error;
pub mod files;
pub mod migration;
pub mod models;
pub mod redaction;
pub mod resource;
pub mod secret;
pub mod ssh;
pub mod state;
pub mod sync;
pub mod telnet_profile;
pub mod terminal;

pub use ssh::bastion;
pub const CLI_VERSION: &str = "1.0.0";

#[cfg(ohos)]
#[path = "../../ohos/src/core.rs"]
pub mod ohos;
