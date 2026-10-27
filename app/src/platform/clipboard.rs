//! Clipboard operations use the native adapter owned by this application.
use super::host::platform;
use crate::error::AppResult;

pub async fn read_text(app: &tauri::AppHandle) -> AppResult<String> {
    platform(app).read_clipboard().await
}

pub async fn write_text(app: &tauri::AppHandle, text: String) -> AppResult<()> {
    platform(app).write_clipboard(text).await
}
