use rssh_core::ai::codex_auth;
use rssh_core::ai::command_blacklist::CategoryGroup;
use rssh_core::ai::commands::{
    AiProviderInfo, AiProviderPatch, AiSessionInfo, AiSettings, AiSettingsPatch, AiTarget,
};
use rssh_core::ai::llm;
use rssh_core::ai::redact_rules::RedactRuleRecord;
use rssh_core::ai::session::{self};
use rssh_core::ai::skills::SkillRecord;
use rssh_core::error::AppResult;
use rssh_core::state::AppState;
use std::sync::Arc;
use tauri::{AppHandle, State};

#[tauri::command]
pub async fn ai_codex_auth_status(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
) -> AppResult<codex_auth::AuthStatus> {
    rssh_core::ai::commands::ai_codex_auth_status(state.inner().as_ref(), provider_id).await
}

#[tauri::command]
pub async fn ai_codex_login_start(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
) -> AppResult<codex_auth::DeviceLogin> {
    rssh_core::ai::commands::ai_codex_login_start(state.inner().as_ref(), provider_id).await
}

#[tauri::command]
pub async fn ai_codex_login_poll(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
    login_id: String,
) -> AppResult<codex_auth::LoginPoll> {
    rssh_core::ai::commands::ai_codex_login_poll(state.inner().as_ref(), provider_id, login_id)
        .await
}

#[tauri::command]
pub async fn ai_codex_login_cancel(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
    login_id: String,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_codex_login_cancel(state.inner().as_ref(), provider_id, login_id)
        .await
}

#[tauri::command]
pub async fn ai_codex_logout(
    state: State<'_, Arc<AppState>>,
    provider_id: String,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_codex_logout(state.inner().as_ref(), provider_id).await
}

#[tauri::command]
pub async fn ai_list_skills(state: State<'_, Arc<AppState>>) -> AppResult<Vec<SkillRecord>> {
    rssh_core::ai::commands::ai_list_skills(state.inner().as_ref()).await
}

