use std::sync::Arc;

/// Serial output destined for the host. Mirrors `pty::PtyOut`, but kept as its
/// own type so the serial module carries no dependency on the pty module — two
/// unrelated transports shouldn't couple just to share a 2-variant enum. The
/// Tauri command turns these into `serial:data:<id>` / `serial:close:<id>`.
pub enum SerialOut {
    Data(Vec<u8>),
    Close,
}

/// Sink the reader thread invokes for each chunk. The `&str` is the session id,
/// so one sink can serve any number of serial sessions (same shape as PtySink).
pub type SerialSink = Arc<dyn Fn(&str, SerialOut) + Send + Sync>;

/// Wire config from the frontend. Strings (not enums) for parity/flow because it
/// arrives as JSON; each backend maps them with the same fail-soft defaults.
/// snake_case keys match the SerialProfile model and
/// the tab meta, so a saved profile's fields feed `serial_open` with no remap.
#[derive(Clone, serde::Deserialize)]
pub struct SerialConfig {
    pub baud_rate: u32,
    #[serde(default = "default_data_bits")]
    pub data_bits: u8,
    #[serde(default)]
    pub parity: String,
    #[serde(default = "default_stop_bits")]
    pub stop_bits: u8,
    #[serde(default)]
    pub flow_control: String,
    /// IXANY flow control (Unix and HarmonyOS; Windows no-op). The other
    /// settings live on SerialProfile and are applied in the frontend terminal.
    #[serde(default)]
    pub xany: bool,
}

fn default_data_bits() -> u8 {
    8
}
fn default_stop_bits() -> u8 {
    1
}

#[cfg(not(target_env = "ohos"))]
#[path = "serial/desktop.rs"]
mod desktop;
#[cfg(target_env = "ohos")]
pub use crate::ohos::serial::{available_ports, open, SerialHandle};
#[cfg(not(target_env = "ohos"))]
pub use desktop::{available_ports, open, SerialHandle};
