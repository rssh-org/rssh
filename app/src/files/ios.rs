//! iOS document folders retain their original security-scoped NSURL. Every
//! external access is coordinated, including the full lifetime of stream I/O.

// The macOS test build exercises Foundation without the iOS app entry points.
#![cfg_attr(all(test, macos), allow(dead_code))]

use crate::error::{AppError, AppResult};
use crate::files::{LocalWalkEntry, ResolvedPath};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2_foundation::{
    NSFileCoordinator, NSFileCoordinatorReadingOptions, NSFileCoordinatorWritingOptions, NSString,
    NSURL,
};
use rssh_core::files::transfer::{
    FileIdentity, NativeEndpoint, RenameError, TransferEndpoint, TransferMode, TransferStream,
};
use serde_json::json;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite, AsyncWriteExt, ReadBuf};

#[cfg(ios)]
#[path = "ios_picker.rs"]
mod picker;

#[derive(Clone)]
struct Grant {
    url: Retained<NSURL>,
    root: PathBuf,
    directory: bool,
    write: bool,
}

#[derive(Default)]
struct Grants(Mutex<Vec<Grant>>);

struct Scope {
    url: Retained<NSURL>,
    active: bool,
}

fn folder_required() -> AppError {
    AppError::sftp("sftp_folder_required", json!({}))
}
fn provider_error(error: impl std::fmt::Display) -> AppError {
    AppError::sftp(
        "sftp_provider_unsupported",
        json!({ "err": error.to_string() }),
    )
}
fn join_error(error: tokio::task::JoinError) -> AppError {
    provider_error(error)
}
fn file_path(location: &str) -> AppResult<PathBuf> {
    match url::Url::parse(location) {
        Ok(url) => url.to_file_path().map_err(|_| folder_required()),
        Err(_) => Ok(PathBuf::from(location)),
    }
}
fn file_url(path: &Path) -> String {
    url::Url::from_file_path(path)
        .expect("absolute native path")
        .into()
}

impl Scope {
    fn start(url: &Retained<NSURL>) -> AppResult<Self> {
        // NSURL is immutable, Send + Sync; each successful start has one stop.
        let active = unsafe { url.startAccessingSecurityScopedResource() };
        if !active {
            let path = url
                .path()
                .map(|value| PathBuf::from(value.to_string()))
                .ok_or_else(folder_required)?;
            // URLs inside our own container do not need a sandbox extension.
            let home = dirs::home_dir().ok_or_else(folder_required)?;
            let inside_container = path.starts_with(&home)
                || std::fs::canonicalize(&path)
                    .ok()
                    .zip(std::fs::canonicalize(&home).ok())
                    .is_some_and(|(path, home)| path.starts_with(home));
            if !inside_container {
                return Err(folder_required());
            }
        }
        Ok(Self {
            url: url.clone(),
            active,
        })
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        if self.active {
            unsafe { self.url.stopAccessingSecurityScopedResource() };
        }
    }
}

impl Grant {
    fn selected(url: Retained<NSURL>, directory: bool, write: bool) -> AppResult<Self> {
        let path = PathBuf::from(url.path().ok_or_else(folder_required)?.to_string());
        let mut grant = Self {
            url,
            root: path.clone(),
            directory,
            write,
        };
        grant.root = coordinated(&grant, &path, false, |path| {
            let root = std::fs::canonicalize(path)?;
            if directory && !root.is_dir() {
                return Err(folder_required());
            }
            Ok(root)
        })?;
        Ok(grant)
    }

    fn check(&self, path: &str) -> AppResult<PathBuf> {
        let path = file_path(path)?;
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(folder_required());
        }
        if (self.directory && path.starts_with(&self.root))
            || (!self.directory && path == self.root)
        {
            Ok(path)
        } else {
            Err(folder_required())
        }
    }
}

fn grants(app: &AppHandle) -> tauri::State<'_, Grants> {
    if app.try_state::<Grants>().is_none() {
        app.manage(Grants::default());
    }
    app.state::<Grants>()
}

