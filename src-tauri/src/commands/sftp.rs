use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_json::json;
use tauri::State;

use crate::error::{locked, AppError, AppResult};
use crate::models::{Credential, CredentialType};
use crate::ssh::sftp::{FileStat, RemoteEntry, SftpHandle, WalkEntry};
use crate::state::AppState;
use crate::state::{SessionKind, SessionOwner};

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

#[tauri::command]
pub async fn sftp_connect(
    window: tauri::Window,
    state: State<'_, AppState>,
    host: String,
    port: u16,
    username: String,
    auth_type: String,
    secret: Option<String>,
) -> AppResult<String> {
    let reservation = crate::commands::lifecycle::reserve_generated_resource(
        &state,
        SessionKind::Sftp,
        SessionOwner::Window(window.label().to_owned()),
    )?;
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
#[tauri::command]
pub async fn sftp_connect_session(
    window: tauri::Window,
    state: State<'_, AppState>,
    session_id: String,
) -> AppResult<String> {
    let (reservation, ssh_handle) = crate::commands::lifecycle::reserve_sftp_child(
        &state,
        &session_id,
        &SessionOwner::Window(window.label().to_owned()),
    )?;
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
fn get_sftp(state: &State<'_, AppState>, sftp_id: &str) -> AppResult<Arc<SftpHandle>> {
    locked(&state.sftp_sessions)?
        .get(sftp_id)
        .cloned()
        .ok_or_else(|| AppError::not_found("sftp_session_not_found", json!({})))
}

#[tauri::command]
pub async fn sftp_home(state: State<'_, AppState>, sftp_id: String) -> AppResult<String> {
    let h = get_sftp(&state, &sftp_id)?;
    h.home_dir().await
}

#[tauri::command]
pub async fn sftp_list(
    state: State<'_, AppState>,
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
#[tauri::command]
pub async fn sftp_walk_remote_dir(
    state: State<'_, AppState>,
    sftp_id: String,
    remote_root: String,
) -> AppResult<Vec<WalkEntry>> {
    let h = get_sftp(&state, &sftp_id)?;
    h.walk_files(&remote_root).await
}

/// The file adapter owns local filesystem and authorized URI traversal.
#[tauri::command]
pub async fn walk_local_dir(local_root: String) -> AppResult<Vec<crate::files::LocalWalkEntry>> {
    crate::files::walk_directory(local_root).await
}

#[tauri::command]
pub async fn sftp_download(
    state: State<'_, AppState>,
    sftp_id: String,
    path: String,
) -> AppResult<Vec<u8>> {
    let h = get_sftp(&state, &sftp_id)?;
    h.download(&path).await
}

#[tauri::command]
pub async fn sftp_upload(
    state: State<'_, AppState>,
    sftp_id: String,
    path: String,
    data: Vec<u8>,
) -> AppResult<()> {
    let h = get_sftp(&state, &sftp_id)?;
    h.upload(&path, &data).await
}

#[tauri::command]
pub async fn sftp_mkdir(
    state: State<'_, AppState>,
    sftp_id: String,
    path: String,
) -> AppResult<()> {
    let h = get_sftp(&state, &sftp_id)?;
    h.mkdir(&path).await
}

#[tauri::command]
pub async fn sftp_close(
    window: tauri::Window,
    state: State<'_, AppState>,
    sftp_id: String,
) -> AppResult<()> {
    crate::commands::lifecycle::close_resource(
        &state,
        &sftp_id,
        SessionKind::Sftp,
        &SessionOwner::Window(window.label().to_owned()),
    )
}

/// Pick a destination or source using the native host; keep authorization URIs intact.
#[tauri::command]
pub async fn sftp_pick_save_path(
    app: tauri::AppHandle,
    default_name: String,
) -> AppResult<Option<String>> {
    crate::files::pick_save(&app, default_name, Vec::new()).await
}

#[tauri::command]
pub async fn sftp_pick_open_path(
    app: tauri::AppHandle,
) -> AppResult<Option<crate::files::PickedLocation>> {
    crate::files::pick_open(&app).await
}

/// Select a source or destination directory and retain its platform grant.
#[cfg(any(windows, macos, linux, ohos))]
#[tauri::command]
pub async fn sftp_pick_folder(
    app: tauri::AppHandle,
    write: bool,
) -> AppResult<Option<crate::files::PickedLocation>> {
    crate::files::pick_folder(&app, write).await
}

#[tauri::command]
pub async fn sftp_pick_open_files(
    app: tauri::AppHandle,
) -> AppResult<Option<Vec<crate::files::PickedLocation>>> {
    crate::files::pick_open_files(&app).await
}

// File access keeps its native errors for non-SFTP callers. Preserve SFTP's
// existing open-error contract without nesting a coded error inside params.
fn local_open_error(error: AppError) -> AppError {
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
#[tauri::command]
pub async fn sftp_download_to(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    sftp_id: String,
    remote_path: String,
    local_path: String,
    transfer_id: String,
) -> AppResult<()> {
    let sftp = get_sftp(&state, &sftp_id)?;
    let (_guard, cancel) = CancelGuard::register(&state, transfer_id.clone())?;
    let host = crate::emitter::Host::Tauri(app.clone());
    match crate::files::download_target(&app, local_path)
        .await
        .map_err(local_open_error)?
    {
        crate::files::DownloadTarget::AtomicPath(path) => sftp
            .download_streaming(&remote_path, &path, &host, &transfer_id, cancel)
            .await
            .map(|_| ()),
        crate::files::DownloadTarget::Stream(mut file) => sftp
            .download_streaming_to_writer(&remote_path, file.stream(), &host, &transfer_id, cancel)
            .await
            .map(|_| ()),
    }
}

/// Stream-upload from a caller-supplied local source. transfer_id mirrors above.
#[tauri::command]
pub async fn sftp_upload_from(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    sftp_id: String,
    local_path: String,
    remote_path: String,
    transfer_id: String,
) -> AppResult<()> {
    let sftp = get_sftp(&state, &sftp_id)?;
    let (_guard, cancel) = CancelGuard::register(&state, transfer_id.clone())?;
    let host = crate::emitter::Host::Tauri(app.clone());
    let mut file = crate::files::open_file(&app, local_path, false)
        .await
        .map_err(local_open_error)?;
    // content:// fds may not support fstat; fall back to 0 (indeterminate bar).
    let total = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let reader = file.stream();
    sftp.upload_streaming(reader, total, &remote_path, &host, &transfer_id, cancel)
        .await
        .map(|_| ())
}

#[tauri::command]
pub async fn sftp_remove(
    state: State<'_, AppState>,
    sftp_id: String,
    path: String,
) -> AppResult<()> {
    let h = get_sftp(&state, &sftp_id)?;
    h.remove(&path).await
}

#[tauri::command]
pub async fn sftp_rename(
    state: State<'_, AppState>,
    sftp_id: String,
    old_path: String,
    new_path: String,
) -> AppResult<()> {
    let h = get_sftp(&state, &sftp_id)?;
    h.rename(&old_path, &new_path).await
}

#[tauri::command]
pub async fn sftp_stat(
    state: State<'_, AppState>,
    sftp_id: String,
    path: String,
) -> AppResult<FileStat> {
    let h = get_sftp(&state, &sftp_id)?;
    h.stat(&path).await
}

/// 用户在传输页点"取消"调用：把 transfer_id 对应的 cancel flag 置 1，
/// streaming 循环下一次 chunk 检查时退出。
#[tauri::command]
pub fn sftp_cancel_transfer(state: State<'_, AppState>, transfer_id: String) -> AppResult<()> {
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
