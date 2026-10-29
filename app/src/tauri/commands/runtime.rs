use crate::error::AppResult;
use crate::platform::runtime::{self, RuntimeCapabilities};

#[tauri::command]
pub async fn get_runtime_capabilities(window: tauri::Window) -> AppResult<RuntimeCapabilities> {
    runtime::capabilities(&window).await
}

#[tauri::command]
pub async fn set_back_navigation_enabled(window: tauri::Window, enabled: bool) -> AppResult<()> {
    use tauri::Manager;
    let revision = crate::platform::host::next_back_navigation_revision();
    crate::platform::host::platform(window.app_handle())
        .set_back_navigation_enabled(window.app_handle(), window.label(), enabled, revision)
        .await
}
