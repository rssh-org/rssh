//! HarmonyOS host adaptation. Transport and storage domains remain shared.

#[cfg(unix)]
mod file_descriptor;
mod file_grants;
mod file_name;

#[cfg(target_env = "ohos")]
pub mod clipboard;
#[cfg(target_env = "ohos")]
pub mod device;
#[cfg(target_env = "ohos")]
pub mod files;
#[cfg(target_env = "ohos")]
mod runtime;
#[cfg(target_env = "ohos")]
pub use runtime::*;

#[cfg(target_env = "ohos")]
pub mod serial;
