use crate::error::{AppError, AppResult};

pub async fn open_url(_app: &tauri::AppHandle, url: String) -> AppResult<()> {
    use openharmony_ability_plugin_url::UrlExt;
    super::app()?.open_url(url).await.map_err(|error| {
        AppError::other(
            "window_open_url_failed",
            serde_json::json!({"err": error.to_string()}),
        )
    })
}
