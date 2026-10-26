use rssh_core::commands::sync::{SyncAutoPullStatus, SyncCheckResult, SyncProvider};
use rssh_core::error::AppResult;
use rssh_core::state::AppState;
use rssh_core::sync::metadata::SyncMetadata;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn get_sync_auto_pull_status(state: State<'_, Arc<AppState>>) -> AppResult<SyncAutoPullStatus> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::sync::get_sync_auto_pull_status(state.inner().as_ref())
}

#[tauri::command]
pub fn set_sync_auto_pull(
    state: State<'_, Arc<AppState>>,
    provider: SyncProvider,
    enabled: bool,
    password: Option<String>,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::sync::set_sync_auto_pull(
        state.inner().as_ref(),
        provider,
        enabled,
        password,
    )
}

#[tauri::command]
pub async fn sync_check(state: State<'_, Arc<AppState>>) -> AppResult<SyncCheckResult> {
    rssh_core::commands::sync::sync_check(state.inner().as_ref()).await
}

#[tauri::command]
pub async fn sync_refresh_local_metadata(
    state: State<'_, Arc<AppState>>,
) -> AppResult<SyncMetadata> {
    rssh_core::commands::sync::sync_refresh_local_metadata(state.inner().as_ref()).await
}

#[tauri::command]
pub async fn sync_check_remotes(state: State<'_, Arc<AppState>>) -> AppResult<SyncCheckResult> {
    rssh_core::commands::sync::sync_check_remotes(state.inner().as_ref()).await
}

#[tauri::command]
pub fn export_config(state: State<'_, Arc<AppState>>) -> AppResult<String> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::sync::export_config(state.inner().as_ref())
}

#[tauri::command]
pub fn import_config(state: State<'_, Arc<AppState>>, json: String) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::sync::import_config(state.inner().as_ref(), json)
}

#[tauri::command]
pub async fn github_push(state: State<'_, Arc<AppState>>, password: String) -> AppResult<()> {
    rssh_core::commands::sync::github_push(state.inner().as_ref(), password).await
}

#[tauri::command]
pub async fn github_pull(state: State<'_, Arc<AppState>>, password: String) -> AppResult<()> {
    rssh_core::commands::sync::github_pull(state.inner().as_ref(), password).await
}

#[tauri::command]
pub async fn webdav_push(state: State<'_, Arc<AppState>>, password: String) -> AppResult<()> {
    rssh_core::commands::sync::webdav_push(state.inner().as_ref(), password).await
}

#[tauri::command]
pub async fn webdav_pull(state: State<'_, Arc<AppState>>, password: String) -> AppResult<()> {
    rssh_core::commands::sync::webdav_pull(state.inner().as_ref(), password).await
}
