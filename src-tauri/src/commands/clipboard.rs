#[cfg(not(target_env = "ohos"))]
use crate::error::AppError;
use crate::error::AppResult;

#[cfg(not(target_env = "ohos"))]
fn clipboard_error(op: &str, error: impl std::fmt::Display) -> AppError {
    AppError::other(
        "window_clipboard_failed",
        serde_json::json!({ "op": op, "err": error.to_string() }),
    )
}

#[tauri::command]
pub async fn clipboard_read(app: tauri::AppHandle) -> AppResult<String> {
    #[cfg(target_env = "ohos")]
    {
        let _ = app;
        crate::ohos::clipboard::read_text().await
    }
    #[cfg(not(target_env = "ohos"))]
    {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        // Native clipboard reads can wait for another application's response.
        // Keep them off the UI thread, including Linux selection ownership.
        tokio::task::spawn_blocking(move || {
            app.clipboard()
                .read_text()
                .map_err(|e| clipboard_error("read", e))
        })
        .await
        .map_err(|e| clipboard_error("read", e))?
    }
}

#[tauri::command]
pub async fn clipboard_write(app: tauri::AppHandle, text: String) -> AppResult<()> {
    #[cfg(target_env = "ohos")]
    {
        let _ = app;
        crate::ohos::clipboard::write_text(text).await
    }
    #[cfg(not(target_env = "ohos"))]
    {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        tokio::task::spawn_blocking(move || {
            app.clipboard()
                .write_text(text)
                .map_err(|e| clipboard_error("write", e))
        })
        .await
        .map_err(|e| clipboard_error("write", e))?
    }
}
