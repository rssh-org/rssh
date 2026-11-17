use std::sync::Arc;

use crate::aws::ssm::{self, SsmHandle, SsmOut, SsmSink};
use crate::emitter::Host;
use crate::error::{locked, AppError, AppResult};
use crate::state::{AppState, SessionKind, SessionOwner, SessionPhase};

#[allow(clippy::too_many_arguments)]
pub async fn aws_ssm_open(
    host: Host,
    owner: SessionOwner,
    state: &AppState,
    source_id: String,
    region: String,
    instance_id: String,
    account_id: String,
    cols: u16,
    rows: u16,
    session_id: Option<String>,
) -> AppResult<String> {
    crate::aws::api::validate_region(&region)?;
    if !instance_id.starts_with("i-")
        || instance_id.len() > 32
        || !instance_id[2..].bytes().all(|b| b.is_ascii_hexdigit())
        || instance_id.len() <= 2
        || account_id.len() != 12
        || !account_id.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(AppError::config(
            "aws_ssm_protocol_error",
            serde_json::json!({"reason":"invalid target identity"}),
        ));
    }
    let id = super::lifecycle::resolve_session_id(session_id)?;
    let reservation =
        super::lifecycle::reserve_resource(state, &id, SessionKind::Ssm, owner.clone())?;
    reservation.bind_discovery_source(&source_id)?;
    let operation = reservation.pending_operation()?;
    // Logout/deletion owns the write side. Register first, then wait for this
    // gate cancellably: a source closer can otherwise deadlock waiting for an
    // opener which is itself waiting for the closer's write lock.
    let _source_gate = tokio::select! {
        biased;
        _ = operation.cancelled() => {
            // No external operation has been dispatched or retained yet.
            operation.complete(Ok(()));
            return Err(AppError::other("aws_ssm_open_cancelled", serde_json::json!({})));
        }
        guard = state.aws_auth.source_gate.read() => guard,
    };
    let sink: SsmSink = Arc::new(move |id, event| match event {
        SsmOut::Data(bytes) => {
            if host.emit_bytes(&format!("ssm:data:{id}"), &bytes).is_err() {
                // A detached browser/window must not leave the AWS session open.
                let _ =
                    super::lifecycle::close_resource(host.state(), id, SessionKind::Ssm, &owner);
            }
        }
        SsmOut::Error(error) => {
            let _ = host.emit(&format!("ssm:error:{id}"), error.to_string());
        }
        SsmOut::Close => {
            let _ = host.emit(&format!("ssm:close:{id}"), ());
            // The actor has acknowledged exit before this event. Registry-owned
            // cleanup terminates the remote session and retries API failures.
            if let Err(error) =
                super::lifecycle::close_resource(host.state(), id, SessionKind::Ssm, &owner)
            {
                log::warn!("SSM lifecycle cleanup failed: {error}");
            }
        }
    });
    let opened = ssm::open_resource(
        state.aws_auth.clone(),
        id,
        source_id,
        region,
        instance_id,
        account_id,
        cols,
        rows,
        sink,
        operation,
    )
    .await?;
    opened
        .activate(|id, handle| {
            reservation.activate_returned(id, super::lifecycle::ReadySession::Ssm(handle))
        })
        .await
}

fn owned_handle(state: &AppState, owner: &SessionOwner, id: &str) -> AppResult<SsmHandle> {
    let registry = locked(&state.lifecycle_sessions)?;
    let record = registry
        .get(id)
        .ok_or_else(|| AppError::not_found("aws_ssm_not_found", serde_json::json!({})))?;
    if &record.owner != owner {
        return Err(AppError::config(
            "session_owner_mismatch",
            serde_json::json!({"id":id}),
        ));
    }
    if record.kind != SessionKind::Ssm || record.phase() != SessionPhase::Ready {
        return Err(AppError::not_found(
            "aws_ssm_not_found",
            serde_json::json!({}),
        ));
    }
    locked(&state.ssm_sessions)?
        .get(id)
        .cloned()
        .ok_or_else(|| AppError::not_found("aws_ssm_not_found", serde_json::json!({})))
}

pub fn aws_ssm_write(
    owner: SessionOwner,
    state: &AppState,
    session_id: String,
    data: Vec<u8>,
) -> AppResult<()> {
    owned_handle(state, &owner, &session_id)?.write(&data)
}

pub fn aws_ssm_resize(
    owner: SessionOwner,
    state: &AppState,
    session_id: String,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    owned_handle(state, &owner, &session_id)?.resize(cols, rows)
}

pub async fn aws_ssm_close(
    owner: SessionOwner,
    state: &AppState,
    session_id: String,
) -> AppResult<()> {
    super::lifecycle::close_resource_and_wait(state, &session_id, SessionKind::Ssm, &owner).await
}
