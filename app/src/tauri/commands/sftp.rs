use rssh_core::commands::sftp::{
    local_open_error,
    queue::{Direction, TransferRequest},
};
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
    transfer_id: Option<String>,
) -> AppResult<String> {
    rssh_core::commands::sftp::sftp_connect_session(
        rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        session_id,
        transfer_id,
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
pub async fn walk_local_dir(
    app: tauri::AppHandle,
    local_root: String,
) -> AppResult<Vec<crate::files::LocalWalkEntry>> {
    crate::platform::host::platform(&app)
        .walk_directory(local_root)
        .await
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
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    path: String,
    data: Vec<u8>,
) -> AppResult<()> {
    rssh_core::commands::sftp::sftp_upload(
        &rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        state.inner().as_ref(),
        sftp_id,
        path,
        data,
    )
    .await
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
    window: tauri::Window,
    sftp_id: String,
    remote_path: String,
    local_path: String,
    transfer_id: String,
    original_session_id: Option<String>,
) -> AppResult<()> {
    let request = TransferRequest {
        sftp_id,
        remote_path,
        local_path,
        transfer_id,
        original_session_id,
    };
    let host = crate::tauri_host::host(app.clone());
    let owner = rssh_core::state::SessionOwner::Window(window.label().to_owned());
    let transfer = request.begin(host.state(), &owner, Direction::Download)?;
    let (dir, name) = crate::files::transfer_destination(&app, request.local_path.clone())
        .await
        .map_err(local_open_error)?;
    transfer.download(&host, dir.as_ref(), &name).await
}

#[tauri::command]
pub async fn sftp_upload_from(
    app: tauri::AppHandle,
    window: tauri::Window,
    sftp_id: String,
    local_path: String,
    remote_path: String,
    transfer_id: String,
    original_session_id: Option<String>,
) -> AppResult<()> {
    let request = TransferRequest {
        sftp_id,
        remote_path,
        local_path,
        transfer_id,
        original_session_id,
    };
    let host = crate::tauri_host::host(app.clone());
    let owner = rssh_core::state::SessionOwner::Window(window.label().to_owned());
    let transfer = request.begin(host.state(), &owner, Direction::Upload)?;
    let mut source = crate::files::open_file(&app, request.local_path.clone(), false)
        .await
        .map_err(local_open_error)?;
    transfer.upload(&host, source.stream()).await
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
pub fn sftp_cancel_transfer(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    transfer_id: String,
) -> AppResult<()> {
    state.transfers.cancel(
        &rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        &transfer_id,
    )
}

#[tauri::command]
pub fn sftp_forget_transfer(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    transfer_id: String,
) -> AppResult<Option<String>> {
    state.transfers.forget(
        &rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        &transfer_id,
    )
}