#[cfg(ios)]
async fn select(
    app: &AppHandle,
    directory: bool,
    multiple: bool,
    write: bool,
) -> AppResult<Option<Vec<String>>> {
    let Some(urls) = picker::pick(app, directory, multiple).await? else {
        return Ok(None);
    };
    let selected = tokio::task::spawn_blocking(move || {
        urls.into_iter()
            .map(|url| Grant::selected(url, directory, write))
            .collect::<AppResult<Vec<_>>>()
    })
    .await
    .map_err(join_error)??;
    let locations = selected.iter().map(|grant| file_url(&grant.root)).collect();
    let state = grants(app);
    let mut entries = state.0.lock().map_err(provider_error)?;
    for grant in selected {
        // Keep folder authority if selecting a source inside that folder later.
        entries.retain(|entry| entry.root != grant.root || (entry.directory && !grant.directory));
        entries.push(grant);
    }
    Ok(Some(locations))
}

#[cfg(ios)]
pub async fn pick_folder(app: &AppHandle, write: bool) -> AppResult<Option<String>> {
    Ok(select(app, true, false, write)
        .await?
        .and_then(|files| files.into_iter().next()))
}
#[cfg(ios)]
pub async fn pick_open(app: &AppHandle) -> AppResult<Option<String>> {
    Ok(select(app, false, false, false)
        .await?
        .and_then(|files| files.into_iter().next()))
}
#[cfg(ios)]
pub async fn pick_open_files(app: &AppHandle) -> AppResult<Option<Vec<String>>> {
    select(app, false, true, false).await
}

fn find_grant(app: &AppHandle, location: &str) -> AppResult<Option<Grant>> {
    let path = file_path(location)?;
    let state = grants(app);
    let entries = state.0.lock().map_err(provider_error)?;
    Ok(entries
        .iter()
        .filter(|entry| {
            (entry.directory && path.starts_with(&entry.root))
                || (!entry.directory && path == entry.root)
        })
        .max_by_key(|entry| entry.root.components().count())
        .cloned())
}

pub async fn transfer_endpoint(
    app: &AppHandle,
    location: String,
    write: bool,
) -> AppResult<(Arc<dyn TransferEndpoint>, String)> {
    let grant = match find_grant(app, &location)? {
        Some(grant) => grant,
        None if write => return Err(folder_required()),
        None => {
            // Source file pickers can authorize one file; uploads need no
            // sibling writes. Keep that authority restricted to the source.
            let path = file_path(&location)?;
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            tokio::task::spawn_blocking(move || Grant::selected(url, false, false))
                .await
                .map_err(join_error)??
        }
    };
    if write && !grant.write {
        return Err(folder_required());
    }
    let path = grant.check(&location)?.to_string_lossy().into_owned();
    Ok((Arc::new(ScopedEndpoint { grant, write }), path))
}

pub async fn resolve_paths(
    app: &AppHandle,
    root: String,
    paths: Vec<String>,
    write: bool,
) -> AppResult<Vec<ResolvedPath>> {
    let grant = find_grant(app, &root)?.ok_or_else(folder_required)?;
    if write && !grant.write {
        return Err(folder_required());
    }
    let base = grant.check(&root)?;
    tokio::task::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|relative| {
                let result = (|| {
                    crate::files::validate_relative_path(&relative)?;
                    let target = base.join(&relative);
                    coordinated(&grant, &base, write, |base| {
                        let parent = target.parent().ok_or_else(folder_required)?;
                        // Reject an existing symlink before creating any nested
                        // directory; a selected tree does not grant its target.
                        let mut cursor = base.to_owned();
                        for component in parent
                            .strip_prefix(base)
                            .map_err(provider_error)?
                            .components()
                        {
                            cursor.push(component);
                            match std::fs::symlink_metadata(&cursor) {
                                Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
                                Ok(_) => return Err(folder_required()),
                                Err(error)
                                    if error.kind() == std::io::ErrorKind::NotFound && write =>
                                {
                                    std::fs::create_dir(&cursor)?
                                }
                                Err(error) => return Err(error.into()),
                            }
                        }
                        let canonical = std::fs::canonicalize(parent)?;
                        if !canonical.starts_with(base) {
                            return Err(folder_required());
                        }
                        Ok(file_url(
                            &canonical.join(target.file_name().ok_or_else(folder_required)?),
                        ))
                    })
                })();
                ResolvedPath::new(relative, result)
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(join_error)
}

pub async fn walk_directory(app: &AppHandle, root: String) -> AppResult<Vec<LocalWalkEntry>> {
    let grant = find_grant(app, &root)?.ok_or_else(folder_required)?;
    let path = grant.check(&root)?;
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        coordinated(&grant, &path, false, |path| {
            runtime.block_on(super::native::walk_directory(
                path.to_string_lossy().into_owned(),
            ))
        })
    })
    .await
    .map_err(join_error)?
}

