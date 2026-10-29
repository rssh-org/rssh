//! HarmonyOS entry point and native SDK adapters over the shared application.

pub use rssh_core::error;

#[cfg(unix)]
mod file_descriptor;
mod file_grants;
mod file_name;
#[cfg(any(ohos, test))]
mod navigation;

#[cfg(ohos)]
mod native;
#[cfg(ohos)]
pub(crate) use native::{app, native_error, unavailable};
#[cfg(ohos)]
mod clipboard;
#[cfg(ohos)]
mod device;
#[cfg(ohos)]
mod external;
#[cfg(ohos)]
mod files;
#[cfg(ohos)]
mod platform;
#[cfg(ohos)]
mod pty;
#[cfg(ohos)]
mod runtime;
#[cfg(any(ohos, test))]
mod secret;
#[cfg(ohos)]
mod serial;
#[cfg(any(ohos, test))]
mod window;
