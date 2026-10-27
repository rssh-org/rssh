//! Native host contracts. Business commands select operations, not platform protocols.

#[cfg(feature = "tauri")]
pub mod clipboard;
#[cfg(feature = "tauri")]
pub mod external;
pub mod runtime;
#[cfg(any(windows, macos, linux, ohos))]
#[cfg(feature = "tauri")]
pub mod window;

#[cfg(feature = "tauri")]
pub mod host;
#[cfg(all(feature = "tauri", not(ohos)))]
pub(crate) mod tauri;