/// Accessor callbacks run synchronously while the coordinator owns its claim.
/// Rewritten URLs are rejected: a concurrent provider move must be revalidated.
fn coordinated<T>(
    grant: &Grant,
    path: &Path,
    write: bool,
    action: impl FnOnce(&Path) -> AppResult<T>,
) -> AppResult<T> {
    let _scope = Scope::start(&grant.url)?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let coordinator = NSFileCoordinator::new();
    let action = RefCell::new(Some(action));
    let result = RefCell::new(None);
    let accessor = RcBlock::new(|resolved: std::ptr::NonNull<NSURL>| {
        // The coordinator owns this NSURL throughout the callback.
        let resolved = unsafe { resolved.as_ref() }
            .path()
            .map(|path| PathBuf::from(path.to_string()));
        let value = match resolved {
            Some(ref resolved) if resolved == path => {
                action.borrow_mut().take().expect("one accessor")(resolved)
            }
            _ => Err(AppError::sftp(
                "sftp_transfer_target_changed",
                json!({"path": path}),
            )),
        };
        *result.borrow_mut() = Some(value);
    });
    let mut error = None;
    if write {
        coordinator.coordinateWritingItemAtURL_options_error_byAccessor(
            &url,
            NSFileCoordinatorWritingOptions::empty(),
            Some(&mut error),
            &accessor,
        );
    } else {
        coordinator.coordinateReadingItemAtURL_options_error_byAccessor(
            &url,
            NSFileCoordinatorReadingOptions::empty(),
            Some(&mut error),
            &accessor,
        );
    }
    if let Some(error) = error {
        return Err(provider_error(error.localizedDescription()));
    }
    drop(accessor);
    result
        .into_inner()
        .unwrap_or_else(|| Err(provider_error("file coordinator did not grant access")))
}

struct ScopedEndpoint {
    grant: Grant,
    write: bool,
}

impl ScopedEndpoint {
    async fn operation<T: Send + 'static>(
        &self,
        path: &str,
        write: bool,
        action: impl FnOnce(PathBuf) -> AppResult<T> + Send + 'static,
    ) -> AppResult<T> {
        if write && !self.write {
            return Err(folder_required());
        }
        let path = self.grant.check(path)?;
        let grant = self.grant.clone();
        tokio::task::spawn_blocking(move || {
            coordinated(&grant, &path, write, |path| action(path.to_owned()))
        })
        .await
        .map_err(join_error)?
    }
}

