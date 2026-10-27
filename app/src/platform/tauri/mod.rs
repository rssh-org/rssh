//! Standard Tauri native services for desktop, Android and iOS.

#[cfg(any(windows, macos, linux))]
mod clipboard;
mod external;

use crate::error::{AppError, AppResult};
use crate::files::{self, DownloadTarget, FileFilter, LocalWalkEntry, OpenedFile, ResolvedPath};
use crate::platform::host::{AppWindowPurpose, NativePlatform};
use crate::platform::runtime::RuntimeCapabilities;
use tauri::AppHandle;

pub(crate) struct TauriPlatform;

#[async_trait::async_trait]
impl NativePlatform for TauriPlatform {
    async fn capabilities(&self, _window_label: &str) -> AppResult<RuntimeCapabilities> {
        Ok(RuntimeCapabilities::native())
    }

    async fn read_clipboard(&self) -> AppResult<String> {
        #[cfg(any(windows, macos, linux))]
        {
            tokio::task::spawn_blocking(clipboard::read_text)
                .await
                .map_err(clipboard_dispatch_error)?
        }
        #[cfg(any(android, ios))]
        {
            Err(clipboard_unavailable("read"))
        }
    }

    async fn write_clipboard(&self, text: String) -> AppResult<()> {
        #[cfg(any(windows, macos, linux))]
        {
            tokio::task::spawn_blocking(move || clipboard::write_text(text))
                .await
                .map_err(clipboard_dispatch_error)?
        }
        #[cfg(any(android, ios))]
        {
            let _ = text;
            Err(clipboard_unavailable("write"))
        }
    }

    async fn open_url(&self, app: &AppHandle, url: String) -> AppResult<()> {
        external::open_url(app, url).await
    }

    fn window_title(&self, requested: &str) -> String {
        requested.to_owned()
    }

    async fn ensure_window(&self, _purpose: AppWindowPurpose) -> Result<(), String> {
        #[cfg(any(windows, macos, linux))]
        {
            Ok(())
        }
        #[cfg(any(android, ios))]
        {
            Err("Additional windows are unavailable on this device".into())
        }
    }

    async fn confirm_window(&self, _window: tauri::WebviewWindow) -> Result<(), String> {
        // Standard backends complete native creation before build() returns.
        Ok(())
    }

    async fn file_name(&self, app: &AppHandle, location: &str) -> AppResult<Option<String>> {
        files::native::file_name(app, location).await
    }

    async fn pick_open(&self, app: &AppHandle) -> AppResult<Option<String>> {
        files::native::select_open(app).await
    }

    async fn pick_open_files(&self, app: &AppHandle) -> AppResult<Option<Vec<String>>> {
        files::native::select_open_files(app).await
    }

    async fn pick_folder(&self, app: &AppHandle, write: bool) -> AppResult<Option<String>> {
        #[cfg(any(windows, macos, linux))]
        {
            files::native::select_folder(app, write).await
        }
        #[cfg(any(android, ios))]
        {
            let _ = (app, write);
            Err(AppError::other(
                "file_path_invalid",
                serde_json::json!({"err":"Folder selection is unavailable on this host"}),
            ))
        }
    }

    async fn pick_save(
        &self,
        app: &AppHandle,
        name: String,
        filters: Vec<FileFilter>,
    ) -> AppResult<Option<String>> {
        files::native::pick_save(app, name, filters).await
    }

    async fn open_file(
        &self,
        app: &AppHandle,
        location: String,
        write: bool,
    ) -> AppResult<OpenedFile> {
        files::native::open_file(app, location, write)
            .await
            .map(OpenedFile::from_native)
    }

    async fn download_target(
        &self,
        app: &AppHandle,
        location: String,
    ) -> AppResult<DownloadTarget> {
        files::native::download_target(app, location).await
    }

    async fn resolve_paths(
        &self,
        root: String,
        paths: Vec<String>,
        write: bool,
    ) -> AppResult<Vec<ResolvedPath>> {
        files::native::resolve_paths(root, paths, write).await
    }

    async fn walk_directory(&self, root: String) -> AppResult<Vec<LocalWalkEntry>> {
        files::native::walk_directory(root).await
    }
}

#[cfg(any(windows, macos, linux))]
fn clipboard_dispatch_error(error: tokio::task::JoinError) -> AppError {
    AppError::other(
        "window_clipboard_failed",
        serde_json::json!({"op":"dispatch", "err":error.to_string()}),
    )
}

#[cfg(any(android, ios))]
fn clipboard_unavailable(op: &str) -> AppError {
    AppError::other(
        "window_clipboard_failed",
        serde_json::json!({"op":op, "err":"Clipboard access is provided by the WebView"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_title_preserves_window_content() {
        let platform: &dyn NativePlatform = &TauriPlatform;
        for requested in [
            "首页",
            "Settings",
            "user@host:~",
            "RSSH — Local Analysis",
            "",
        ] {
            assert_eq!(platform.window_title(requested), requested);
        }
    }
}
