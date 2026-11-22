use rssh_core::commands::sftp::{
    local_open_error,
    transfer::{Attempt, Direction, Request},
};
use rssh_core::error::AppResult;
use rssh_core::ssh::sftp::{FileStat, RemoteEntry, WalkEntry};
use rssh_core::state::AppState;
use rssh_core::state::SessionOwner;
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
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    default_name: String,
) -> AppResult<Option<crate::files::PickedDownload>> {
    crate::platform::host::platform(&app)
        .pick_download_path(&window, default_name)
        .await
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
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    remote_path: String,
    local_path: String,
    transfer_id: String,
    grant_id: Option<tauri::ResourceId>,
    original_session_id: Option<String>,
) -> AppResult<u64> {
    #[cfg(not(any(android, ios)))]
    let _ = grant_id;
    let owner = SessionOwner::Window(window.label().to_owned());
    let request = Request {
        transfer_id,
        original_session_id,
        remote_path,
        local_path: local_path.clone(),
        direction: Direction::Download,
    };
    let attempt = Attempt::begin(&state, &owner, &sftp_id, request)?;
    #[cfg(any(android, ios))]
    {
        use crate::platform::tauri::download_folder;
        let grant_id = grant_id.ok_or_else(crate::files::download_siblings_unavailable)?;
        return download_folder::download(
            &window,
            grant_id,
            &local_path,
            attempt,
            crate::tauri_host::host(app),
        )
        .await;
    }
    #[cfg(any(macos, linux, windows, ohos))]
    let path = crate::platform::host::platform(&app)
        .download_target(local_path)
        .await
        .map_err(local_open_error)?;
    #[cfg(any(macos, linux, windows, ohos))]
    attempt.download(&crate::tauri_host::host(app), &path).await
}

#[tauri::command]
pub async fn sftp_upload_from(
    window: tauri::Window,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    sftp_id: String,
    local_path: String,
    remote_path: String,
    transfer_id: String,
    original_session_id: Option<String>,
) -> AppResult<u64> {
    let owner = SessionOwner::Window(window.label().to_owned());
    let request = Request {
        transfer_id,
        original_session_id,
        remote_path,
        local_path: local_path.clone(),
        direction: Direction::Upload,
    };
    let attempt = Attempt::begin(&state, &owner, &sftp_id, request)?;
    let mut file = crate::files::open_file(&app, local_path, false)
        .await
        .map_err(local_open_error)?;
    attempt
        .upload(&crate::tauri_host::host(app), file.stream())
        .await
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
    let owner = SessionOwner::Window(window.label().to_owned());
    rssh_core::error::locked(&state.transfers)?.cancel(&owner, &transfer_id);
    Ok(())
}

#[tauri::command]
pub fn sftp_forget_transfer(
    window: tauri::WebviewWindow,
    grant_id: Option<tauri::ResourceId>,
    state: State<'_, Arc<AppState>>,
    transfer_id: String,
) -> AppResult<()> {
    #[cfg(not(any(android, ios)))]
    let _ = grant_id;
    let owner = SessionOwner::Window(window.label().to_owned());
    rssh_core::error::locked(&state.transfers)?.forget(&owner, &transfer_id)?;
    #[cfg(any(android, ios))]
    if let Some(id) = grant_id {
        use tauri::Manager;
        let _ = window
            .resources_table()
            .take::<crate::platform::tauri::download_folder::FolderGrant>(id);
    }
    Ok(())
}