#[async_trait::async_trait]
impl TransferEndpoint for ScopedEndpoint {
    async fn canonicalize_target(&self, path: &str) -> AppResult<String> {
        let grant = self.grant.clone();
        self.operation(path, false, move |path| {
            let parent = path.parent().ok_or_else(folder_required)?;
            let canonical = std::fs::canonicalize(parent)?;
            if grant.directory && !canonical.starts_with(&grant.root) {
                return Err(folder_required());
            }
            Ok(canonical
                .join(path.file_name().ok_or_else(folder_required)?)
                .to_string_lossy()
                .into_owned())
        })
        .await
    }
    async fn sibling(&self, path: &str, name: &str) -> AppResult<String> {
        if !self.write || !self.grant.directory {
            return Err(folder_required());
        }
        let path = self.grant.check(path)?;
        NativeEndpoint.sibling(&path.to_string_lossy(), name).await
    }
    async fn stat(&self, path: &str) -> AppResult<Option<FileIdentity>> {
        let runtime = tokio::runtime::Handle::current();
        self.operation(path, false, move |path| {
            runtime.block_on(NativeEndpoint.stat(&path.to_string_lossy()))
        })
        .await
    }
    async fn open(&self, path: &str, mode: TransferMode) -> AppResult<Box<dyn TransferStream>> {
        let write = !matches!(mode, TransferMode::Read);
        if write && !self.write {
            return Err(folder_required());
        }
        let path = self.grant.check(path)?;
        let grant = self.grant.clone();
        let (opened_tx, opened_rx) = tokio::sync::oneshot::channel();
        let (close_tx, close_rx) = std::sync::mpsc::channel();
        let worker = tokio::task::spawn_blocking(move || {
            coordinated(&grant, &path, write, |path| {
                use std::os::unix::fs::OpenOptionsExt;
                let mut options = std::fs::OpenOptions::new();
                options.read(true).custom_flags(libc::O_NOFOLLOW);
                if write {
                    options.write(true);
                }
                if matches!(mode, TransferMode::CreateNew) {
                    options.create_new(true);
                }
                let file = options.open(path)?;
                if !file.metadata()?.is_file() {
                    return Err(provider_error("not a regular file"));
                }
                if opened_tx.send(file).is_ok() {
                    let _ = close_rx.recv();
                }
                Ok(())
            })
        });
        match opened_rx.await {
            Ok(file) => Ok(Box::new(CoordinatedStream {
                resource: Some(StreamResource {
                    file: tokio::fs::File::from_std(file),
                    close: close_tx,
                    worker,
                }),
            })),
            Err(_) => {
                worker.await.map_err(join_error)??;
                Err(provider_error(
                    "coordinated file open did not return a file",
                ))
            }
        }
    }
    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<(), RenameError> {
        if !self.write {
            return Err(RenameError::Other(folder_required()));
        }
        let from = self.grant.check(from).map_err(RenameError::Other)?;
        let to = self.grant.check(to).map_err(RenameError::Other)?;
        if from.parent() != to.parent() {
            return Err(RenameError::Other(folder_required()));
        }
        let grant = self.grant.clone();
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let _scope = Scope::start(&grant.url).map_err(RenameError::Other)?;
            let old_url = NSURL::fileURLWithPath(&NSString::from_str(&from.to_string_lossy()));
            let new_url = NSURL::fileURLWithPath(&NSString::from_str(&to.to_string_lossy()));
            let coordinator = NSFileCoordinator::new();
            let result = RefCell::new(None);
            let accessor = RcBlock::new(
                |old: std::ptr::NonNull<NSURL>, new: std::ptr::NonNull<NSURL>| {
                    // The coordinator owns both immutable NSURLs for this callback.
                    let (old, new) = unsafe { (old.as_ref(), new.as_ref()) };
                    let unchanged = old
                        .path()
                        .is_some_and(|p| Path::new(&p.to_string()) == from)
                        && new.path().is_some_and(|p| Path::new(&p.to_string()) == to);
                    if !unchanged {
                        *result.borrow_mut() = Some(Err(RenameError::Other(AppError::sftp(
                            "sftp_transfer_target_changed",
                            json!({"path": to}),
                        ))));
                        return;
                    }
                    coordinator.itemAtURL_willMoveToURL(old, new);
                    let renamed = runtime.block_on(NativeEndpoint.rename(
                        &from.to_string_lossy(),
                        &to.to_string_lossy(),
                        replace,
                    ));
                    if renamed.is_ok() {
                        coordinator.itemAtURL_didMoveToURL(old, new);
                    }
                    *result.borrow_mut() = Some(renamed);
                },
            );
            let mut error = None;
            coordinator
                .coordinateWritingItemAtURL_options_writingItemAtURL_options_error_byAccessor(
                    &old_url,
                    NSFileCoordinatorWritingOptions::ForMoving,
                    &new_url,
                    NSFileCoordinatorWritingOptions::ForReplacing,
                    Some(&mut error),
                    &accessor,
                );
            if let Some(error) = error {
                return Err(RenameError::Other(provider_error(
                    error.localizedDescription(),
                )));
            }
            drop(accessor);
            result.into_inner().unwrap_or_else(|| {
                Err(RenameError::Other(provider_error(
                    "rename was not coordinated",
                )))
            })
        })
        .await
        .map_err(|error| RenameError::Other(join_error(error)))?
    }

    async fn remove(&self, path: &str) -> AppResult<()> {
        self.operation(path, true, |path| {
            std::fs::remove_file(path)?;
            Ok(())
        })
        .await
    }
}

