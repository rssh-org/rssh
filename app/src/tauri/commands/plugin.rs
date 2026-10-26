use rssh_core::error::AppResult;
use rssh_core::models::{Plugin, PluginExecResult};
use rssh_core::state::AppState;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn plugins_root(state: State<'_, Arc<AppState>>) -> AppResult<String> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::plugin::plugins_root(state.inner().as_ref())
}

#[tauri::command]
pub fn install_plugin(
    state: State<'_, Arc<AppState>>,
    base64_zip: String,
    area: Option<String>,
) -> AppResult<Plugin> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::plugin::install_plugin(state.inner().as_ref(), base64_zip, area)
}

#[tauri::command]
pub fn list_plugins(state: State<'_, Arc<AppState>>) -> AppResult<Vec<Plugin>> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::plugin::list_plugins(state.inner().as_ref())
}

#[tauri::command]
pub fn set_plugin_enabled(
    state: State<'_, Arc<AppState>>,
    id: String,
    enabled: bool,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::plugin::set_plugin_enabled(state.inner().as_ref(), id, enabled)
}

#[tauri::command]
pub fn set_plugin_order(state: State<'_, Arc<AppState>>, ids: Vec<String>) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::plugin::set_plugin_order(state.inner().as_ref(), ids)
}

#[tauri::command]
pub fn uninstall_plugin(state: State<'_, Arc<AppState>>, id: String) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::plugin::uninstall_plugin(state.inner().as_ref(), id)
}

#[tauri::command]
pub async fn plugin_exec(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    command: String,
    timeout_ms: Option<u64>,
) -> AppResult<PluginExecResult> {
    rssh_core::commands::plugin::plugin_exec(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        session_id,
        command,
        timeout_ms,
    )
    .await
}
