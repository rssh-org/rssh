use crate::error::AppResult;
use crate::platform::runtime::{self, RuntimeCapabilities};

#[tauri::command]
pub async fn get_runtime_capabilities(window: tauri::Window) -> AppResult<RuntimeCapabilities> {
    runtime::capabilities(&window).await
}
