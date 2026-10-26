use crate::error::AppResult;

#[tauri::command]
pub async fn open_external_url(app: tauri::AppHandle, url: String) -> AppResult<()> {
    crate::platform::external::open_url(&app, url).await
}
