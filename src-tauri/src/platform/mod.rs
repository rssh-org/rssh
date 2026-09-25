//! Native host contracts. Business commands select operations, not platform protocols.

pub mod clipboard;
pub mod external;
pub mod runtime;
#[cfg(any(windows, macos, linux, ohos))]
pub mod window;
