use rssh_core::commands::settings::FontInfo;
use rssh_core::error::{AppError, AppResult};
use rssh_core::models::{HighlightRule, Snippet};
use rssh_core::state::AppState;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn get_setting(
    state: State<'_, Arc<AppState>>,
    key: String,
) -> Result<Option<String>, AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::settings::get_setting(state.inner().as_ref(), key)
}

#[tauri::command]
pub fn set_setting(
    state: State<'_, Arc<AppState>>,
    key: String,
    value: String,
) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::settings::set_setting(state.inner().as_ref(), key, value)
}

#[tauri::command]
pub fn list_highlights(state: State<'_, Arc<AppState>>) -> Result<Vec<HighlightRule>, AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::settings::list_highlights(state.inner().as_ref())
}

#[tauri::command]
pub fn add_highlight(state: State<'_, Arc<AppState>>, rule: HighlightRule) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::settings::add_highlight(state.inner().as_ref(), rule)
}

#[tauri::command]
pub fn remove_highlight(state: State<'_, Arc<AppState>>, keyword: String) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::settings::remove_highlight(state.inner().as_ref(), keyword)
}

#[tauri::command]
pub fn update_highlight(
    state: State<'_, Arc<AppState>>,
    old_keyword: String,
    rule: HighlightRule,
) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::settings::update_highlight(state.inner().as_ref(), old_keyword, rule)
}

#[tauri::command]
pub fn load_snippets(state: State<'_, Arc<AppState>>) -> Result<Vec<Snippet>, AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::settings::load_snippets(state.inner().as_ref())
}

#[tauri::command]
pub fn save_snippets(
    state: State<'_, Arc<AppState>>,
    snippets: Vec<Snippet>,
) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::settings::save_snippets(state.inner().as_ref(), snippets)
}

#[tauri::command]
pub fn reset_highlights(state: State<'_, Arc<AppState>>) -> Result<(), AppError> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::settings::reset_highlights(state.inner().as_ref())
}

#[tauri::command]
pub fn secret_backend(state: State<'_, Arc<AppState>>) -> String {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::settings::secret_backend(state.inner().as_ref())
}

#[tauri::command]
pub async fn list_fonts() -> Vec<FontInfo> {
    rssh_core::commands::settings::list_fonts().await
}

#[tauri::command]
pub fn list_recordings(state: State<'_, Arc<AppState>>) -> AppResult<Vec<String>> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::settings::list_recordings(state.inner().as_ref())
}

#[tauri::command]
pub fn read_recording(state: State<'_, Arc<AppState>>, name: String) -> AppResult<String> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::settings::read_recording(state.inner().as_ref(), name)
}
