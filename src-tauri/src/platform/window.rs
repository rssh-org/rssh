//! Shared window operations; native creation protocols belong to the backend.

use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};

#[cfg(ohos)]
use crate::ohos::window as backend;
#[cfg(not(ohos))]
#[path = "tauri/window.rs"]
mod backend;

#[derive(Clone, Copy)]
pub(crate) enum AppWindowPurpose {
    Tab,
    LocalAnalysis,
}

pub(crate) async fn ensure_app_window_available(purpose: AppWindowPurpose) -> Result<(), String> {
    backend::ensure_available(purpose).await
}

/// Both IPC tab cloning and AI handoff use this boundary. The window's unique
/// label becomes its session owner when the frontend creates its resources.
pub(crate) async fn open_app_window(
    app: &AppHandle,
    label: &str,
    title: &str,
    init_script: &str,
    purpose: AppWindowPurpose,
) -> Result<(), String> {
    ensure_app_window_available(purpose).await?;
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(title)
        .inner_size(1200.0, 800.0)
        .initialization_script(init_script)
        .build()
        .map_err(|error| error.to_string())?;
    backend::confirm_creation(window).await
}
