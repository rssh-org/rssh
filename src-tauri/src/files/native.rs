//! Standard Tauri file backend: desktop paths, Android provider URIs and iOS scopes.

use super::{DownloadTarget, FileFilter, LocalWalkEntry};
use crate::error::{AppError, AppResult};
use serde_json::json;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};
use tauri_plugin_dialog::DialogExt;

pub(super) async fn file_name(app: &tauri::AppHandle, location: &str) -> AppResult<Option<String>> {
    #[cfg(android)]
    {
        use tauri::Manager;
        let app = app.clone();
        let source = location.to_owned();
        tokio::task::spawn_blocking(move || app.path().file_name(&source))
            .await
            .map_err(dialog_error)
    }
    #[cfg(not(android))]
    {
        let _ = app;
        Ok(filesystem_path(location.to_owned())?
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()))
    }
}

pub async fn pick_save(
    app: &tauri::AppHandle,
    default_name: String,
    filters: Vec<FileFilter>,
) -> AppResult<Option<String>> {
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

pub(super) async fn select_open(app: &tauri::AppHandle) -> AppResult<Option<String>> {
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

pub(super) async fn select_open_files(app: &tauri::AppHandle) -> AppResult<Option<Vec<String>>> {
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

#[cfg(any(windows, macos, linux))]
pub(super) async fn select_folder(
    app: &tauri::AppHandle,
    write: bool,
) -> AppResult<Option<String>> {
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

fn filesystem_path(location: String) -> AppResult<PathBuf> {
    location
        .parse::<tauri_plugin_fs::FilePath>()
        .expect("FilePath::from_str is infallible")
        .into_path()
        .map_err(|error| AppError::other("file_path_invalid", json!({"err": error.to_string()})))
}

pub(super) async fn resolve_paths(
    local_root: String,
    relative_paths: Vec<String>,
    write: bool,
) -> AppResult<Vec<super::ResolvedPath>> {
    let _ = write;
    let root = filesystem_path(local_root)?;
    Ok(relative_paths
        .into_iter()
        .map(|relative| {
            let result = super::validate_relative_path(&relative).map(|()| {
                relative
                    .split('/')
                    .fold(root.clone(), |path, part| path.join(part))
                    .to_string_lossy()
                    .into_owned()
            });
            super::ResolvedPath::new(relative, result)
        })
        .collect())
}

fn dialog_error(error: tokio::task::JoinError) -> AppError {
    AppError::other("dialog_task_failed", json!({"err": error.to_string()}))
}

/// The iOS authorization must outlive the file it grants access to.
struct FileAccessGuard {
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

struct FileResource<G> {
    file: tokio::fs::File,
    _access: G,
}

#[cfg(any(ios, test))]
impl<G> FileResource<G> {
    async fn close(self) {
        // Tokio workers can keep the descriptor alive after File::drop.
        // into_std waits for those workers, including after an I/O future
        // was cancelled, before we close the descriptor and release access.
        let Self { file, _access } = self;
        drop(file.into_std().await);
        drop(_access);
    }
}

/// Owns the stream and its authorization together for the entire operation.
pub struct OpenedFile {
    resource: Option<FileResource<FileAccessGuard>>,
}

impl OpenedFile {
    pub fn stream(
        &mut self,
    ) -> &mut (impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send) {
        &mut self.resource.as_mut().expect("file is open").file
    }

    pub async fn metadata(&self) -> std::io::Result<std::fs::Metadata> {
        self.resource
            .as_ref()
            .expect("file is open")
            .file
            .metadata()
            .await
    }
}

impl Drop for OpenedFile {
    fn drop(&mut self) {
        #[cfg(ios)]
        if let Some(resource) = self.resource.take() {
            // This task owns the grant even if the caller is cancelled. The
            // application runtime outlives individual transfer/window tasks.
            tauri::async_runtime::spawn(resource.close());
        }
    }
}

pub async fn download_target(
    app: &tauri::AppHandle,
    location: String,
) -> AppResult<DownloadTarget> {
    // Ordinary paths retain sibling .part + rename. Provider URIs retain the
    // selected-file authorization and provider open operation.
    if let tauri_plugin_fs::FilePath::Path(path) = location
        .parse::<tauri_plugin_fs::FilePath>()
        .expect("FilePath::from_str is infallible")
    {
        return Ok(DownloadTarget::AtomicPath(path));
    }
    let _ = app;
    Ok(DownloadTarget::Provider(location))
}

pub async fn open_file(app: &tauri::AppHandle, path: String, write: bool) -> AppResult<OpenedFile> {
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
    Ok(OpenedFile {
        resource: Some(FileResource {
            file: tokio::fs::File::from_std(file),
            _access: access,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct AccessProbe {
        released: std::sync::mpsc::Sender<Vec<u8>>,
        path: std::path::PathBuf,
    }

    impl Drop for AccessProbe {
        fn drop(&mut self) {
            let _ = self.released.send(std::fs::read(&self.path).unwrap());
        }
    }

    // Hold the only blocking worker so Tokio's queued file operation cannot
    // finish before the close future is polled. Channel ownership also releases
    // the worker if a test assertion fails; there are no timing-based sleeps.
    fn hold_file_worker(runtime: &tokio::runtime::Runtime) -> std::sync::mpsc::Sender<()> {
        let (started, ready) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        runtime.spawn_blocking(move || {
            started.send(()).unwrap();
            let _ = wait.recv();
        });
        ready.recv().unwrap();
        release
    }

    fn check_pending_file_close(read: bool) {
        use std::{
            future::Future,
            pin::pin,
            task::{Context, Waker},
        };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(source.path(), b"before").unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let release = hold_file_worker(&runtime);
        let (released, access) = std::sync::mpsc::channel();
        let mut resource = FileResource {
            file: tokio::fs::File::from_std(source.reopen().unwrap()),
            _access: AccessProbe {
                released,
                path: source.path().to_owned(),
            },
        };
        runtime.block_on(async {
            if read {
                let mut buf = [0u8; 6];
                let mut reading = pin!(resource.file.read(&mut buf));
                assert!(reading
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending());
                // Dropping a pending read future leaves the worker owning the FD.
            } else {
                resource.file.write_all(b"after!").await.unwrap();
                // A cancellation or remote read error now skips normal flush.
            }
        });
        let mut closing = pin!(resource.close());
        assert!(closing
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
        assert!(matches!(
            access.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        runtime.block_on(closing);
        assert_eq!(
            access.recv().unwrap(),
            if read { b"before" } else { b"after!" }
        );
    }

    #[test]
    fn authorization_outlives_a_queued_write_after_cancellation() {
        check_pending_file_close(false);
    }

    #[test]
    fn authorization_outlives_a_cancelled_read_future() {
        check_pending_file_close(true);
    }

    #[test]
    fn completed_file_releases_authorization_without_pending_io() {
        use std::{
            future::Future,
            pin::pin,
            task::{Context, Waker},
        };

        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(source.path(), b"complete").unwrap();
        let (released, access) = std::sync::mpsc::channel();
        let resource = FileResource {
            file: tokio::fs::File::from_std(source.reopen().unwrap()),
            _access: AccessProbe {
                released,
                path: source.path().to_owned(),
            },
        };
        let mut closing = pin!(resource.close());
        assert!(closing
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_ready());
        assert_eq!(access.recv().unwrap(), b"complete");
    }

    #[cfg(not(android))]
    #[test]
    fn selected_names_decode_file_urls_but_preserve_literal_path_percent_signs() {
        let root = std::env::temp_dir();
        for name in ["报告 #%.txt", "literal%20.txt"] {
            let path = root.join(name);
            let uri = tauri::Url::from_file_path(&path).unwrap().to_string();
            assert_eq!(filesystem_path(uri).unwrap().file_name().unwrap(), name);
            assert_eq!(
                filesystem_path(path.to_string_lossy().into_owned())
                    .unwrap()
                    .file_name()
                    .unwrap(),
                name
            );
        }
    }

    #[tokio::test]
    async fn desktop_resolution_preserves_root_and_nested_names() {
        let root = std::env::temp_dir().join("rssh path test");
        assert_eq!(
            filesystem_path(tauri::Url::from_file_path(&root).unwrap().to_string()).unwrap(),
            root
        );
        let resolved = resolve_paths(
            root.to_string_lossy().into_owned(),
            vec![
                "目录/a #%.txt".into(),
                "../invalid".into(),
                "second.txt".into(),
            ],
            true,
        )
        .await
        .unwrap();
        assert!(
            matches!(&resolved[0], super::super::ResolvedPath::Ready { relative_path, location }
            if relative_path == "目录/a #%.txt" && location == &root.join("目录/a #%.txt").to_string_lossy())
        );
        assert!(
            matches!(&resolved[1], super::super::ResolvedPath::Failed { relative_path, .. }
            if relative_path == "../invalid")
        );
        assert!(
            matches!(&resolved[2], super::super::ResolvedPath::Ready { relative_path, .. }
            if relative_path == "second.txt")
        );
    }
}

/// Maximum recursion depth, matching the remote and native walkers.
const LOCAL_WALK_DEPTH_CAP: u32 = 32;

/// Walk ordinary filesystem paths without changing the existing symlink policy.
pub async fn walk_directory(local_root: String) -> AppResult<Vec<LocalWalkEntry>> {
    let root = filesystem_path(local_root)?;
    let mut queue: VecDeque<(PathBuf, u32)> = VecDeque::new();
    queue.push_back((root.clone(), 0));
    let mut result: Vec<LocalWalkEntry> = Vec::new();

    while let Some((dir, depth)) = queue.pop_front() {
        if depth >= LOCAL_WALK_DEPTH_CAP {
            return Err(AppError::other(
                "local_tree_too_deep",
                json!({
                    "path": dir.display().to_string(),
                    "depth": depth,
                    "limit": LOCAL_WALK_DEPTH_CAP,
                }),
            ));
        }
        let mut rd = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = rd.next_entry().await? {
            // `entry.metadata()` does not traverse symlinks — single syscall
            // covers both type discrimination and size for regular files,
            // replacing the previous file_type() + metadata() double-stat.
            let path = entry.path();
            let meta = entry.metadata().await?;
            if meta.is_dir() {
                queue.push_back((path, depth + 1));
            } else if meta.is_file() {
                result.push(LocalWalkEntry {
                    local_path: path.to_string_lossy().into_owned(),
                    rel_path: rel_unix(&path, &root),
                    size: meta.len(),
                });
            } else if meta.is_symlink() {
                // Follow once to learn what the target is. Skip symlink-to-dir
                // to avoid cycles, and silently skip broken symlinks.
                if let Ok(target_meta) = tokio::fs::metadata(&path).await {
                    if target_meta.is_file() {
                        result.push(LocalWalkEntry {
                            local_path: path.to_string_lossy().into_owned(),
                            rel_path: rel_unix(&path, &root),
                            size: target_meta.len(),
                        });
                    }
                }
            }
            // Anything else (block/char/fifo): skip.
        }
    }
    Ok(result)
}

/// Convert the portion of `full` relative to `root` into a '/'-separated string.
/// The remote destination always uses '/', independently of the local platform.
fn rel_unix(full: &Path, root: &Path) -> String {
    let stripped = full.strip_prefix(root).unwrap_or(full);
    stripped
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod local_walk_tests {
    use super::*;

    #[tokio::test]
    async fn file_uri_walk_returns_openable_locations_and_preserves_symlink_policy() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("目录 #%");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("nested/file #%.txt"), b"content").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&source, source.join("cycle")).unwrap();
            std::os::unix::fs::symlink(
                source.join("nested/file #%.txt"),
                source.join("file-alias"),
            )
            .unwrap();
        }
        let entries = walk_directory(tauri::Url::from_file_path(&source).unwrap().to_string())
            .await
            .unwrap();
        assert_eq!(entries.len(), if cfg!(unix) { 2 } else { 1 });
        assert!(entries
            .iter()
            .any(|entry| entry.rel_path == "nested/file #%.txt"));
        for entry in entries {
            assert_eq!(entry.size, 7);
            assert_eq!(std::fs::read(&entry.local_path).unwrap(), b"content");
            assert_eq!(PathBuf::from(entry.local_path), source.join(entry.rel_path));
        }
    }
}
