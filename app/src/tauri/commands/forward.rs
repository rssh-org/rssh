use rssh_core::error::{AppError, AppResult};
use rssh_core::models::Forward;
use rssh_core::ssh::forward as fwd;
use rssh_core::state::AppState;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn list_forwards(state: State<'_, Arc<AppState>>) -> Result<Vec<Forward>, AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::forward::list_forwards(state.inner().as_ref())
}

#[tauri::command]
pub fn get_forward(state: State<'_, Arc<AppState>>, id: String) -> Result<Forward, AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::forward::get_forward(state.inner().as_ref(), id)
}

#[tauri::command]
pub fn create_forward(state: State<'_, Arc<AppState>>, forward: Forward) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::forward::create_forward(state.inner().as_ref(), forward)
}

#[tauri::command]
pub fn update_forward(state: State<'_, Arc<AppState>>, forward: Forward) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::forward::update_forward(state.inner().as_ref(), forward)
}

#[tauri::command]
pub fn delete_forward(state: State<'_, Arc<AppState>>, id: String) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::forward::delete_forward(state.inner().as_ref(), id)
}

#[tauri::command]
pub async fn forward_start(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    forward_id: String,
) -> AppResult<String> {
    rssh_core::commands::forward::forward_start(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        forward_id,
    )
    .await
}

#[tauri::command]
pub fn forward_stats(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    active_id: String,
) -> AppResult<fwd::ForwardStats> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::forward::forward_stats(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        active_id,
    )
}

#[tauri::command]
pub async fn forward_rule_start(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    active_id: String,
    rule_index: usize,
) -> AppResult<()> {
    rssh_core::commands::forward::forward_rule_start(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        active_id,
        rule_index,
    )
    .await
}

#[tauri::command]
pub async fn forward_rule_stop(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    active_id: String,
    rule_index: usize,
) -> AppResult<()> {
    rssh_core::commands::forward::forward_rule_stop(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        active_id,
        rule_index,
    )
    .await
}

#[tauri::command]
pub fn forward_stop(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    active_id: String,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::forward::forward_stop(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        active_id,
    )
}
