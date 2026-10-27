//! File access contract shared by commands and native backends.
//! Locations stay opaque; the selected backend owns picker, authorization and I/O mechanics.

use crate::error::AppResult;

#[cfg(not(ohos))]
pub(crate) mod native;
#[cfg(feature = "tauri")]
use crate::platform::host::platform;

// Browser/headless file operations use filesystem paths directly.
#[cfg(not(ohos))]
pub use native::walk_directory;

#[cfg(feature = "tauri")]
pub struct OpenedFile {
    backend: FileBackend,
}

#[cfg(feature = "tauri")]
enum FileBackend {
    #[cfg(not(ohos))]
    Native(native::OpenedFile),
    Provider(tokio::fs::File),
}

#[cfg(feature = "tauri")]
impl OpenedFile {
    /// The host has transferred an owned descriptor; closing it releases access.
    pub fn from_std(file: std::fs::File) -> Self {
        Self {
            backend: FileBackend::Provider(tokio::fs::File::from_std(file)),
        }
    }

    #[cfg(not(ohos))]
    pub(crate) fn from_native(file: native::OpenedFile) -> Self {
        Self {
            backend: FileBackend::Native(file),
        }
    }

    pub fn stream(&mut self) -> &mut tokio::fs::File {
        match &mut self.backend {
            #[cfg(not(ohos))]
            FileBackend::Native(file) => file.stream(),
            FileBackend::Provider(file) => file,
        }
    }

    pub async fn metadata(&self) -> std::io::Result<std::fs::Metadata> {
        match &self.backend {
            #[cfg(not(ohos))]
            FileBackend::Native(file) => file.metadata().await,
            FileBackend::Provider(file) => file.metadata().await,
        }
    }
}

#[cfg(feature = "tauri")]
pub async fn pick_save(
    app: &tauri::AppHandle,
    default_name: String,
    filters: Vec<FileFilter>,
) -> AppResult<Option<String>> {
    platform(app).pick_save(app, default_name, filters).await
}

#[cfg(feature = "tauri")]
pub async fn open_file(
    app: &tauri::AppHandle,
    location: String,
    write: bool,
) -> AppResult<OpenedFile> {
    platform(app).open_file(app, location, write).await
}

#[cfg(feature = "tauri")]
pub async fn download_target(
    app: &tauri::AppHandle,
    location: String,
) -> AppResult<DownloadTarget> {
    platform(app).download_target(app, location).await
}

#[cfg(feature = "tauri")]
#[derive(serde::Serialize)]
pub struct PickedLocation {
    pub location: String,
    pub name: Option<String>,
}

/// A source location and its relative destination name are separate values.
#[derive(serde::Serialize)]
pub struct LocalWalkEntry {
    pub rel_path: String,
    pub size: u64,
    pub local_path: String,
}

#[cfg(feature = "tauri")]
#[derive(serde::Deserialize)]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

/// Backends classify locations without opening or truncating the destination.
/// Directory authorization permits a sibling temporary file and atomic replacement;
/// a selected provider document grants access only to that document.
#[cfg(feature = "tauri")]
pub enum DownloadTarget {
    AtomicPath(std::path::PathBuf),
    Provider(String),
}

#[cfg(feature = "tauri")]
async fn describe_location(app: &tauri::AppHandle, location: String) -> AppResult<PickedLocation> {
    let name = platform(app).file_name(app, &location).await?;
    Ok(PickedLocation {
        location,
        name: name.filter(|name| valid_display_name(name)),
    })
}

// Metadata is a single suggested remote name, never authority to select a path.
// Backslashes remain literal: the remote SFTP namespace uses POSIX separators.
#[cfg(any(feature = "tauri", test))]
fn valid_display_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
}

#[cfg(feature = "tauri")]
pub async fn pick_open(app: &tauri::AppHandle) -> AppResult<Option<PickedLocation>> {
    match platform(app).pick_open(app).await? {
        Some(location) => describe_location(app, location).await.map(Some),
        None => Ok(None),
    }
}

#[cfg(feature = "tauri")]
pub async fn pick_open_files(app: &tauri::AppHandle) -> AppResult<Option<Vec<PickedLocation>>> {
    let Some(locations) = platform(app).pick_open_files(app).await? else {
        return Ok(None);
    };
    let mut files = Vec::with_capacity(locations.len());
    for location in locations {
        files.push(describe_location(app, location).await?);
    }
    Ok(Some(files))
}

