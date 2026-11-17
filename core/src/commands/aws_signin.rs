//! Host-independent AWS Sign-In commands. Secrets remain in the core store.

use crate::aws::auth::{AuthStatus, LoginStart};
use crate::error::{locked, AppError, AppResult};
use crate::state::{AppState, SessionKind, SessionOwner, SessionPhase};

pub async fn aws_signin_status(state: &AppState, source_id: String) -> AppResult<AuthStatus> {
    state.aws_auth.status_async(&source_id).await
}

pub async fn aws_signin_start(
    owner: SessionOwner,
    state: &AppState,
    source_id: String,
    region: String,
) -> AppResult<LoginStart> {
    state.aws_auth.start(&source_id, &region, &owner).await
}

pub async fn aws_signin_complete(
    owner: SessionOwner,
    state: &AppState,
    source_id: String,
    login_id: String,
    code: String,
) -> AppResult<AuthStatus> {
    state
        .aws_auth
        .complete(&source_id, &login_id, &code, &owner)
        .await
}

pub async fn aws_signin_cancel(
    owner: SessionOwner,
    state: &AppState,
    source_id: String,
    login_id: String,
) -> AppResult<()> {
    state.aws_auth.cancel(&source_id, &login_id, &owner).await
}

pub async fn aws_signin_logout(
    state: std::sync::Arc<AppState>,
    source_id: String,
) -> AppResult<()> {
    // Closing the page must not release the gate halfway through remote cleanup
    // and let another opening session escape the logout operation.
    tokio::spawn(async move {
        let _source_change = state.aws_auth.source_gate.write().await;
        close_source_sessions(&state, &source_id).await?;
        state.aws_auth.logout(&source_id).await
    })
    .await
    .map_err(|error| {
        AppError::other(
            "task_join_failed",
            serde_json::json!({"err":error.to_string()}),
        )
    })?
}

/// Call with the source-change gate held exclusively. Cleanup retains the
/// authenticated grant until every pending or active remote session has ended.
pub(crate) async fn close_source_sessions(state: &AppState, source_id: &str) -> AppResult<()> {
    let sessions: Vec<_> = locked(&state.lifecycle_sessions)?
        .iter()
        .filter(|(_, record)| {
            record.kind == SessionKind::Ssm
                && record.discovery_source_id.as_deref() == Some(source_id)
                && record.phase() != SessionPhase::Closed
        })
        .map(|(id, record)| (id.clone(), record.owner.clone()))
        .collect();
    for (id, owner) in sessions {
        super::lifecycle::close_resource_and_wait(state, &id, SessionKind::Ssm, &owner).await?;
    }
    Ok(())
}
