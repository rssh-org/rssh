use std::sync::Arc;

use crate::error::{AppError, AppResult};

/// Options implemented by this host's serial service. An empty rate list means
/// the native driver accepts a custom baud rate and validates it when opening.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialCapabilities {
    pub flow_control: bool,
    pub xany: bool,
    pub signals: bool,
    pub baud_rates: Vec<u32>,
}

impl SerialCapabilities {
    pub fn validate(&self, config: &SerialConfig) -> AppResult<()> {
        if !self.flow_control && matches!(config.flow_control.as_str(), "hardware" | "software") {
            return Err(AppError::config(
                "serial_flow_unsupported",
                serde_json::json!({}),
            ));
        }
        if !self.xany && config.xany {
            return Err(AppError::config(
                "serial_xany_unsupported",
                serde_json::json!({}),
            ));
        }
        if !self.baud_rates.is_empty() && !self.baud_rates.contains(&config.baud_rate) {
            return Err(AppError::config(
                "serial_baud_unsupported",
                serde_json::json!({ "baud": config.baud_rate }),
            ));
        }
        Ok(())
    }

    #[cfg(any(ohos, test))]
    pub fn require_signals(&self) -> AppResult<()> {
        if self.signals {
            Ok(())
        } else {
            Err(AppError::config(
                "serial_signals_unsupported",
                serde_json::json!({}),
            ))
        }
    }
}

#[cfg(not(ohos))]
pub fn capabilities() -> SerialCapabilities {
    SerialCapabilities {
        flow_control: true,
        xany: cfg!(unix),
        signals: true,
        baud_rates: Vec::new(),
    }
}

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

/// Wire config from the frontend. Strings for parity/flow match saved profiles;
/// each host validates unsupported features before mapping native settings.
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
    /// IXANY flow control where the host advertises support. The other
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

#[cfg(not(ohos))]
#[path = "serial/desktop.rs"]
mod desktop;
#[cfg(ohos)]
pub use crate::ohos::serial::{available_ports, capabilities, open, SerialHandle};
#[cfg(not(ohos))]
pub use desktop::{available_ports, open, SerialHandle};

/// The platform adapter only opens the resource. This shared coordinator owns
/// failed-open cleanup and carries that ownership through activation.
pub async fn open_resource(
    session_id: String,
    port: &str,
    config: SerialConfig,
    sink: SerialSink,
    operation: crate::resource::PendingOperation,
) -> AppResult<crate::resource::OpenedResource<SerialHandle>> {
    #[cfg(ohos)]
    let result = open(session_id, port, config, sink, &operation).await;
    #[cfg(not(ohos))]
    let result = open(session_id, port, config, sink);
    operation.finish_open(result).await
}

/// Some native backends require an acknowledged release before a port can be
/// reopened. Others retain their existing last-handle-drop cleanup behavior.
pub(crate) fn cleanup(handle: &SerialHandle) -> Option<crate::resource::CleanupHandle> {
    #[cfg(ohos)]
    {
        Some(crate::resource::CleanupHandle::new(Arc::new(
            handle.clone(),
        )))
    }
    #[cfg(not(ohos))]
    {
        let _ = handle;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb_capabilities() -> SerialCapabilities {
        SerialCapabilities {
            flow_control: false,
            xany: false,
            signals: false,
            baud_rates: vec![9600, 115200],
        }
    }

    fn config() -> SerialConfig {
        serde_json::from_value(serde_json::json!({ "baud_rate": 115200 })).unwrap()
    }

    #[test]
    fn basic_serial_accepts_defaults_and_rejects_unsupported_saved_settings() {
        let capabilities = usb_capabilities();
        assert!(capabilities.validate(&config()).is_ok());
        for flow in ["hardware", "software"] {
            let mut config = config();
            config.flow_control = flow.into();
            assert!(capabilities
                .validate(&config)
                .unwrap_err()
                .to_string()
                .contains("serial_flow_unsupported"));
        }
        let mut config = config();
        config.xany = true;
        assert!(capabilities
            .validate(&config)
            .unwrap_err()
            .to_string()
            .contains("serial_xany_unsupported"));
        config.xany = false;
        config.baud_rate = 12345;
        assert!(capabilities
            .validate(&config)
            .unwrap_err()
            .to_string()
            .contains("serial_baud_unsupported"));
        assert!(capabilities
            .require_signals()
            .unwrap_err()
            .to_string()
            .contains("serial_signals_unsupported"));
    }

    #[test]
    fn extended_serial_accepts_custom_rates_and_signal_controls() {
        let capabilities = SerialCapabilities {
            flow_control: true,
            xany: true,
            signals: true,
            baud_rates: Vec::new(),
        };
        let mut config = config();
        config.flow_control = "software".into();
        config.xany = true;
        config.baud_rate = 12345;
        assert!(capabilities.validate(&config).is_ok());
        assert!(capabilities.require_signals().is_ok());
    }

    #[test]
    fn serial_capabilities_use_the_frontend_wire_names() {
        assert_eq!(
            serde_json::to_value(usb_capabilities()).unwrap(),
            serde_json::json!({
                "flowControl": false, "xany": false, "signals": false,
                "baudRates": [9600, 115200],
            }),
        );
    }
}
