use rssh_core::error::{AppError, AppResult};
use rssh_core::models::{Credential, Profile, SshAlgorithmCatalog};
use rssh_core::state::AppState;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn list_profiles(state: State<'_, Arc<AppState>>) -> Result<Vec<Profile>, AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::profile::list_profiles(state.inner().as_ref())
}

#[tauri::command]
pub fn get_profile(state: State<'_, Arc<AppState>>, id: String) -> Result<Profile, AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::profile::get_profile(state.inner().as_ref(), id)
}

#[tauri::command]
pub fn create_profile(state: State<'_, Arc<AppState>>, profile: Profile) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::profile::create_profile(state.inner().as_ref(), profile)
}

#[tauri::command]
pub fn update_profile(state: State<'_, Arc<AppState>>, profile: Profile) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::profile::update_profile(state.inner().as_ref(), profile)
}

#[tauri::command]
pub fn delete_profile(state: State<'_, Arc<AppState>>, id: String) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::profile::delete_profile(state.inner().as_ref(), id)
}

#[tauri::command]
pub fn ssh_algorithm_catalog() -> SshAlgorithmCatalog {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::profile::ssh_algorithm_catalog()
}

#[tauri::command]
pub fn list_credentials(state: State<'_, Arc<AppState>>) -> Result<Vec<Credential>, AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::profile::list_credentials(state.inner().as_ref())
}

#[tauri::command]
pub fn get_credential(state: State<'_, Arc<AppState>>, id: String) -> Result<Credential, AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::profile::get_credential(state.inner().as_ref(), id)
}

#[tauri::command]
pub fn create_credential(
    state: State<'_, Arc<AppState>>,
    credential: Credential,
) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::profile::create_credential(state.inner().as_ref(), credential)
}

#[tauri::command]
pub fn update_credential(
    state: State<'_, Arc<AppState>>,
    credential: Credential,
) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::profile::update_credential(state.inner().as_ref(), credential)
}

#[tauri::command]
pub fn delete_credential(state: State<'_, Arc<AppState>>, id: String) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::profile::delete_credential(state.inner().as_ref(), id)
}

#[tauri::command]
pub fn read_default_key_file(name: String) -> AppResult<String> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::profile::read_default_key_file(name)
}
