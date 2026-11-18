//! Host-independent AWS Sign-In commands. Secrets remain in the core store.

use crate::aws::auth::{AuthStatus, LoginStart};
use crate::error::AppResult;
use crate::state::{AppState, SessionOwner};

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

pub async fn aws_signin_logout(state: &AppState, source_id: String) -> AppResult<()> {
    state.aws_auth.logout(&source_id).await
}
