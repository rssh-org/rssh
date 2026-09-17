#[cfg(not(ohos))]
use std::collections::VecDeque;
#[cfg(not(ohos))]
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_json::json;
use tauri::State;

use crate::error::{locked, AppError, AppResult};
use crate::models::{Credential, CredentialType};
use crate::ssh::sftp::{FileStat, RemoteEntry, SftpHandle, WalkEntry};
use crate::state::AppState;
use crate::state::{SessionKind, SessionOwner};

/// Maximum recursion depth for the local walker. Mirrors the remote-side cap.
#[cfg(not(ohos))]
const LOCAL_WALK_DEPTH_CAP: u32 = 32;

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

/// Walk a selected source directory. Local locations stay opaque to callers;
/// only rel_path is used to construct the destination on the remote server.
#[tauri::command]
pub async fn walk_local_dir(local_root: String) -> AppResult<Vec<super::files::LocalWalkEntry>> {
    #[cfg(ohos)]
    {
        crate::ohos::files::walk_directory(local_root).await
    }
    #[cfg(not(ohos))]
    {
        walk_filesystem_directory(local_root).await
    }
}

#[cfg(not(ohos))]
async fn walk_filesystem_directory(
    local_root: String,
) -> AppResult<Vec<super::files::LocalWalkEntry>> {
    let root = super::files::filesystem_path(local_root)?;
    let mut queue: VecDeque<(PathBuf, u32)> = VecDeque::new();
    queue.push_back((root.clone(), 0));
    let mut result: Vec<super::files::LocalWalkEntry> = Vec::new();

    while let Some((dir, depth)) = queue.pop_front() {
        if depth >= LOCAL_WALK_DEPTH_CAP {
            return Err(AppError::other(
                "local_tree_too_deep",
                json!({
                    "path": dir.display().to_string(),
                    "depth": depth,
                    "limit": LOCAL_WALK_DEPTH_CAP,
                }),
            ));
        }
        let mut rd = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = rd.next_entry().await? {
            // `entry.metadata()` does not traverse symlinks — single syscall
            // covers both type discrimination and size for regular files,
            // replacing the previous file_type() + metadata() double-stat.
            let path = entry.path();
            let meta = entry.metadata().await?;
            if meta.is_dir() {
                queue.push_back((path, depth + 1));
            } else if meta.is_file() {
                result.push(super::files::LocalWalkEntry {
                    local_path: path.to_string_lossy().into_owned(),
                    rel_path: rel_unix(&path, &root),
                    size: meta.len(),
                });
            } else if meta.is_symlink() {
                // Follow once to learn what the target is. Skip symlink-to-dir
                // to avoid cycles, and silently skip broken symlinks.
                if let Ok(target_meta) = tokio::fs::metadata(&path).await {
                    if target_meta.is_file() {
                        result.push(super::files::LocalWalkEntry {
                            local_path: path.to_string_lossy().into_owned(),
                            rel_path: rel_unix(&path, &root),
                            size: target_meta.len(),
                        });
                    }
                }
            }
            // Anything else (block/char/fifo): skip.
        }
    }
    Ok(result)
}

/// Convert the portion of `full` relative to `root` into a '/'-separated string.
/// On Windows std::path::Component uses '\'; we normalise here and the frontend
/// converts back to the platform separator when joining.
#[cfg(not(ohos))]
fn rel_unix(full: &Path, root: &Path) -> String {
    let stripped = full.strip_prefix(root).unwrap_or(full);
    stripped
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(all(test, not(ohos)))]
mod local_walk_tests {
    use super::*;

    #[tokio::test]
    async fn file_uri_walk_returns_openable_locations_and_preserves_symlink_policy() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("目录 #%");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("nested/file #%.txt"), b"content").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&source, source.join("cycle")).unwrap();
            std::os::unix::fs::symlink(
                source.join("nested/file #%.txt"),
                source.join("file-alias"),
            )
            .unwrap();
        }
        let entries = walk_local_dir(tauri::Url::from_file_path(&source).unwrap().to_string())
            .await
            .unwrap();
        assert_eq!(entries.len(), if cfg!(unix) { 2 } else { 1 });
        assert!(entries
            .iter()
            .any(|entry| entry.rel_path == "nested/file #%.txt"));
        for entry in entries {
            assert_eq!(entry.size, 7);
            assert_eq!(std::fs::read(&entry.local_path).unwrap(), b"content");
            assert_eq!(PathBuf::from(entry.local_path), source.join(entry.rel_path));
        }
    }
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
    super::files::pick_save(&app, default_name, Vec::new()).await
}

#[tauri::command]
pub async fn sftp_pick_open_path(app: tauri::AppHandle) -> AppResult<Option<String>> {
    super::files::pick_open(&app).await
}

/// Select a source or destination directory and retain its platform grant.
#[cfg(any(windows, macos, linux, ohos))]
#[tauri::command]
pub async fn sftp_pick_folder(app: tauri::AppHandle, write: bool) -> AppResult<Option<String>> {
    super::files::pick_folder(&app, write).await
}

#[tauri::command]
pub async fn sftp_pick_open_files(app: tauri::AppHandle) -> AppResult<Option<Vec<String>>> {
    super::files::pick_open_files(&app).await
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
    // Filesystem destinations keep atomic .part + rename. Authorized URIs
    // stream to an owned descriptor because the provider has no rename path.
    #[cfg(not(ohos))]
    if let tauri_plugin_fs::FilePath::Path(path) = local_path
        .parse::<tauri_plugin_fs::FilePath>()
        .expect("FilePath::from_str is infallible")
    {
        return sftp
            .download_streaming(&remote_path, &path, &host, &transfer_id, cancel)
            .await
            .map(|_| ());
    }
    let (file, _access) = super::files::open_file(&app, local_path, true).await?;
    let mut writer = tokio::fs::File::from_std(file);
    sftp.download_streaming_to_writer(&remote_path, &mut writer, &host, &transfer_id, cancel)
        .await
        .map(|_| ())
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
    let (reader, _access) = super::files::open_file(&app, local_path, false).await?;
    let mut reader = tokio::fs::File::from_std(reader);
    // content:// fds may not support fstat; fall back to 0 (indeterminate bar).
    let total = reader.metadata().await.map(|m| m.len()).unwrap_or(0);
    sftp.upload_streaming(
        &mut reader,
        total,
        &remote_path,
        &host,
        &transfer_id,
        cancel,
    )
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
