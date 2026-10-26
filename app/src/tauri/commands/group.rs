use rssh_core::error::AppError;
use rssh_core::models::Group;
use rssh_core::state::AppState;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn list_groups(state: State<'_, Arc<AppState>>) -> Result<Vec<Group>, AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::group::list_groups(state.inner().as_ref())
}

#[tauri::command]
pub fn create_group(state: State<'_, Arc<AppState>>, group: Group) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::group::create_group(state.inner().as_ref(), group)
}

#[tauri::command]
pub fn update_group(state: State<'_, Arc<AppState>>, group: Group) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::group::update_group(state.inner().as_ref(), group)
}

#[tauri::command]
pub fn delete_group(state: State<'_, Arc<AppState>>, id: String) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::group::delete_group(state.inner().as_ref(), id)
}
