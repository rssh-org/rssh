use serde_json::json;
use tauri::State;

use crate::error::{locked, AppError, AppResult};
use crate::models::{Credential, Forward, Profile};
use crate::ssh::forward as fwd;
use crate::state::{AppState, SessionKind, SessionOwner, SessionPhase};

#[tauri::command]
pub fn list_forwards(state: State<AppState>) -> Result<Vec<Forward>, AppError> {
    crate::db::forward::list(&state.db)
}

#[tauri::command]
pub fn get_forward(state: State<AppState>, id: String) -> Result<Forward, AppError> {
    crate::db::forward::get(&state.db, &id)
}

#[tauri::command]
pub fn create_forward(state: State<AppState>, forward: Forward) -> Result<(), AppError> {
    crate::db::forward::insert(&state.db, &forward)
}

#[tauri::command]
pub fn update_forward(state: State<AppState>, forward: Forward) -> Result<(), AppError> {
    crate::db::forward::update(&state.db, &forward)
}

#[tauri::command]
pub fn delete_forward(state: State<AppState>, id: String) -> Result<(), AppError> {
    crate::db::forward::delete(&state.db, &id)
}

// ---------------------------------------------------------------------------
// 活跃端口转发 — 启动 / 停止
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn forward_start(
    window: tauri::Window,
    state: State<'_, AppState>,
    forward_id: String,
) -> AppResult<String> {
    forward_start_impl(
        &state,
        SessionOwner::Window(window.label().to_owned()),
        forward_id,
    )
    .await
}

/// Transport-agnostic body shared by the Tauri command and the headless server.
/// Forwarding emits no events, so it needs no `Host` — only `AppState`.
pub async fn forward_start_impl(
    state: &AppState,
    owner: SessionOwner,
    forward_id: String,
) -> AppResult<String> {
    let reservation =
        crate::commands::lifecycle::reserve_generated_resource(state, SessionKind::Forward, owner)?;
    let active_id = reservation.id().to_owned();
    let f = crate::db::forward::get(&state.db, &forward_id)?;
    let p = crate::db::profile::get(&state.db, &f.profile_id).map_err(|e| match e {
        AppError::NotFound(_) => AppError::not_found("fwd_profile_not_found", json!({})),
        other => other,
    })?;
    let mut c = crate::db::credential::get(&state.db, &p.credential_id).map_err(|e| match e {
        AppError::NotFound(_) => AppError::not_found("fwd_cred_not_found", json!({})),
        other => other,
    })?;
    c.secret = state
        .secret_store
        .get(&crate::secret::cred_secret_key(&c.id))?;
    let timeout_secs: u64 = crate::db::settings::get(&state.db, "connect_timeout")?
        .and_then(|v| v.parse().ok())
        .unwrap_or(crate::ssh::client::DEFAULT_CONNECT_TIMEOUT);

    // 解析 forward target profile 的堡垒机链，每一跳加载 secret
    let chain_profiles = crate::ssh::bastion::resolve_chain(&state.db, &p)?;
    let mut chain: Vec<(Profile, Credential)> = Vec::with_capacity(chain_profiles.len());
    for hop in chain_profiles {
        let mut bc =
            crate::db::credential::get(&state.db, &hop.credential_id).map_err(|e| match e {
                AppError::NotFound(_) => AppError::not_found(
                    "bastion_cred_not_found",
                    json!({ "name": hop.name.clone() }),
                ),
                other => other,
            })?;
        bc.secret = state
            .secret_store
            .get(&crate::secret::cred_secret_key(&bc.id))?;
        chain.push((hop, bc));
    }
    let known_hosts_path = crate::ssh::known_hosts::path_for(&state.data_dir);
    let target = fwd::ConnTarget {
        profile: p,
        credential: c,
        bastion_chain: chain,
        known_hosts_path,
        timeout_secs,
    };
    let handle =
        crate::ssh::client::run_blocking_ssh(move || async move { fwd::start(f, target).await })
            .await?;
    reservation.activate(crate::commands::lifecycle::ReadySession::Forward(handle))?;

    Ok(active_id)
}

