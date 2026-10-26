use rssh_core::commands::sftp::{get_sftp, local_open_error, CancelGuard};
use rssh_core::error::AppResult;
use rssh_core::ssh::sftp::{FileStat, RemoteEntry, WalkEntry};
use rssh_core::state::AppState;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub async fn sftp_connect(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    host: String,
    port: u16,
    username: String,
    auth_type: String,
    secret: Option<String>,
) -> AppResult<String> {
    rssh_core::commands::sftp::sftp_connect(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        host,
        port,
        username,
        auth_type,
        secret,
    )
    .await
}

#[tauri::command]
pub async fn sftp_connect_session(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> AppResult<String> {
    rssh_core::commands::sftp::sftp_connect_session(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        session_id,
    )
    .await
}

#[tauri::command]
pub async fn sftp_home(state: State<'_, Arc<AppState>>, sftp_id: String) -> AppResult<String> {
    rssh_core::commands::sftp::sftp_home(state.inner().as_ref(), sftp_id).await
}

#[tauri::command]
pub async fn sftp_list(
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    path: String,
) -> AppResult<Vec<RemoteEntry>> {
    rssh_core::commands::sftp::sftp_list(state.inner().as_ref(), sftp_id, path).await
}

#[tauri::command]
pub async fn sftp_walk_remote_dir(
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    remote_root: String,
) -> AppResult<Vec<WalkEntry>> {
    rssh_core::commands::sftp::sftp_walk_remote_dir(state.inner().as_ref(), sftp_id, remote_root)
        .await
}

#[tauri::command]
pub async fn walk_local_dir(local_root: String) -> AppResult<Vec<crate::files::LocalWalkEntry>> {
    crate::files::walk_directory(local_root).await
}

#[tauri::command]
pub async fn sftp_download(
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    path: String,
) -> AppResult<Vec<u8>> {
    rssh_core::commands::sftp::sftp_download(state.inner().as_ref(), sftp_id, path).await
}

#[tauri::command]
pub async fn sftp_upload(
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    path: String,
    data: Vec<u8>,
) -> AppResult<()> {
    rssh_core::commands::sftp::sftp_upload(state.inner().as_ref(), sftp_id, path, data).await
}

#[tauri::command]
pub async fn sftp_mkdir(
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    path: String,
) -> AppResult<()> {
    rssh_core::commands::sftp::sftp_mkdir(state.inner().as_ref(), sftp_id, path).await
}

#[tauri::command]
pub async fn sftp_close(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
) -> AppResult<()> {
    rssh_core::commands::sftp::sftp_close(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        sftp_id,
    )
    .await
}

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

#[tauri::command]
pub async fn sftp_download_to(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    remote_path: String,
    local_path: String,
    transfer_id: String,
) -> AppResult<()> {
    let sftp = get_sftp(&state, &sftp_id)?;
    let (_guard, cancel) = CancelGuard::register(&state, transfer_id.clone())?;
    let host = crate::tauri_host::host(app.clone());
    match crate::files::download_target(&app, local_path)
        .await
        .map_err(local_open_error)?
    {
        crate::files::DownloadTarget::AtomicPath(path) => sftp
            .download_streaming(&remote_path, &path, &host, &transfer_id, cancel)
            .await
            .map(|_| ()),
        crate::files::DownloadTarget::Provider(location) => sftp
            .download_streaming_to_writer(
                &remote_path,
                || async {
                    crate::files::open_file(&app, location, true)
                        .await
                        .map_err(local_open_error)
                },
                &host,
                &transfer_id,
                cancel,
            )
            .await
            .map(|_| ()),
    }
}

#[tauri::command]
pub async fn sftp_upload_from(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    local_path: String,
    remote_path: String,
    transfer_id: String,
) -> AppResult<()> {
    let sftp = get_sftp(&state, &sftp_id)?;
    let (_guard, cancel) = CancelGuard::register(&state, transfer_id.clone())?;
    let host = crate::tauri_host::host(app.clone());
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
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    path: String,
) -> AppResult<()> {
    rssh_core::commands::sftp::sftp_remove(state.inner().as_ref(), sftp_id, path).await
}

#[tauri::command]
pub async fn sftp_rename(
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    old_path: String,
    new_path: String,
) -> AppResult<()> {
    rssh_core::commands::sftp::sftp_rename(state.inner().as_ref(), sftp_id, old_path, new_path)
        .await
}

#[tauri::command]
pub async fn sftp_stat(
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    path: String,
) -> AppResult<FileStat> {
    rssh_core::commands::sftp::sftp_stat(state.inner().as_ref(), sftp_id, path).await
}

#[tauri::command]
pub fn sftp_cancel_transfer(state: State<'_, Arc<AppState>>, transfer_id: String) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::sftp::sftp_cancel_transfer(state.inner().as_ref(), transfer_id)
}
