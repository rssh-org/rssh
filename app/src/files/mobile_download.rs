//! A single-document grant cannot create a sibling. Download into the RSSH
//! file area, then let the user export the completed file with the system UI.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Manager, WebviewWindow};

use super::download_library::{self, invalid_path, SavedDownload};
use crate::error::AppResult;
use crate::state::AppState;

fn directory(app: &AppHandle) -> PathBuf {
    app.state::<Arc<AppState>>().data_dir.join("sftp-downloads")
}

pub(super) fn owned_destination(app: &AppHandle, path: &Path) -> AppResult<Option<PathBuf>> {
    let Some(path) = download_library::child(&directory(app), path) else {
        return Ok(None);
    };
    if !path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(download_library::destination_name)
    {
        return Err(invalid_path());
    }
    Ok(Some(path))
}

pub(crate) async fn list(app: &AppHandle) -> AppResult<Vec<SavedDownload>> {
    let root = directory(app);
    tokio::task::spawn_blocking(move || download_library::list(&root))
        .await
        .map_err(std::io::Error::other)?
}

pub(crate) async fn destination(
    app: &AppHandle,
    _window: &WebviewWindow,
    name: String,
) -> AppResult<Option<String>> {
    if !super::valid_display_name(&name) {
        return Err(invalid_path());
    }
    #[cfg(ohos)]
    {
        let platform = crate::platform::host::platform(app);
        if !platform
            .capabilities(_window.label())
            .await?
            .sftp_download_export
        {
            // An authorized directory grants a real destination and siblings.
            let Some(folder) = platform.pick_folder(app, true).await? else {
                return Ok(None);
            };
            let paths = platform.resolve_paths(folder, vec![name], true).await?;
            return match paths.into_iter().next() {
                Some(super::ResolvedPath::Ready { location, .. }) => Ok(Some(location)),
                Some(super::ResolvedPath::Failed { error, .. }) => Err(error),
                None => Err(invalid_path()),
            };
        }
    }
    if !download_library::destination_name(&name) {
        return Err(invalid_path());
    }
    let root = directory(app);
    std::fs::create_dir_all(&root)?;
    if !std::fs::symlink_metadata(&root)?.is_dir() {
        return Err(invalid_path());
    }
    Ok(Some(root.join(name).to_string_lossy().into_owned()))
}

pub(crate) async fn export(
    app: &AppHandle,
    window: WebviewWindow,
    local_path: String,
    identity: String,
) -> AppResult<()> {
    let path = PathBuf::from(local_path);
    let source = download_library::open_verified(&directory(app), &path, &identity)?;
    #[cfg(ios)]
    {
        let snapshot = tokio::task::spawn_blocking(move || {
            download_library::PickerFile::create(&path, source, &std::env::temp_dir())
        })
        .await
        .map_err(std::io::Error::other)??;
        return export_ios(window, snapshot).await;
    }
    #[cfg(any(android, ohos))]
    {
        use tokio::io::AsyncWriteExt;
        let _ = window;
        // These save pickers grant a new document for an explicit export.
        // The queue's completed copy remains available even if export fails.
        let mut source = tokio::fs::File::from_std(source);
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(invalid_path)?;
        let Some(uri) = super::pick_save(app, name.to_owned(), Vec::new()).await? else {
            return Ok(());
        };
        let mut output = super::open_file(app, uri, true).await?;
        tokio::io::copy(&mut source, &mut output).await?;
        output.shutdown().await?;
        Ok(())
    }
}

#[cfg(ios)]
async fn export_ios(window: WebviewWindow, file: download_library::PickerFile) -> AppResult<()> {
    let (done, result) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |webview| {
            // Tauri owns this controller for the duration of its main-thread
            // callback. UIKit retains the picker after it is presented.
            let presenter = unsafe {
                webview
                    .view_controller()
                    .cast::<objc2_ui_kit::UIViewController>()
                    .as_ref()
            };
            let show = presenter
                .ok_or_else(|| std::io::Error::other("iOS window closed"))
                .and_then(|presenter| super::ios_export::present(presenter, file));
            let _ = done.send(show);
        })
        .map_err(std::io::Error::other)?;
    result.await.map_err(std::io::Error::other)??;
    Ok(())
}
