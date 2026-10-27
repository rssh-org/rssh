//! Native capabilities supplied by the application entry point.
//!
//! The host owns platform SDK objects. Commands use this interface through
//! Tauri's application state, so each application has one explicit adapter.

use std::sync::Arc;
use tauri::{AppHandle, Manager};

use super::runtime::RuntimeCapabilities;
use crate::error::AppResult;
use crate::files::{DownloadTarget, FileFilter, LocalWalkEntry, OpenedFile, ResolvedPath};

#[derive(Clone, Copy)]
pub enum AppWindowPurpose {
    Tab,
    LocalAnalysis,
}

#[async_trait::async_trait]
pub trait NativePlatform: Send + Sync {
    async fn capabilities(&self, window_label: &str) -> AppResult<RuntimeCapabilities>;
    async fn read_clipboard(&self) -> AppResult<String>;
    async fn write_clipboard(&self, text: String) -> AppResult<()>;
    async fn open_url(&self, app: &AppHandle, url: String) -> AppResult<()>;

    fn window_title(&self, requested: &str) -> String;
    async fn ensure_window(&self, purpose: AppWindowPurpose) -> Result<(), String>;
    async fn confirm_window(&self, window: tauri::WebviewWindow) -> Result<(), String>;

    async fn file_name(&self, app: &AppHandle, location: &str) -> AppResult<Option<String>>;
    async fn pick_open(&self, app: &AppHandle) -> AppResult<Option<String>>;
    async fn pick_open_files(&self, app: &AppHandle) -> AppResult<Option<Vec<String>>>;
    async fn pick_folder(&self, app: &AppHandle, write: bool) -> AppResult<Option<String>>;
    async fn pick_save(
        &self,
        app: &AppHandle,
        default_name: String,
        filters: Vec<FileFilter>,
    ) -> AppResult<Option<String>>;
    async fn open_file(
        &self,
        app: &AppHandle,
        location: String,
        write: bool,
    ) -> AppResult<OpenedFile>;
    async fn download_target(&self, app: &AppHandle, location: String)
        -> AppResult<DownloadTarget>;
    async fn resolve_paths(
        &self,
        root: String,
        paths: Vec<String>,
        write: bool,
    ) -> AppResult<Vec<ResolvedPath>>;
    async fn walk_directory(&self, root: String) -> AppResult<Vec<LocalWalkEntry>>;
}

pub(crate) fn platform(app: &AppHandle) -> Arc<dyn NativePlatform> {
    app.state::<Arc<dyn NativePlatform>>().inner().clone()
}
