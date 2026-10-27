//! HarmonyOS application context and native implementations supplied to core.
use std::sync::OnceLock;

use openharmony_ability::OpenHarmonyApp;

use rssh_core::error::{AppError, AppResult};

// Ability recreation refreshes the contexts inside this shared handle.
static APP: OnceLock<OpenHarmonyApp> = OnceLock::new();

pub fn configure(app: OpenHarmonyApp) {
    assert!(APP.set(app).is_ok(), "RSSH native host already configured");
}

pub fn services() -> rssh_core::state::NativeServices {
    rssh_core::state::NativeServices {
        keyring: crate::secret::try_open().map(|store| {
            std::sync::Arc::new(store) as std::sync::Arc<dyn rssh_core::secret::SecretStore>
        }),
        serial: std::sync::Arc::new(crate::serial::OhosSerialBackend),
    }
}

pub fn app() -> AppResult<OpenHarmonyApp> {
    APP.get().cloned().ok_or_else(unavailable)
}

pub fn unavailable() -> AppError {
    AppError::other("ohos_native_unavailable", serde_json::json!({}))
}

pub fn native_error(error: impl std::fmt::Display) -> AppError {
    AppError::other(
        "ohos_native_failed",
        serde_json::json!({"err": error.to_string()}),
    )
}