#[tauri::command]
pub fn forward_stats(
    window: tauri::Window,
    state: State<'_, AppState>,
    active_id: String,
) -> AppResult<fwd::ForwardStats> {
    forward_stats_impl(
        &state,
        &SessionOwner::Window(window.label().to_owned()),
        active_id,
    )
}

pub fn forward_stats_impl(
    state: &AppState,
    owner: &SessionOwner,
    active_id: String,
) -> AppResult<fwd::ForwardStats> {
    with_ready_forward(state, owner, &active_id, |handle| Ok(handle.stats()))
}

fn with_ready_forward<T>(
    state: &AppState,
    owner: &SessionOwner,
    active_id: &str,
    f: impl FnOnce(&fwd::ForwardHandle) -> AppResult<T>,
) -> AppResult<T> {
    let sessions = locked(&state.lifecycle_sessions)?;
    let record = sessions
        .get(active_id)
        .ok_or_else(|| AppError::not_found("fwd_not_found", json!({ "id": active_id })))?;
    if record.kind != SessionKind::Forward || record.phase != SessionPhase::Ready {
        return Err(AppError::not_found(
            "fwd_not_found",
            json!({ "id": active_id }),
        ));
    }
    if &record.owner != owner {
        return Err(AppError::config(
            "session_owner_mismatch",
            json!({ "id": active_id }),
        ));
    }
    let forwards = locked(&state.active_forwards)?;
    let handle = forwards
        .get(active_id)
        .ok_or_else(|| AppError::not_found("fwd_not_found", json!({ "id": active_id })))?;
    f(handle)
}

async fn forward_rule_command_impl(
    state: &AppState,
    owner: &SessionOwner,
    active_id: String,
    rule_index: usize,
    start: bool,
) -> AppResult<()> {
    let response = with_ready_forward(state, owner, &active_id, |handle| {
        if start {
            Ok(handle.request_rule_start(rule_index)?)
        } else {
            Ok(handle.request_rule_stop(rule_index)?)
        }
    })?;
    response
        .await
        .map_err(|_| AppError::ssh("fwd_session_closed", json!({})))?
}

#[tauri::command]
pub async fn forward_rule_start(
    window: tauri::Window,
    state: State<'_, AppState>,
    active_id: String,
    rule_index: usize,
) -> AppResult<()> {
    forward_rule_command_impl(
        &state,
        &SessionOwner::Window(window.label().to_owned()),
        active_id,
        rule_index,
        true,
    )
    .await
}

#[tauri::command]
pub async fn forward_rule_stop(
    window: tauri::Window,
    state: State<'_, AppState>,
    active_id: String,
    rule_index: usize,
) -> AppResult<()> {
    forward_rule_command_impl(
        &state,
        &SessionOwner::Window(window.label().to_owned()),
        active_id,
        rule_index,
        false,
    )
    .await
}

pub async fn forward_rule_start_impl(
    state: &AppState,
    owner: &SessionOwner,
    active_id: String,
    rule_index: usize,
) -> AppResult<()> {
    forward_rule_command_impl(state, owner, active_id, rule_index, true).await
}

pub async fn forward_rule_stop_impl(
    state: &AppState,
    owner: &SessionOwner,
    active_id: String,
    rule_index: usize,
) -> AppResult<()> {
    forward_rule_command_impl(state, owner, active_id, rule_index, false).await
}

#[tauri::command]
pub fn forward_stop(
    window: tauri::Window,
    state: State<'_, AppState>,
    active_id: String,
) -> AppResult<()> {
    crate::commands::lifecycle::close_resource(
        &state,
        &active_id,
        SessionKind::Forward,
        &SessionOwner::Window(window.label().to_owned()),
    )
}
