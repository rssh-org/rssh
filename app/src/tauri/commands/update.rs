use rssh_core::error::AppResult;

#[tauri::command]
pub async fn fetch_latest_release_tag(repo: String) -> AppResult<String> {
    rssh_core::commands::update::fetch_latest_release_tag(repo).await
}
