use std::{path::PathBuf, sync::OnceLock};

use openharmony_ability::OpenHarmonyApp;
use openharmony_ability_plugin_app_control::AppControlBridgePlugin;
use openharmony_ability_plugin_files::FilesBridgePlugin;
use openharmony_ability_plugin_permission::PermissionBridgePlugin;
use openharmony_ability_plugin_url::UrlBridgePlugin;
use openharmony_ability_plugin_webview::{
    WebviewBridgePlugin, WebviewProtocol, WebviewProtocolOptions,
};
use openharmony_ability_plugin_window::WindowBridgePlugin;

use crate::error::{AppError, AppResult};

// The framework refreshes this shared handle's contexts on Ability recreation.
// Tauri consumes its own copy when building the runtime.
static APP: OnceLock<OpenHarmonyApp> = OnceLock::new();

fn register_plugins(app: &OpenHarmonyApp) -> napi_ohos::Result<()> {
    for scheme in ["tauri", "ipc", "asset", "isolation"] {
        WebviewProtocol::register(
            scheme,
            WebviewProtocolOptions::Standard
                | WebviewProtocolOptions::CorsEnabled
                | WebviewProtocolOptions::CspBypassing
                | WebviewProtocolOptions::FetchEnabled
                | WebviewProtocolOptions::CodeCacheEnabled,
        )?;
    }
    app.register_plugin(WebviewBridgePlugin)?;
    app.register_plugin(WindowBridgePlugin)?;
    app.register_plugin(AppControlBridgePlugin)?;
    app.register_plugin(FilesBridgePlugin)?;
    app.register_plugin(UrlBridgePlugin)?;
    app.register_plugin(PermissionBridgePlugin)?;
    app.register_plugin(super::files::FilesAccessPlugin)?;
    app.register_plugin(super::clipboard::ClipboardPlugin)?;
    app.register_plugin(super::device::RuntimePlugin)?;
    app.register_plugin(super::serial::SerialPlugin)?;
    Ok(())
}

#[openharmony_ability_derive::ability]
fn configure_ability(app: OpenHarmonyApp) {
    register_plugins(&app).expect("failed to register RSSH native capability plugins");
    assert!(
        APP.set(app.clone()).is_ok(),
        "RSSH native host already configured"
    );
    tauri::ohos::APP
        .lock()
        .expect("Tauri host lock poisoned")
        .replace(app);
    crate::run();
}

pub fn app() -> AppResult<OpenHarmonyApp> {
    APP.get().cloned().ok_or_else(unavailable)
}

pub fn data_dir() -> AppResult<PathBuf> {
    // EntryAbility supplies ApplicationContext.filesDir, preserving the existing
    // database and master key across the framework migration.
    app()?
        .base_path()
        .map(PathBuf::from)
        .ok_or_else(unavailable)
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