#[cfg(any(windows, macos, linux, ohos))]
#[cfg(feature = "tauri")]
pub async fn pick_folder(app: &tauri::AppHandle, write: bool) -> AppResult<Option<PickedLocation>> {
    match platform(app).pick_folder(app, write).await? {
        Some(location) => describe_location(app, location).await.map(Some),
        None => Ok(None),
    }
}

/// Every selected name has an independent outcome; one bad target does not
/// invalidate unrelated downloads in the same selection.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResolvedPath {
    Ready {
        relative_path: String,
        location: String,
    },
    Failed {
        relative_path: String,
        error: String,
    },
}

impl ResolvedPath {
    pub fn new(relative_path: String, result: AppResult<String>) -> Self {
        match result {
            Ok(location) => Self::Ready {
                relative_path,
                location,
            },
            Err(error) => Self::Failed {
                relative_path,
                error: error.to_string(),
            },
        }
    }
}

#[cfg(not(ohos))]
pub async fn resolve_paths(
    local_root: String,
    relative_paths: Vec<String>,
    write: bool,
) -> AppResult<Vec<ResolvedPath>> {
    native::resolve_paths(local_root, relative_paths, write).await
}

/// Remote names are relative names, never authority to escape a selected root.
#[cfg(any(not(ohos), test))]
fn validate_relative_path(path: &str) -> AppResult<()> {
    let valid = !path.is_empty()
        && path.split('/').all(|part| {
            let mut components = std::path::Path::new(part).components();
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.contains('\0')
                && matches!(components.next(), Some(std::path::Component::Normal(_)))
                && components.next().is_none()
        });
    if !valid {
        return Err(crate::error::AppError::other(
            "file_path_invalid",
            serde_json::json!({"err": "Expected a relative file name below the selected directory"}),
        ));
    }
    Ok(())
}

// Owning the writer also owns its platform access grant. In particular iOS
// must keep the grant until Tokio's outstanding file work has finished.
#[cfg(feature = "tauri")]
impl tokio::io::AsyncWrite for OpenedFile {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(self.stream()).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(self.stream()).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(self.stream()).poll_shutdown(cx)
    }
}

#[cfg(feature = "tauri")]
pub async fn save_text(
    app: &tauri::AppHandle,
    default_name: String,
    contents: String,
    filters: Vec<FileFilter>,
) -> AppResult<Option<String>> {
    use tokio::io::AsyncWriteExt;
    let Some(path) = pick_save(app, default_name, filters).await? else {
        return Ok(None);
    };
    let mut file = open_file(app, path.clone(), true).await?;
    file.stream().write_all(contents.as_bytes()).await?;
    file.stream().flush().await?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "tauri")]
    #[tokio::test]
    async fn provider_descriptor_uses_the_shared_transfer_stream() {
        use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

        let source = tempfile::NamedTempFile::new().unwrap();
        let mut file = OpenedFile::from_std(source.reopen().unwrap());
        // Downloads write through OpenedFile; uploads read its same owned stream.
        file.write_all(b"provider content").await.unwrap();
        file.flush().await.unwrap();
        assert_eq!(file.metadata().await.unwrap().len(), 16);
        file.stream()
            .seek(std::io::SeekFrom::Start(0))
            .await
            .unwrap();
        let mut content = Vec::new();
        file.stream().read_to_end(&mut content).await.unwrap();
        assert_eq!(content, b"provider content");
        drop(file);
        assert_eq!(std::fs::read(source.path()).unwrap(), content);
    }

    #[test]
    fn provider_names_are_single_names_not_remote_paths() {
        for name in ["", ".", "..", "../key", "a/b", "a\0b"] {
            assert!(!valid_display_name(name), "accepted {name:?}");
        }
        for name in ["报告: final %.txt", ".env", "a\\b.txt"] {
            assert!(valid_display_name(name), "rejected {name:?}");
        }
    }

    #[test]
    fn directory_targets_reject_traversal_and_keep_literal_uri_characters() {
        for invalid in [
            "",
            "/root",
            "../secret",
            "x/../secret",
            "./x",
            "x//y",
            "x/",
            "x\0y",
        ] {
            assert!(
                validate_relative_path(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        for valid in ["备份/报告 #%?.txt", ".config/config.json"] {
            assert!(validate_relative_path(valid).is_ok());
        }
    }

    #[cfg(unix)]
    #[test]
    fn preserves_posix_file_names_that_are_not_path_separators() {
        for valid in ["dir/a\\b.txt", "dir/line\nbreak"] {
            assert!(validate_relative_path(valid).is_ok());
        }
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_path_components_in_remote_file_names() {
        for invalid in ["dir/a\\b.txt", "dir/C:relative", "dir/\\root"] {
            assert!(validate_relative_path(invalid).is_err());
        }
    }
}