struct StreamResource {
    file: tokio::fs::File,
    close: std::sync::mpsc::Sender<()>,
    worker: tokio::task::JoinHandle<AppResult<()>>,
}
impl StreamResource {
    async fn close(mut self) -> AppResult<()> {
        let flushed = self.file.flush().await;
        // into_std waits for all pending Tokio file workers before the sandbox
        // scope and coordinator claim are released, including cancelled reads.
        drop(self.file.into_std().await);
        drop(self.close);
        let coordinated = self.worker.await.map_err(join_error)?;
        flushed?;
        coordinated
    }
}
struct CoordinatedStream {
    resource: Option<StreamResource>,
}
impl CoordinatedStream {
    fn file(&mut self) -> &mut tokio::fs::File {
        &mut self.resource.as_mut().expect("open stream").file
    }
}
impl AsyncRead for CoordinatedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(self.file()).poll_read(cx, buf)
    }
}
impl AsyncWrite for CoordinatedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(self.file()).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(self.file()).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(self.file()).poll_shutdown(cx)
    }
}
impl AsyncSeek for CoordinatedStream {
    fn start_seek(mut self: Pin<&mut Self>, position: std::io::SeekFrom) -> std::io::Result<()> {
        Pin::new(self.file()).start_seek(position)
    }
    fn poll_complete(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
        Pin::new(self.file()).poll_complete(cx)
    }
}
#[async_trait::async_trait]
impl TransferStream for CoordinatedStream {
    async fn close(mut self: Box<Self>) -> AppResult<()> {
        self.resource.take().expect("open stream").close().await
    }
}
impl Drop for CoordinatedStream {
    fn drop(&mut self) {
        if let Some(resource) = self.resource.take() {
            tokio::spawn(async move {
                if let Err(error) = resource.close().await {
                    log::warn!("failed to close coordinated iOS file: {error}");
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

    #[tokio::test]
    async fn coordinated_stream_closes_before_sibling_replacement_and_retains_resume_bytes() {
        let dir = tempfile::tempdir_in(dirs::home_dir().unwrap()).unwrap();
        let url = NSURL::fileURLWithPath(&NSString::from_str(&dir.path().to_string_lossy()));
        let endpoint = ScopedEndpoint {
            grant: Grant::selected(url, true, true).unwrap(),
            write: true,
        };
        let part = dir
            .path()
            .join("download.part")
            .to_string_lossy()
            .into_owned();
        let target = dir.path().join("download").to_string_lossy().into_owned();
        assert_eq!(endpoint.canonicalize_target(&target).await.unwrap(), target);
        assert!(endpoint.stat(&target).await.unwrap().is_none());
        let mut stream = endpoint.open(&part, TransferMode::CreateNew).await.unwrap();
        stream.write_all(b"prefix").await.unwrap();
        stream.close().await.unwrap();
        let mut stream = endpoint.open(&part, TransferMode::ReadWrite).await.unwrap();
        let mut bytes = [0; 6];
        stream.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"prefix");
        stream.seek(std::io::SeekFrom::Start(6)).await.unwrap();
        stream.write_all(b"tail").await.unwrap();
        stream.close().await.unwrap();
        std::fs::write(&target, b"old").unwrap();
        endpoint.rename(&part, &target, true).await.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"prefixtail");
    }

    #[tokio::test]
    async fn dropping_an_interrupted_stream_releases_the_coordinator_after_pending_io() {
        let dir = tempfile::tempdir_in(dirs::home_dir().unwrap()).unwrap();
        let url = NSURL::fileURLWithPath(&NSString::from_str(&dir.path().to_string_lossy()));
        let endpoint = ScopedEndpoint {
            grant: Grant::selected(url, true, true).unwrap(),
            write: true,
        };
        let part = dir
            .path()
            .join("interrupted.part")
            .to_string_lossy()
            .into_owned();
        let mut stream = endpoint.open(&part, TransferMode::CreateNew).await.unwrap();
        stream.write_all(b"retained bytes").await.unwrap();
        drop(stream);
        let mut reopened = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            endpoint.open(&part, TransferMode::ReadWrite),
        )
        .await
        .unwrap()
        .unwrap();
        let mut bytes = Vec::new();
        reopened.read_to_end(&mut bytes).await.unwrap();
        reopened.close().await.unwrap();
        assert_eq!(bytes, b"retained bytes");
    }

    #[tokio::test]
    async fn single_file_scope_cannot_create_siblings_or_escape_to_another_folder() {
        let dir = tempfile::tempdir_in(dirs::home_dir().unwrap()).unwrap();
        let path = dir.path().join("source");
        std::fs::write(&path, b"source").unwrap();
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let endpoint = ScopedEndpoint {
            grant: Grant::selected(url, false, false).unwrap(),
            write: false,
        };
        let path = path.to_string_lossy();
        assert!(endpoint.sibling(&path, "copy.part").await.is_err());
        assert!(endpoint.open(&path, TransferMode::ReadWrite).await.is_err());
        assert!(endpoint
            .stat(&dir.path().join("other").to_string_lossy())
            .await
            .is_err());
    }
}
