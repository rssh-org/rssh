use rssh_core::commands::sftp_transfer::{self, Direction};
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
        .walk_directory(&app, local_root)
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
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    remote_path: String,
    local_path: String,
    transfer_id: String,
) -> AppResult<()> {
    let (local, local_path) = crate::files::transfer_endpoint(&app, local_path, true).await?;
    let owner = rssh_core::state::SessionOwner::Window(window.label().to_owned());
    let host = crate::tauri_host::host(app);
    sftp_transfer::run(
        &state,
        &owner,
        &host,
        &sftp_id,
        local.as_ref(),
        &local_path,
        &remote_path,
        &transfer_id,
        Direction::Download,
    )
    .await
}

#[tauri::command]
pub async fn sftp_upload_from(
    app: tauri::AppHandle,
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    local_path: String,
    remote_path: String,
    transfer_id: String,
) -> AppResult<()> {
    let (local, local_path) = crate::files::transfer_endpoint(&app, local_path, false).await?;
    let owner = rssh_core::state::SessionOwner::Window(window.label().to_owned());
    let host = crate::tauri_host::host(app);
    sftp_transfer::run(
        &state,
        &owner,
        &host,
        &sftp_id,
        local.as_ref(),
        &local_path,
        &remote_path,
        &transfer_id,
        Direction::Upload,
    )
    .await
}

#[tauri::command]
pub fn sftp_forget_transfer(
    window: tauri::Window,
    state: State<'_, Arc<AppState>>,
    transfer_id: String,
) -> AppResult<()> {
    state.sftp_transfers.forget(
        &rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        &transfer_id,
    )
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
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::sftp::sftp_cancel_transfer(
        state.inner().as_ref(),
        &rssh_core::state::SessionOwner::Window(window.label().to_owned()),
        transfer_id,
    )
}
