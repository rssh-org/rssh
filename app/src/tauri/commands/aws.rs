//! Thin native adapters for AWS Sign-In and Session Manager.

use rssh_core::aws::auth::{AuthStatus, LoginStart};
use rssh_core::commands::{aws_signin, aws_ssm};
use rssh_core::error::AppResult;
use rssh_core::state::{AppState, SessionOwner};
use std::sync::Arc;
use tauri::State;

fn owner(window: &tauri::Window) -> SessionOwner {
    SessionOwner::Window(window.label().to_owned())
}

#[tauri::command]
pub async fn aws_signin_status(
    state: State<'_, Arc<AppState>>,
    source_id: String,
) -> AppResult<AuthStatus> {
    aws_signin::aws_signin_status(state.inner(), source_id).await
}

#[tauri::command]
pub async fn aws_signin_start(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    source_id: String,
    region: String,
) -> AppResult<LoginStart> {
    aws_signin::aws_signin_start(owner(&window), state.inner(), source_id, region).await
}

#[tauri::command]
pub async fn aws_signin_complete(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    source_id: String,
    login_id: String,
    code: String,
) -> AppResult<AuthStatus> {
    aws_signin::aws_signin_complete(owner(&window), state.inner(), source_id, login_id, code).await
}

#[tauri::command]
pub async fn aws_signin_cancel(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    source_id: String,
    login_id: String,
) -> AppResult<()> {
    aws_signin::aws_signin_cancel(owner(&window), state.inner(), source_id, login_id).await
}

#[tauri::command]
pub async fn aws_signin_logout(
    state: State<'_, Arc<AppState>>,
    source_id: String,
) -> AppResult<()> {
    aws_signin::aws_signin_logout(state.inner().clone(), source_id).await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn aws_ssm_open(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    source_id: String,
    region: String,
    instance_id: String,
    account_id: String,
    cols: u16,
    rows: u16,
    session_id: Option<String>,
) -> AppResult<String> {
    aws_ssm::aws_ssm_open(
        crate::tauri_host::host(app),
        owner(&window),
        state.inner(),
        source_id,
        region,
        instance_id,
        account_id,
        cols,
        rows,
        session_id,
    )
    .await
}

#[tauri::command]
pub async fn aws_ssm_write(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    data: Vec<u8>,
) -> AppResult<()> {
    aws_ssm::aws_ssm_write(owner(&window), state.inner(), session_id, data)
}

#[tauri::command]
pub async fn aws_ssm_resize(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    aws_ssm::aws_ssm_resize(owner(&window), state.inner(), session_id, cols, rows)
}

#[tauri::command]
pub async fn aws_ssm_close(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> AppResult<()> {
    aws_ssm::aws_ssm_close(owner(&window), state.inner(), session_id).await
}
