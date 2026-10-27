//! Shared window operations; native creation protocols belong to the backend.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use super::host::platform;
pub use super::host::AppWindowPurpose;

pub(crate) async fn ensure_app_window_available(
    app: &AppHandle,
    purpose: AppWindowPurpose,
) -> Result<(), String> {
    platform(app).ensure_window(purpose).await
}

pub(crate) fn set_app_window_title(window: &tauri::Window, title: &str) -> tauri::Result<()> {
    window.set_title(&platform(window.app_handle()).window_title(title))
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
    ensure_app_window_available(app, purpose).await?;
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(platform(app).window_title(title))
        .inner_size(1200.0, 800.0)
        .initialization_script(init_script)
        .build()
        .map_err(|error| error.to_string())?;
    platform(app).confirm_window(window).await
}
