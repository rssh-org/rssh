use crate::cli::CliStatus;
use crate::error::AppResult;
use tauri::AppHandle;

#[tauri::command]
pub fn cli_status(app: AppHandle) -> CliStatus {
    crate::cli::cli_status(app)
}

#[tauri::command]
pub fn cli_install(app: AppHandle) -> AppResult<String> {
    crate::cli::cli_install(app)
}
