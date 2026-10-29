//! HarmonyOS owns SDK-specific mechanics; the shared application owns commands.

use crate::error::AppResult;
use rssh_host::files::{DownloadTarget, FileFilter, LocalWalkEntry, OpenedFile, ResolvedPath};
use rssh_host::platform::host::{AppWindowPurpose, NativePlatform};
use rssh_host::platform::runtime::RuntimeCapabilities;
use tauri::AppHandle;

pub struct OhosPlatform;

#[async_trait::async_trait]
impl NativePlatform for OhosPlatform {
    async fn capabilities(&self, window_label: &str) -> AppResult<RuntimeCapabilities> {
        crate::device::capabilities(window_label).await
    }

    async fn set_back_navigation_enabled(
        &self,
        app: &AppHandle,
        window_label: &str,
        enabled: bool,
        revision: u64,
    ) -> AppResult<()> {
        crate::navigation::set_enabled(app, window_label, enabled, revision);
        Ok(())
    }

    async fn read_clipboard(&self) -> AppResult<String> {
        crate::clipboard::read_text().await
    }

    async fn write_clipboard(&self, text: String) -> AppResult<()> {
        crate::clipboard::write_text(text).await
    }

    async fn open_url(&self, app: &AppHandle, url: String) -> AppResult<()> {
        crate::external::open_url(app, url).await
    }

    fn window_title(&self, requested: &str) -> String {
        crate::window::native_title(requested).to_owned()
    }

    async fn ensure_window(&self, purpose: AppWindowPurpose) -> Result<(), String> {
        crate::window::ensure_available(purpose).await
    }

    async fn confirm_window(&self, window: tauri::WebviewWindow) -> Result<(), String> {
        crate::window::confirm_creation(window).await
    }

    async fn file_name(&self, _app: &AppHandle, location: &str) -> AppResult<Option<String>> {
        crate::files::file_name(location.to_owned()).await.map(Some)
    }

    async fn pick_open(&self, _app: &AppHandle) -> AppResult<Option<String>> {
        crate::files::pick_open().await
    }

    async fn pick_open_files(&self, _app: &AppHandle) -> AppResult<Option<Vec<String>>> {
        crate::files::pick_open_files().await
    }

    async fn pick_folder(&self, _app: &AppHandle, write: bool) -> AppResult<Option<String>> {
        crate::files::pick_folder(write).await
    }

    async fn pick_save(
        &self,
        _app: &AppHandle,
        name: String,
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
        crate::files::pick_save(name, filters).await
    }

    async fn open_file(
        &self,
        _app: &AppHandle,
        location: String,
        write: bool,
    ) -> AppResult<OpenedFile> {
        crate::files::open_file(location, write)
            .await
            .map(OpenedFile::from_std)
    }

    async fn download_target(
        &self,
        _app: &AppHandle,
        location: String,
    ) -> AppResult<DownloadTarget> {
        match crate::files::download_path(location.clone()).await? {
            Some(path) => Ok(DownloadTarget::AtomicPath(path)),
            None => Ok(DownloadTarget::Provider(location)),
        }
    }

    async fn resolve_paths(
        &self,
        root: String,
        paths: Vec<String>,
        write: bool,
    ) -> AppResult<Vec<ResolvedPath>> {
        crate::files::resolve_paths(root, paths, write).await
    }

    async fn walk_directory(&self, root: String) -> AppResult<Vec<LocalWalkEntry>> {
        crate::files::walk_directory(root).await
    }
}
