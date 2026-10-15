use crate::error::{AppError, AppResult};

pub async fn open_url(app: &tauri::AppHandle, url: String) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_url(&url, None::<&str>).map_err(|error| {
        AppError::other(
            "window_open_url_failed",
            serde_json::json!({"err": error.to_string()}),
        )
    })
}
