//! Native services shared by the HarmonyOS app and core transports.
use std::sync::OnceLock;

use openharmony_ability::OpenHarmonyApp;

use crate::error::{AppError, AppResult};

#[path = "pty.rs"]
pub mod pty;

#[path = "serial.rs"]
pub mod serial;

// Ability recreation refreshes the contexts inside this shared handle.
static APP: OnceLock<OpenHarmonyApp> = OnceLock::new();

pub fn configure(app: OpenHarmonyApp) {
    assert!(APP.set(app).is_ok(), "RSSH native host already configured");
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
