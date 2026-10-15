use crate::error::AppResult;
use crate::platform::clipboard;

#[tauri::command]
pub async fn clipboard_read() -> AppResult<String> {
    clipboard::read_text().await
}

#[tauri::command]
pub async fn clipboard_write(text: String) -> AppResult<()> {
    clipboard::write_text(text).await
}
