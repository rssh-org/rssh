use crate::error::AppResult;
use crate::platform::clipboard;

#[tauri::command]
pub async fn clipboard_read(app: tauri::AppHandle) -> AppResult<String> {
    clipboard::read_text(&app).await
}

#[tauri::command]
pub async fn clipboard_write(app: tauri::AppHandle, text: String) -> AppResult<()> {
    clipboard::write_text(&app, text).await
}
