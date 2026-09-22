//! File access contract shared by commands and native backends.
//! Locations stay opaque; the selected backend owns picker, authorization and I/O mechanics.

use crate::error::AppResult;

#[cfg(not(ohos))]
#[path = "native.rs"]
mod backend;
#[cfg(ohos)]
#[path = "ohos.rs"]
mod backend;

pub use backend::{download_target, open_file, pick_save, walk_directory, OpenedFile};

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

#[derive(serde::Deserialize)]
pub struct FileFilter {
    name: String,
    extensions: Vec<String>,
}

/// Backends preserve ordinary-path atomic downloads and provider-owned streams.
pub enum DownloadTarget {
    AtomicPath(std::path::PathBuf),
    Stream(OpenedFile),
}

async fn describe_location(app: &tauri::AppHandle, location: String) -> AppResult<PickedLocation> {
    let name = backend::file_name(app, &location).await?;
    Ok(PickedLocation {
        location,
        name: name.filter(|name| valid_display_name(name)),
    })
}

// Metadata is a single suggested remote name, never authority to select a path.
// Backslashes remain literal: the remote SFTP namespace uses POSIX separators.
fn valid_display_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
}

pub async fn pick_open(app: &tauri::AppHandle) -> AppResult<Option<PickedLocation>> {
    match backend::select_open(app).await? {
        Some(location) => describe_location(app, location).await.map(Some),
        None => Ok(None),
    }
}

pub async fn pick_open_files(app: &tauri::AppHandle) -> AppResult<Option<Vec<PickedLocation>>> {
    let Some(locations) = backend::select_open_files(app).await? else {
        return Ok(None);
    };
    let mut files = Vec::with_capacity(locations.len());
    for location in locations {
        files.push(describe_location(app, location).await?);
    }
    Ok(Some(files))
}

#[cfg(any(windows, macos, linux, ohos))]
pub async fn pick_folder(app: &tauri::AppHandle, write: bool) -> AppResult<Option<PickedLocation>> {
    match backend::select_folder(app, write).await? {
        Some(location) => describe_location(app, location).await.map(Some),
        None => Ok(None),
    }
}

pub async fn resolve_paths(
    local_root: String,
    relative_paths: Vec<String>,
    write: bool,
) -> AppResult<Vec<String>> {
    validate_relative_paths(&relative_paths)?;
    backend::resolve_paths(local_root, relative_paths, write).await
}

/// Remote names are relative names, never authority to escape a selected root.
fn validate_relative_paths(paths: &[String]) -> AppResult<()> {
    for path in paths {
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
    }
    Ok(())
}

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
                validate_relative_paths(&[invalid.into()]).is_err(),
                "accepted {invalid:?}"
            );
        }
        assert!(validate_relative_paths(&[
            "备份/报告 #%?.txt".into(),
            ".config/config.json".into()
        ])
        .is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn preserves_posix_file_names_that_are_not_path_separators() {
        assert!(
            validate_relative_paths(&["dir/a\\b.txt".into(), "dir/line\nbreak".into()]).is_ok()
        );
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_path_components_in_remote_file_names() {
        for invalid in ["dir/a\\b.txt", "dir/C:relative", "dir/\\root"] {
            assert!(validate_relative_paths(&[invalid.into()]).is_err());
        }
    }
}
