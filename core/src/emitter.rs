//! Event delivery and optional window capabilities supplied by the application.

use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;

use crate::state::AppState;

/// False means the recipient has disconnected. Prompt producers must then fail
/// instead of leaving a waiter that no frontend can answer.
pub type Sink = Arc<dyn Fn(&str, serde_json::Value) -> bool + Send + Sync>;

#[async_trait::async_trait]
pub trait WindowHost: Send + Sync {
    fn allows_diagnosis_downloads(&self) -> bool;
    async fn ensure_local_analysis_available(&self) -> Result<(), String>;
    async fn open_app_window(
        &self,
        label: &str,
        title: &str,
        init_script: &str,
    ) -> Result<(), String>;
}

/// Terminal data has one wire representation across all application adapters.
pub fn b64_payload(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

#[derive(Clone)]
pub struct Host {
    state: Arc<AppState>,
    sink: Sink,
    windows: Option<Arc<dyn WindowHost>>,
}

impl Host {
    pub fn new(state: Arc<AppState>, sink: Sink) -> Self {
        Self {
            state,
            sink,
            windows: None,
        }
    }

    pub fn with_window_host(mut self, windows: Arc<dyn WindowHost>) -> Self {
        self.windows = Some(windows);
        self
    }

    pub fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<(), String> {
        let payload = serde_json::to_value(payload).map_err(|error| error.to_string())?;
        if (self.sink)(event, payload) {
            Ok(())
        } else {
            Err("event recipient disconnected".into())
        }
    }

    pub fn emit_bytes(&self, event: &str, bytes: &[u8]) -> Result<(), String> {
        self.emit(event, b64_payload(bytes))
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn allows_diagnosis_downloads(&self) -> bool {
        self.windows
            .as_ref()
            .is_none_or(|windows| windows.allows_diagnosis_downloads())
    }

    pub async fn ensure_local_analysis_available(&self) -> Result<(), String> {
        match &self.windows {
            Some(windows) => windows.ensure_local_analysis_available().await,
            None => Err("Local analysis windows are unavailable in headless mode".into()),
        }
    }

    pub async fn open_app_window(
        &self,
        label: &str,
        title: &str,
        init_script: &str,
    ) -> Result<(), String> {
        match &self.windows {
            Some(windows) => windows.open_app_window(label, title, init_script).await,
            None => Err("multi-window unavailable in headless mode".into()),
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn terminal_events_preserve_the_shared_base64_wire_format() {
        let (host, mut events) = crate::ssh::test_support::headless_host();
        host.emit_bytes("ssh:data:session", &[0, 127, 255]).unwrap();
        assert_eq!(
            events.try_recv().unwrap(),
            ("ssh:data:session".into(), serde_json::json!("AH//"))
        );
    }

    #[test]
    fn disconnected_recipient_fails_prompt_delivery() {
        let (host, events) = crate::ssh::test_support::headless_host();
        drop(events);
        assert!(host.emit("ssh:auth:prompt", serde_json::json!({})).is_err());
    }

    #[tokio::test]
    async fn headless_supports_downloads_without_claiming_native_windows() {
        let (host, _events) = crate::ssh::test_support::headless_host();
        assert!(host.allows_diagnosis_downloads());
        assert!(host.ensure_local_analysis_available().await.is_err());
        assert!(host.open_app_window("label", "title", "").await.is_err());
    }
}
