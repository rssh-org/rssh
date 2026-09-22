//! Native GUI file command entry points. Platform mechanics live in crate::files.

use crate::error::AppResult;
use crate::files::{self, FileFilter};

#[tauri::command]
pub async fn resolve_local_paths(
    local_root: String,
    relative_paths: Vec<String>,
    write: bool,
) -> AppResult<Vec<String>> {
    files::resolve_paths(local_root, relative_paths, write).await
}

#[tauri::command]
// Browser exports use Blob downloads in save-file.ts; JCEF explicitly rejects
// them. This command belongs to native GUI hosts, never the headless server.
pub async fn save_text_file(
    app: tauri::AppHandle,
    default_name: String,
    contents: String,
    filters: Vec<FileFilter>,
) -> AppResult<Option<String>> {
    files::save_text(&app, default_name, contents, filters).await
}
