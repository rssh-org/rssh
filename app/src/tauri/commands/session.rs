use rssh_core::error::AppResult;
use rssh_core::state::AppState;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub async fn ssh_write(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    data: Vec<u8>,
) -> AppResult<()> {
    rssh_core::commands::session::ssh_write(state.inner().as_ref(), session_id, data).await
}

#[tauri::command]
pub async fn ssh_resize(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    cols: u32,
    rows: u32,
) -> AppResult<()> {
    rssh_core::commands::session::ssh_resize(state.inner().as_ref(), session_id, cols, rows).await
}

#[tauri::command]
pub async fn ssh_disconnect(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    tab_id: Option<String>,
) -> AppResult<()> {
    rssh_core::commands::session::ssh_disconnect(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        session_id,
        tab_id,
    )
    .await
}

#[tauri::command]
pub async fn ssh_auth_cancel(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
) -> AppResult<()> {
    rssh_core::commands::session::ssh_auth_cancel(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
    )
    .await
}

#[tauri::command]
pub async fn ssh_auth_respond(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    responses: Vec<String>,
) -> AppResult<()> {
    rssh_core::commands::session::ssh_auth_respond(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        responses,
    )
    .await
}

#[tauri::command]
pub async fn ssh_passphrase_respond(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    passphrase: String,
) -> AppResult<()> {
    rssh_core::commands::session::ssh_passphrase_respond(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        passphrase,
    )
    .await
}

#[tauri::command]
pub async fn ssh_passphrase_cancel(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
) -> AppResult<()> {
    rssh_core::commands::session::ssh_passphrase_cancel(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
    )
    .await
}

#[tauri::command]
pub async fn ssh_host_key_respond(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    answer: String,
) -> AppResult<()> {
    rssh_core::commands::session::ssh_host_key_respond(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        answer,
    )
    .await
}

#[tauri::command]
pub async fn ssh_host_key_cancel(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
) -> AppResult<()> {
    rssh_core::commands::session::ssh_host_key_cancel(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
    )
    .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn ssh_connect(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    profile_id: String,
    log_session_id: Option<String>,
    cols: u32,
    rows: u32,
    session_id: Option<String>,
) -> AppResult<String> {
    rssh_core::commands::session::ssh_connect(
        crate::tauri_host::host(app),
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        profile_id,
        log_session_id,
        cols,
        rows,
        session_id,
    )
    .await
}
