use rssh_core::error::AppResult;
use rssh_core::models::ConnectorSpec;
use rssh_core::state::AppState;
use std::sync::Arc;
use tauri::{AppHandle, State};

#[tauri::command]
pub fn pty_spawn(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    cols: u16,
    rows: u16,
    session_id: Option<String>,
) -> AppResult<String> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::pty::pty_spawn(
        crate::tauri_host::host(app),
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        cols,
        rows,
        session_id,
    )
}

#[tauri::command]
pub fn pty_spawn_connector(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    cols: u16,
    rows: u16,
    spec: ConnectorSpec,
    session_id: Option<String>,
) -> AppResult<String> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::pty::pty_spawn_connector(
        crate::tauri_host::host(app),
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        cols,
        rows,
        spec,
        session_id,
    )
}

#[tauri::command]
pub fn list_shells() -> AppResult<Vec<String>> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::pty::list_shells()
}

#[tauri::command]
pub fn refresh_shells() -> AppResult<Vec<String>> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::pty::refresh_shells()
}

#[tauri::command]
pub fn pty_write(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    data: Vec<u8>,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::pty::pty_write(state.inner().as_ref(), session_id, data)
}

#[tauri::command]
pub fn pty_resize(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::pty::pty_resize(state.inner().as_ref(), session_id, cols, rows)
}

#[tauri::command]
pub fn pty_close(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::pty::pty_close(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        session_id,
    )
}
