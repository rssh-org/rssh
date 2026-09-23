//! HarmonyOS adapter. Only this backend knows native picker/filter and URI mechanics.

use super::{DownloadTarget, FileFilter};
use crate::error::AppResult;

pub(super) use crate::ohos::files::resolve_paths;
pub use crate::ohos::files::walk_directory;

pub(super) async fn file_name(
    _app: &tauri::AppHandle,
    location: &str,
) -> AppResult<Option<String>> {
    crate::ohos::files::file_name(location.to_owned())
        .await
        .map(Some)
}

pub(super) async fn select_open(_app: &tauri::AppHandle) -> AppResult<Option<String>> {
    crate::ohos::files::pick_open().await
}

pub(super) async fn select_open_files(_app: &tauri::AppHandle) -> AppResult<Option<Vec<String>>> {
    crate::ohos::files::pick_open_files().await
}

pub(super) async fn select_folder(
    _app: &tauri::AppHandle,
    write: bool,
) -> AppResult<Option<String>> {
    crate::ohos::files::pick_folder(write).await
}

pub async fn pick_save(
    _app: &tauri::AppHandle,
    default_name: String,
    filters: Vec<FileFilter>,
) -> AppResult<Option<String>> {
    let filters = filters
        .into_iter()
        .map(|filter| {
            openharmony_ability_plugin_files::FileDialogFilter::new()
                .name(filter.name)
                .pattern(filter.extensions.join(";"))
        })
        .collect();
    crate::ohos::files::pick_save(default_name, filters).await
}

/// The duplicated descriptor owns access after the native provider file closes.
pub struct OpenedFile {
    file: tokio::fs::File,
}

impl OpenedFile {
    pub fn stream(
        &mut self,
    ) -> &mut (impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send) {
        &mut self.file
    }

    pub async fn metadata(&self) -> std::io::Result<std::fs::Metadata> {
        self.file.metadata().await
    }
}

pub async fn open_file(
    _app: &tauri::AppHandle,
    path: String,
    write: bool,
) -> AppResult<OpenedFile> {
    Ok(OpenedFile {
        file: tokio::fs::File::from_std(crate::ohos::files::open_file(path, write).await?),
    })
}

pub async fn download_target(
    _app: &tauri::AppHandle,
    location: String,
) -> AppResult<DownloadTarget> {
    match crate::ohos::files::download_path(location.clone()).await? {
        Some(path) => Ok(DownloadTarget::AtomicPath(path)),
        None => Ok(DownloadTarget::Provider(location)),
    }
}
