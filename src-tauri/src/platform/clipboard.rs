//! Clipboard access with native ownership kept inside the selected backend.

use crate::error::AppResult;
#[cfg(any(windows, macos, linux))]
#[path = "tauri/clipboard.rs"]
mod backend;

pub async fn read_text() -> AppResult<String> {
    #[cfg(ohos)]
    {
        crate::ohos::clipboard::read_text().await
    }
    #[cfg(any(windows, macos, linux))]
    {
        tokio::task::spawn_blocking(backend::read_text)
            .await
            .map_err(join_error)?
    }
    #[cfg(any(android, ios))]
    {
        Err(unavailable("read"))
    }
}

pub async fn write_text(text: String) -> AppResult<()> {
    #[cfg(ohos)]
    {
        crate::ohos::clipboard::write_text(text).await
    }
    #[cfg(any(windows, macos, linux))]
    {
        tokio::task::spawn_blocking(move || backend::write_text(text))
            .await
            .map_err(join_error)?
    }
    #[cfg(any(android, ios))]
    {
        let _ = text;
        Err(unavailable("write"))
    }
}

#[cfg(any(windows, macos, linux))]
fn join_error(error: tokio::task::JoinError) -> crate::error::AppError {
    crate::error::AppError::other(
        "window_clipboard_failed",
        serde_json::json!({"op": "dispatch", "err": error.to_string()}),
    )
}

#[cfg(any(android, ios))]
fn unavailable(op: &str) -> crate::error::AppError {
    // These hosts retain their WebView clipboard adapter, selected by capabilities.
    crate::error::AppError::other(
        "window_clipboard_failed",
        serde_json::json!({"op": op, "err": "Clipboard access is provided by the WebView"}),
    )
}
