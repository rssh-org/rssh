//! HarmonyOS host adaptation. Transport and storage domains remain shared.

#[cfg(unix)]
mod file_descriptor;
mod file_grants;
mod file_name;

#[cfg(ohos)]
pub mod clipboard;
#[cfg(ohos)]
pub mod device;
#[cfg(ohos)]
pub mod files;
#[cfg(ohos)]
mod runtime;
#[cfg(ohos)]
pub use runtime::*;

#[cfg(ohos)]
pub mod serial;
