use rssh_core::error::AppResult;
use rssh_core::state::AppState;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn session_cleanup_status(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    session_ids: Vec<String>,
) -> AppResult<Vec<String>> {
    rssh_core::commands::lifecycle::session_cleanup_status(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        session_ids,
    )
}

#[tauri::command]
pub async fn reconcile_sessions(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    active_ids: Vec<String>,
) -> AppResult<usize> {
    rssh_core::commands::lifecycle::reconcile_sessions(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        active_ids,
    )
    .await
}
