use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_json::json;

use crate::error::{locked, AppError, AppResult};
use crate::models::{Credential, CredentialType};
use crate::ssh::sftp::{FileStat, RemoteEntry, SftpHandle, WalkEntry};
use crate::state::AppState;
use crate::state::SessionKind;

/// RAII：注册 cancel flag 并在 drop 时自动 unregister，无论 streaming 正常返回、
/// 早 `?`、还是 panic。替代旧的手写 register/unregister 配对。
pub struct CancelGuard<'a> {
    state: &'a AppState,
    transfer_id: String,
}

impl<'a> CancelGuard<'a> {
    /// 注册 flag。返回 (guard, flag)：guard 控生命周期，flag 喂给 streaming 函数。
    /// `pub` 让 headless server 复用同一套 RAII 清理（drop 时 unregister，覆盖
    /// 正常返回 / 早 `?` / panic 三种路径），避免手写 register/remove 漏删。
    pub fn register(
        state: &'a AppState,
        transfer_id: String,
    ) -> AppResult<(Self, Arc<AtomicBool>)> {
        let flag = Arc::new(AtomicBool::new(false));
        locked(&state.transfer_cancels)?.insert(transfer_id.clone(), flag.clone());
        Ok((Self { state, transfer_id }, flag))
    }
}

impl Drop for CancelGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut m) = locked(&self.state.transfer_cancels) {
            m.remove(&self.transfer_id);
        }
    }
}

pub async fn sftp_connect(
    owner: crate::state::SessionOwner,
    state: &AppState,
    host: String,
    port: u16,
    username: String,
    auth_type: String,
    secret: Option<String>,
) -> AppResult<String> {
    let reservation =
        crate::commands::lifecycle::reserve_generated_resource(&state, SessionKind::Sftp, owner)?;
    let id = reservation.id().to_owned();
    let cred = Credential {
        id: String::new(),
        name: String::new(),
        username,
        credential_type: CredentialType::from_str(&auth_type),
        secret,
        save_to_remote: false,
    };

    let timeout_secs: u64 = crate::db::settings::get(&state.db, "connect_timeout")?
        .and_then(|v| v.parse().ok())
        .unwrap_or(crate::ssh::client::DEFAULT_CONNECT_TIMEOUT);

    let known_hosts_path = crate::ssh::known_hosts::path_for(&state.data_dir);
    let handle = crate::ssh::client::run_blocking_ssh(move || async move {
        SftpHandle::connect(host, port, cred, known_hosts_path, timeout_secs).await
    })
    .await?;
    reservation.activate(crate::commands::lifecycle::ReadySession::Sftp(Arc::new(
        handle,
    )))?;

    Ok(id)
}

/// Connect SFTP by reusing an active SSH session (no re-authentication).
pub async fn sftp_connect_session(
    owner: crate::state::SessionOwner,
    state: &AppState,
    session_id: String,
) -> AppResult<String> {
    let (reservation, ssh_handle) =
        crate::commands::lifecycle::reserve_sftp_child(&state, &session_id, &owner)?;
    let id = reservation.id().to_owned();

    let parent = session_id.clone();
    let handle = crate::ssh::client::run_blocking_ssh(move || async move {
        SftpHandle::from_handle(&ssh_handle, parent).await
    })
    .await?;
    reservation.activate(crate::commands::lifecycle::ReadySession::Sftp(Arc::new(
        handle,
    )))?;

    Ok(id)
}

/// 从 Mutex 中 clone 出 Arc<SftpHandle>，释放锁后再 await。
pub fn get_sftp(state: &AppState, sftp_id: &str) -> AppResult<Arc<SftpHandle>> {
    locked(&state.sftp_sessions)?
        .get(sftp_id)
        .cloned()
        .ok_or_else(|| AppError::not_found("sftp_session_not_found", json!({})))
}

pub async fn sftp_home(state: &AppState, sftp_id: String) -> AppResult<String> {
    let h = get_sftp(&state, &sftp_id)?;
    h.home_dir().await
}

pub async fn sftp_list(
    state: &AppState,
    sftp_id: String,
    path: String,
) -> AppResult<Vec<RemoteEntry>> {
    let h = get_sftp(&state, &sftp_id)?;
    h.list_dir(&path).await
}

