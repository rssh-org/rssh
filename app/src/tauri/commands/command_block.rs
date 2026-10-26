use rssh_core::db::command_block_redact_rule::RedactRuleRow;
use rssh_core::error::AppResult;
use rssh_core::state::AppState;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn command_block_list_redact_rules(
    state: State<'_, Arc<AppState>>,
) -> AppResult<Vec<RedactRuleRow>> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::command_block::command_block_list_redact_rules(state.inner().as_ref())
}

#[tauri::command]
pub fn command_block_save_redact_rule(
    state: State<'_, Arc<AppState>>,
    id: String,
    pattern: String,
    replacement: String,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::command_block::command_block_save_redact_rule(
        state.inner().as_ref(),
        id,
        pattern,
        replacement,
    )
}

#[tauri::command]
pub fn command_block_delete_redact_rule(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::command_block::command_block_delete_redact_rule(state.inner().as_ref(), id)
}
