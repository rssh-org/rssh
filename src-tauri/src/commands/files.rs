//! Native file capabilities. Callers keep URIs opaque and own opened files
//! for the entire operation; the platform owns picker and access mechanics.

use crate::error::AppResult;

/// A local walk result carries its authorized location separately from the
/// relative name used to construct a remote path. URIs never become paths in UI.
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

pub async fn pick_save(
    app: &tauri::AppHandle,
    default_name: String,
    filters: Vec<FileFilter>,
) -> AppResult<Option<String>> {
    #[cfg(ohos)]
    {
        let _ = app;
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
    #[cfg(not(ohos))]
    {
        use tauri_plugin_dialog::DialogExt;
        let app = app.clone();
        tokio::task::spawn_blocking(move || {
            let mut dialog = app.dialog().file().set_file_name(default_name);
            for filter in filters {
                let extensions: Vec<_> = filter.extensions.iter().map(String::as_str).collect();
                dialog = dialog.add_filter(filter.name, &extensions);
            }
            dialog.blocking_save_file().map(|path| path.to_string())
        })
        .await
        .map_err(dialog_error)
    }
}

pub async fn pick_open(app: &tauri::AppHandle) -> AppResult<Option<String>> {
    #[cfg(ohos)]
    {
        let _ = app;
        crate::ohos::files::pick_open().await
    }
    #[cfg(not(ohos))]
    {
        use tauri_plugin_dialog::DialogExt;
        let app = app.clone();
        tokio::task::spawn_blocking(move || {
            app.dialog()
                .file()
                .blocking_pick_file()
                .map(|path| path.to_string())
        })
        .await
        .map_err(dialog_error)
    }
}

pub async fn pick_open_files(app: &tauri::AppHandle) -> AppResult<Option<Vec<String>>> {
    #[cfg(ohos)]
    {
        let _ = app;
        crate::ohos::files::pick_open_files().await
    }
    #[cfg(not(ohos))]
    {
        use tauri_plugin_dialog::DialogExt;
        let app = app.clone();
        tokio::task::spawn_blocking(move || {
            app.dialog()
                .file()
                .blocking_pick_files()
                .map(|files| files.into_iter().map(|file| file.to_string()).collect())
        })
        .await
        .map_err(dialog_error)
    }
}

#[cfg(any(windows, macos, linux, ohos))]
pub async fn pick_folder(app: &tauri::AppHandle, write: bool) -> AppResult<Option<String>> {
    #[cfg(ohos)]
    {
        let _ = app;
        crate::ohos::files::pick_folder(write).await
    }
    #[cfg(not(ohos))]
    {
        use tauri_plugin_dialog::DialogExt;
        let _ = write;
        let app = app.clone();
        tokio::task::spawn_blocking(move || {
            app.dialog()
                .file()
                .blocking_pick_folder()
                .map(|file| file.to_string())
        })
        .await
        .map_err(dialog_error)
    }
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

#[cfg(not(ohos))]
pub(super) fn filesystem_path(location: String) -> AppResult<std::path::PathBuf> {
    location
        .parse::<tauri_plugin_fs::FilePath>()
        .expect("FilePath::from_str is infallible")
        .into_path()
        .map_err(|error| {
            crate::error::AppError::other(
                "file_path_invalid",
                serde_json::json!({"err": error.to_string()}),
            )
        })
}

#[tauri::command]
pub async fn resolve_local_paths(
    local_root: String,
    relative_paths: Vec<String>,
    write: bool,
) -> AppResult<Vec<String>> {
    validate_relative_paths(&relative_paths)?;
    #[cfg(ohos)]
    {
        crate::ohos::files::resolve_paths(local_root, relative_paths, write).await
    }
    #[cfg(not(ohos))]
    {
        let _ = write;
        let root = filesystem_path(local_root)?;
        Ok(relative_paths
            .into_iter()
            .map(|relative| {
                relative
                    .split('/')
                    .fold(root.clone(), |path, part| path.join(part))
                    .to_string_lossy()
                    .into_owned()
            })
            .collect())
    }
}

#[cfg(not(ohos))]
fn dialog_error(error: tokio::task::JoinError) -> crate::error::AppError {
    crate::error::AppError::other(
        "dialog_task_failed",
        serde_json::json!({"err": error.to_string()}),
    )
}

/// The iOS authorization must outlive the file it grants access to.
pub struct FileAccessGuard {
    #[cfg(ios)]
    app: tauri::AppHandle,
    #[cfg(ios)]
    path: Option<tauri_plugin_fs::FilePath>,
}

impl Drop for FileAccessGuard {
    fn drop(&mut self) {
        #[cfg(ios)]
        if let Some(path) = self.path.take() {
            use tauri_plugin_fs::FsExt;
            if let Err(error) = self.app.fs().stop_accessing_security_scoped_resource(path) {
                log::warn!("failed to release iOS security-scoped file: {error}");
            }
        }
    }
}

pub async fn open_file(
    app: &tauri::AppHandle,
    path: String,
    write: bool,
) -> AppResult<(std::fs::File, FileAccessGuard)> {
    #[cfg(ohos)]
    {
        let _ = app;
        Ok((
            crate::ohos::files::open_file(path, write).await?,
            FileAccessGuard {},
        ))
    }
    #[cfg(not(ohos))]
    {
        use tauri_plugin_fs::{FilePath, FsExt, OpenOptions};
        let path = path
            .parse::<FilePath>()
            .expect("FilePath::from_str is infallible");
        let access = FileAccessGuard {
            #[cfg(ios)]
            app: app.clone(),
            #[cfg(ios)]
            path: match &path {
                FilePath::Url(url) if url.scheme() == "file" => Some(path.clone()),
                _ => None,
            },
        };
        let mut options = OpenOptions::new();
        options
            .read(!write)
            .write(write)
            .create(write)
            .truncate(write);
        let file = app.fs().open(path, options)?;
        Ok((file, access))
    }
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
    use tokio::io::AsyncWriteExt;
    let Some(path) = pick_save(&app, default_name, filters).await? else {
        return Ok(None);
    };
    let (file, _access) = open_file(&app, path.clone(), true).await?;
    let mut file = tokio::fs::File::from_std(file);
    file.write_all(contents.as_bytes()).await?;
    file.flush().await?;
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[cfg(not(ohos))]
    #[tokio::test]
    async fn desktop_resolution_preserves_root_and_nested_names() {
        let root = std::env::temp_dir().join("rssh path test");
        assert_eq!(
            filesystem_path(tauri::Url::from_file_path(&root).unwrap().to_string()).unwrap(),
            root
        );
        let resolved = resolve_local_paths(
            root.to_string_lossy().into_owned(),
            vec!["目录/a #%.txt".into()],
            true,
        )
        .await
        .unwrap();
        assert_eq!(
            resolved,
            vec![root
                .join("目录")
                .join("a #%.txt")
                .to_string_lossy()
                .into_owned()]
        );
    }
}