/// Recursively list every file under a remote directory (symlink-to-file is
/// followed, symlink-to-dir is skipped to prevent cycles). The frontend queues
/// each returned entry as an independent Transfer; the directory abstraction
/// exists only inside this command.
pub async fn sftp_walk_remote_dir(
    state: &AppState,
    sftp_id: String,
    remote_root: String,
) -> AppResult<Vec<WalkEntry>> {
    let h = get_sftp(&state, &sftp_id)?;
    h.walk_files(&remote_root).await
}

/// The file adapter owns local filesystem and authorized URI traversal.

pub async fn sftp_download(state: &AppState, sftp_id: String, path: String) -> AppResult<Vec<u8>> {
    let h = get_sftp(&state, &sftp_id)?;
    h.download(&path).await
}

pub async fn sftp_upload(
    state: &AppState,
    sftp_id: String,
    path: String,
    data: Vec<u8>,
) -> AppResult<()> {
    let h = get_sftp(&state, &sftp_id)?;
    h.upload(&path, &data).await
}

pub async fn sftp_mkdir(state: &AppState, sftp_id: String, path: String) -> AppResult<()> {
    let h = get_sftp(&state, &sftp_id)?;
    h.mkdir(&path).await
}

pub async fn sftp_close(
    owner: crate::state::SessionOwner,
    state: &AppState,
    sftp_id: String,
) -> AppResult<()> {
    crate::commands::lifecycle::close_resource(&state, &sftp_id, SessionKind::Sftp, &owner)
}

/// Pick a destination or source using the native host; keep authorization URIs intact.

/// Select a source or destination directory and retain its platform grant.

// File access keeps its native errors for non-SFTP callers. Preserve SFTP's
// existing open-error contract without nesting a coded error inside params.
pub fn local_open_error(error: AppError) -> AppError {
    match error {
        AppError::Io(cause) => AppError::sftp(
            "sftp_io_failed",
            json!({ "op": "open", "err": cause.params["err"] }),
        ),
        other => other,
    }
}

/// Stream-download to a caller-supplied local target. transfer_id is used as the
/// `sftp:progress:{transfer_id}` event suffix (R1) so the frontend listens
/// per-transfer instead of multiplexing one global stream.

/// Stream-upload from a caller-supplied local source. transfer_id mirrors above.

pub async fn sftp_remove(state: &AppState, sftp_id: String, path: String) -> AppResult<()> {
    let h = get_sftp(&state, &sftp_id)?;
    h.remove(&path).await
}

pub async fn sftp_rename(
    state: &AppState,
    sftp_id: String,
    old_path: String,
    new_path: String,
) -> AppResult<()> {
    let h = get_sftp(&state, &sftp_id)?;
    h.rename(&old_path, &new_path).await
}

pub async fn sftp_stat(state: &AppState, sftp_id: String, path: String) -> AppResult<FileStat> {
    let h = get_sftp(&state, &sftp_id)?;
    h.stat(&path).await
}

/// 用户在传输页点"取消"调用：把 transfer_id 对应的 cancel flag 置 1，
/// streaming 循环下一次 chunk 检查时退出。
pub fn sftp_cancel_transfer(state: &AppState, transfer_id: String) -> AppResult<()> {
    use std::sync::atomic::Ordering;
    if let Some(flag) = locked(&state.transfer_cancels)?.get(&transfer_id) {
        flag.store(true, Ordering::SeqCst);
    }
    Ok(())
}

#[cfg(test)]
mod file_error_tests {
    use super::*;

    #[test]
    fn local_open_failure_preserves_sftp_wire_error_without_nested_encoding() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let error = local_open_error(io.into());
        let AppError::Sftp(cause) = error else {
            panic!("expected the existing SFTP open error");
        };
        assert_eq!(cause.code, "sftp_io_failed");
        assert_eq!(
            cause.params,
            json!({ "op": "open", "err": "access denied" })
        );
    }

    #[test]
    fn provider_authorization_errors_keep_their_original_contract() {
        let error = AppError::other("ohos_native_failed", json!({"err": "not selected"}));
        let expected = error.to_string();
        assert_eq!(local_open_error(error).to_string(), expected);
    }
}