#[tauri::command]
pub async fn ai_get_skill(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> AppResult<Option<SkillRecord>> {
    rssh_core::ai::commands::ai_get_skill(state.inner().as_ref(), id).await
}

#[tauri::command]
pub async fn ai_save_skill(
    state: State<'_, Arc<AppState>>,
    id: String,
    name: String,
    description: String,
    content: String,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_save_skill(state.inner().as_ref(), id, name, description, content)
        .await
}

#[tauri::command]
pub async fn ai_delete_skill(state: State<'_, Arc<AppState>>, id: String) -> AppResult<()> {
    rssh_core::ai::commands::ai_delete_skill(state.inner().as_ref(), id).await
}

#[tauri::command]
pub async fn ai_list_redact_rules(
    state: State<'_, Arc<AppState>>,
) -> AppResult<Vec<RedactRuleRecord>> {
    rssh_core::ai::commands::ai_list_redact_rules(state.inner().as_ref()).await
}

#[tauri::command]
pub async fn ai_save_redact_rule(
    state: State<'_, Arc<AppState>>,
    id: String,
    pattern: String,
    replacement: String,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_save_redact_rule(state.inner().as_ref(), id, pattern, replacement)
        .await
}

#[tauri::command]
pub async fn ai_delete_redact_rule(state: State<'_, Arc<AppState>>, id: String) -> AppResult<()> {
    rssh_core::ai::commands::ai_delete_redact_rule(state.inner().as_ref(), id).await
}

#[tauri::command]
pub async fn ai_list_command_blacklist(
    state: State<'_, Arc<AppState>>,
) -> AppResult<Vec<CategoryGroup>> {
    rssh_core::ai::commands::ai_list_command_blacklist(state.inner().as_ref()).await
}

#[tauri::command]
pub async fn ai_replace_command_blacklist(
    state: State<'_, Arc<AppState>>,
    category: String,
    names: Vec<String>,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_replace_command_blacklist(state.inner().as_ref(), category, names)
        .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn ai_session_start(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    target: AiTarget,
    skill: String,
    provider: String,
    model: String,
    locale: Option<String>,
    resume: Option<String>,
) -> AppResult<AiSessionInfo> {
    rssh_core::ai::commands::ai_session_start(
        crate::tauri_host::host(app),
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        target,
        skill,
        provider,
        model,
        locale,
        resume,
    )
    .await
}

#[tauri::command]
pub async fn ai_user_message(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    text: String,
    instance_id: Option<String>,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_user_message(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        text,
        instance_id,
    )
    .await
}

#[tauri::command]
pub async fn ai_command_result(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    tool_call_id: String,
    exit_code: i32,
    output: String,
    timed_out: bool,
    early_terminated: Option<bool>,
    instance_id: Option<String>,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_command_result(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        tool_call_id,
        exit_code,
        output,
        timed_out,
        early_terminated,
        instance_id,
    )
    .await
}

#[tauri::command]
pub async fn ai_command_reject(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    tool_call_id: String,
    reason: String,
    instance_id: Option<String>,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_command_reject(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        tool_call_id,
        reason,
        instance_id,
    )
    .await
}

#[tauri::command]
pub async fn ai_session_prepare_stop(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    instance_id: Option<String>,
) -> AppResult<Vec<session::AiTerminalMutation>> {
    rssh_core::ai::commands::ai_session_prepare_stop(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        instance_id,
    )
    .await
}

#[tauri::command]
pub async fn ai_session_stop(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    instance_id: Option<String>,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_session_stop(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        instance_id,
    )
    .await
}

#[tauri::command]
pub async fn ai_session_clear_context(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    instance_id: Option<String>,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_session_clear_context(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        instance_id,
    )
    .await
}

#[tauri::command]
pub async fn ai_session_rollback_context(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    user_message_index: usize,
    expected_user_messages: Vec<String>,
    instance_id: Option<String>,
) -> AppResult<Vec<session::AiTerminalMutation>> {
    rssh_core::ai::commands::ai_session_rollback_context(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        user_message_index,
        expected_user_messages,
        instance_id,
    )
    .await
}

#[tauri::command]
pub async fn ai_remote_shell_probe_needed(
    state: State<'_, Arc<AppState>>,
    target_id: String,
) -> AppResult<bool> {
    rssh_core::ai::commands::ai_remote_shell_probe_needed(state.inner().as_ref(), target_id).await
}

#[tauri::command]
pub async fn ai_cache_remote_shell(
    state: State<'_, Arc<AppState>>,
    target_id: String,
    shell: rssh_core::ai::shell::ShellKind,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_cache_remote_shell(state.inner().as_ref(), target_id, shell).await
}

#[tauri::command]
pub async fn ai_session_rebind_target(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    target: AiTarget,
    conversation_id: Option<String>,
    instance_id: Option<String>,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_session_rebind_target(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        target,
        conversation_id,
        instance_id,
    )
    .await
}

#[tauri::command]
pub async fn ai_cancel_stream(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    instance_id: Option<String>,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_cancel_stream(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        instance_id,
    )
    .await
}

#[tauri::command]
pub async fn ai_audit_save(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    file_path: String,
    instance_id: Option<String>,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_audit_save(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        file_path,
        instance_id,
    )
    .await
}

#[tauri::command]
pub async fn ai_audit_log_text(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    instance_id: Option<String>,
) -> AppResult<String> {
    rssh_core::ai::commands::ai_audit_log_text(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        instance_id,
    )
    .await
}

#[tauri::command]
pub async fn ai_audit_get(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    tab_id: String,
    instance_id: Option<String>,
) -> AppResult<rssh_core::ai::audit::AuditLog> {
    rssh_core::ai::commands::ai_audit_get(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        tab_id,
        instance_id,
    )
    .await
}

#[tauri::command]
pub async fn ai_list_sessions(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
) -> AppResult<Vec<AiSessionInfo>> {
    rssh_core::ai::commands::ai_list_sessions(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
    )
    .await
}

#[tauri::command]
pub async fn ai_conversations_list(
    state: State<'_, Arc<AppState>>,
    target: AiTarget,
) -> AppResult<Vec<crate::db::ai_conversation::ConversationMeta>> {
    rssh_core::ai::commands::ai_conversations_list(state.inner().as_ref(), target).await
}

#[tauri::command]
pub async fn ai_conversation_timeline(
    state: State<'_, Arc<AppState>>,
    id: String,
    target: Option<AiTarget>,
) -> AppResult<String> {
    rssh_core::ai::commands::ai_conversation_timeline(state.inner().as_ref(), id, target).await
}

#[tauri::command]
pub async fn ai_conversation_save_timeline(
    state: State<'_, Arc<AppState>>,
    id: String,
    timeline: String,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_conversation_save_timeline(state.inner().as_ref(), id, timeline)
        .await
}

#[tauri::command]
pub async fn ai_conversation_delete(state: State<'_, Arc<AppState>>, id: String) -> AppResult<()> {
    rssh_core::ai::commands::ai_conversation_delete(state.inner().as_ref(), id).await
}

#[tauri::command]
pub async fn ai_settings_get(
    state: State<'_, Arc<AppState>>,
    provider: Option<String>,
) -> AppResult<AiSettings> {
    rssh_core::ai::commands::ai_settings_get(state.inner().as_ref(), provider).await
}

#[tauri::command]
pub async fn ai_list_models(
    state: State<'_, Arc<AppState>>,
    provider_id: Option<String>,
    protocol: String,
    endpoint: String,
    api_key: Option<String>,
) -> AppResult<Vec<llm::ModelInfo>> {
    rssh_core::ai::commands::ai_list_models(
        state.inner().as_ref(),
        provider_id,
        protocol,
        endpoint,
        api_key,
    )
    .await
}

#[tauri::command]
pub async fn ai_provider_list(state: State<'_, Arc<AppState>>) -> AppResult<Vec<AiProviderInfo>> {
    rssh_core::ai::commands::ai_provider_list(state.inner().as_ref()).await
}

#[tauri::command]
pub async fn ai_provider_save(
    state: State<'_, Arc<AppState>>,
    patch: AiProviderPatch,
) -> AppResult<String> {
    rssh_core::ai::commands::ai_provider_save(state.inner().as_ref(), patch).await
}

#[tauri::command]
pub async fn ai_provider_delete(state: State<'_, Arc<AppState>>, id: String) -> AppResult<()> {
    rssh_core::ai::commands::ai_provider_delete(state.inner().as_ref(), id).await
}

#[tauri::command]
pub async fn ai_settings_set(
    state: State<'_, Arc<AppState>>,
    patch: AiSettingsPatch,
) -> AppResult<()> {
    rssh_core::ai::commands::ai_settings_set(state.inner().as_ref(), patch).await
}
