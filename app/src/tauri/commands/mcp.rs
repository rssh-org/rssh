use std::sync::Arc;

use rssh_core::agent::{self, AgentSession};
use rssh_core::ai::commands::AiSessionInfo;
use rssh_core::emitter::{Host, Sink};
use rssh_core::error::AppResult;
use rssh_core::state::{AppState, SessionOwner};
use tauri::{Emitter, State};

fn window_sink(window: tauri::Window) -> Sink {
    let target = window.label().to_owned();
    Arc::new(move |event, payload| window.emit_to(target.as_str(), event, payload).is_ok())
}

#[tauri::command]
pub async fn mcp_start(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
) -> AppResult<crate::mcp::Status> {
    crate::mcp::start(
        state.inner().clone(),
        SessionOwner::Window(window.label().into()),
    )
    .await
}

#[tauri::command]
pub fn mcp_status(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
) -> AppResult<crate::mcp::Status> {
    crate::mcp::status(
        state.inner().as_ref(),
        &SessionOwner::Window(window.label().into()),
    )
}

#[tauri::command]
pub fn mcp_stop(window: tauri::Window, state: State<'_, Arc<AppState>>) -> AppResult<()> {
    crate::mcp::stop(
        state.inner().as_ref(),
        &SessionOwner::Window(window.label().into()),
    )
}

#[tauri::command]
pub fn mcp_register_session(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    tab_id: String,
    label: String,
) -> AppResult<AgentSession> {
    let owner = SessionOwner::Window(window.label().into());
    agent::register_session(
        state.inner().as_ref(),
        &owner,
        window_sink(window),
        session_id,
        tab_id,
        label,
    )
}

#[tauri::command]
pub fn mcp_unregister_session(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> AppResult<()> {
    agent::unregister_session(
        state.inner().as_ref(),
        &SessionOwner::Window(window.label().into()),
        &session_id,
    )
}

#[tauri::command]
pub async fn mcp_accept_session(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    request_id: String,
) -> AppResult<AiSessionInfo> {
    let owner = SessionOwner::Window(window.label().into());
    let host = Host::new(state.inner().clone(), window_sink(window));
    agent::accept_session(state.inner().as_ref(), host, &owner, &request_id).await
}

#[tauri::command]
pub fn mcp_session_ready(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    request_id: String,
    instance_id: String,
) -> AppResult<()> {
    agent::session_ready(
        state.inner().as_ref(),
        &SessionOwner::Window(window.label().into()),
        &request_id,
        &instance_id,
    )
}

#[tauri::command]
pub fn mcp_reject_session(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    request_id: String,
) -> AppResult<()> {
    agent::reject_session(
        state.inner().as_ref(),
        &SessionOwner::Window(window.label().into()),
        &request_id,
    )
}
