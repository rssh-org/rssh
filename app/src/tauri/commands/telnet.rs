use rssh_core::error::AppResult;
use rssh_core::models::TelnetProfile;
use rssh_core::state::AppState;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn telnet_write(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    data: Vec<u8>,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::telnet::telnet_write(state.inner().as_ref(), session_id, data)
}

#[tauri::command]
pub fn telnet_write_line(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    text: String,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::telnet::telnet_write_line(state.inner().as_ref(), session_id, text)
}

#[tauri::command]
pub fn telnet_resize(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::telnet::telnet_resize(state.inner().as_ref(), session_id, cols, rows)
}

#[tauri::command]
pub fn telnet_close(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::telnet::telnet_close(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        session_id,
    )
}

#[tauri::command]
pub fn list_telnet_profiles(state: State<'_, Arc<AppState>>) -> AppResult<Vec<TelnetProfile>> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::telnet::list_telnet_profiles(state.inner().as_ref())
}

#[tauri::command]
pub fn get_telnet_profile(state: State<'_, Arc<AppState>>, id: String) -> AppResult<TelnetProfile> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::telnet::get_telnet_profile(state.inner().as_ref(), id)
}

#[tauri::command]
pub fn create_telnet_profile(
    state: State<'_, Arc<AppState>>,
    profile: TelnetProfile,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::telnet::create_telnet_profile(state.inner().as_ref(), profile)
}

#[tauri::command]
pub fn update_telnet_profile(
    state: State<'_, Arc<AppState>>,
    profile: TelnetProfile,
    login_script_update: Option<crate::telnet_profile::LoginScriptUpdate>,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::telnet::update_telnet_profile(
        state.inner().as_ref(),
        profile,
        login_script_update,
    )
}

#[tauri::command]
pub fn delete_telnet_profile(state: State<'_, Arc<AppState>>, id: String) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::telnet::delete_telnet_profile(state.inner().as_ref(), id)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn telnet_open(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    host: String,
    port: u16,
    cols: u16,
    rows: u16,
    input_newline: Option<String>,
    session_id: Option<String>,
) -> AppResult<String> {
    rssh_core::commands::telnet::telnet_open(
        crate::tauri_host::host(app),
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        host,
        port,
        cols,
        rows,
        input_newline,
        session_id,
    )
    .await
}
