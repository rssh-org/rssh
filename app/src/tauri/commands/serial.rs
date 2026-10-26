use rssh_core::error::AppResult;
use rssh_core::models::SerialProfile;
use rssh_core::state::AppState;
use rssh_core::terminal::serial;
use std::sync::Arc;
use tauri::{AppHandle, State};

#[tauri::command]
pub async fn serial_get_capabilities() -> AppResult<serial::SerialCapabilities> {
    rssh_core::commands::serial::serial_get_capabilities().await
}

#[tauri::command]
pub async fn serial_list_ports() -> AppResult<Vec<String>> {
    rssh_core::commands::serial::serial_list_ports().await
}

#[tauri::command]
pub async fn serial_open(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    port: String,
    config: serial::SerialConfig,
    session_id: Option<String>,
) -> AppResult<String> {
    rssh_core::commands::serial::serial_open(
        crate::tauri_host::host(app),
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        port,
        config,
        session_id,
    )
    .await
}

#[tauri::command]
pub async fn serial_write(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    data: Vec<u8>,
) -> AppResult<()> {
    rssh_core::commands::serial::serial_write(state.inner().as_ref(), session_id, data).await
}

#[tauri::command]
pub async fn serial_set_dtr(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    level: bool,
) -> AppResult<()> {
    rssh_core::commands::serial::serial_set_dtr(state.inner().as_ref(), session_id, level).await
}

#[tauri::command]
pub async fn serial_set_rts(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    level: bool,
) -> AppResult<()> {
    rssh_core::commands::serial::serial_set_rts(state.inner().as_ref(), session_id, level).await
}

#[tauri::command]
pub async fn serial_send_break(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> AppResult<()> {
    rssh_core::commands::serial::serial_send_break(state.inner().as_ref(), session_id).await
}

#[tauri::command]
pub async fn serial_close(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> AppResult<()> {
    rssh_core::commands::serial::serial_close(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        session_id,
    )
    .await
}

#[tauri::command]
pub fn list_serial_profiles(state: State<'_, Arc<AppState>>) -> AppResult<Vec<SerialProfile>> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::serial::list_serial_profiles(state.inner().as_ref())
}

#[tauri::command]
pub fn get_serial_profile(state: State<'_, Arc<AppState>>, id: String) -> AppResult<SerialProfile> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::serial::get_serial_profile(state.inner().as_ref(), id)
}

#[tauri::command]
pub fn create_serial_profile(
    state: State<'_, Arc<AppState>>,
    profile: SerialProfile,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::serial::create_serial_profile(state.inner().as_ref(), profile)
}

#[tauri::command]
pub fn update_serial_profile(
    state: State<'_, Arc<AppState>>,
    profile: SerialProfile,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::serial::update_serial_profile(state.inner().as_ref(), profile)
}

#[tauri::command]
pub fn delete_serial_profile(state: State<'_, Arc<AppState>>, id: String) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::serial::delete_serial_profile(state.inner().as_ref(), id)
}
