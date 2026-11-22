use crate::commands::sftp::transfer::Attempt;
use crate::emitter::Host;
use crate::error::{AppError, AppResult};
use crate::files::{download_siblings_unavailable, PickedDownload};
use rssh_core::files::atomic::{discard_owned_file, DownloadDirectory, FileId, FileSnapshot};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::json;
use std::fs::File;
use std::io::{self, ErrorKind};
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::plugin::{mobile::PluginInvokeError, Builder, PluginHandle, TauriPlugin};
use tauri::{Manager, Resource, WebviewWindow, Wry};

struct AndroidDownloads(PluginHandle<Wry>);

pub(crate) fn init() -> TauriPlugin<Wry> {
    Builder::new("download-directory")
        .setup(|app, api| {
            let handle = api.register_android_plugin("com.rssh.app", "DownloadDirectoryPlugin")?;
            app.manage(AndroidDownloads(handle));
            Ok(())
        })
        .build()
}

#[derive(Deserialize)]
struct Selection {
    tree: String,
    folder: String,
}

pub(crate) struct FolderGrant {
    handle: PluginHandle<Wry>,
    tree: String,
    destination: PathBuf,
    // SAF can expose different inodes for a read-only (transcoded) view and
    // the writable original. Follow the staged file through its exact rename.
    staged: Mutex<PathBuf>,
}

impl Resource for FolderGrant {}

impl std::fmt::Debug for FolderGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FolderGrant")
            .field("destination", &self.destination)
            .finish_non_exhaustive()
    }
}

fn code(error: &PluginInvokeError) -> Option<&str> {
    match error {
        PluginInvokeError::InvokeRejected(response) => response.code.as_deref(),
        _ => None,
    }
}

pub(crate) async fn pick(
    window: &WebviewWindow,
    name: String,
) -> AppResult<Option<PickedDownload>> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        return Err(AppError::sftp("sftp_invalid_filename", json!({})));
    }
    let handle = window.state::<AndroidDownloads>().0.clone();
    let selected: Option<Selection> = handle
        .run_mobile_plugin_async("pickDirectory", json!({ "name": name }))
        .await
        .map_err(|err| match code(&err) {
            Some("sftp_provider_siblings_unavailable") => download_siblings_unavailable(),
            Some("sftp_folder_picker_busy") => AppError::sftp("sftp_folder_picker_busy", json!({})),
            Some("sftp_folder_operations_unsupported") => {
                AppError::sftp("sftp_folder_operations_unsupported", json!({}))
            }
            Some("sftp_folder_picker_unavailable") => {
                AppError::sftp("sftp_folder_picker_unavailable", json!({}))
            }
            _ => AppError::sftp(
                "sftp_io_failed",
                json!({"op":"pick", "err": err.to_string()}),
            ),
        })?;
    let Some(selection) = selected else {
        return Ok(None);
    };
    let destination = Path::new(&selection.folder).join(name);
    let location = destination.to_string_lossy().into_owned();
    let grant_id = window.resources_table().add(FolderGrant {
        handle,
        tree: selection.tree,
        destination,
        staged: Mutex::new(PathBuf::new()),
    });
    Ok(Some(PickedDownload::Folder { location, grant_id }))
}

pub(crate) async fn download(
    window: &WebviewWindow,
    grant_id: u32,
    location: &str,
    attempt: Attempt,
    host: Host,
) -> AppResult<u64> {
    let grant = window
        .resources_table()
        .get::<FolderGrant>(grant_id)
        .map_err(|_| download_siblings_unavailable())?;
    if grant.destination.to_str() != Some(location) {
        return Err(download_siblings_unavailable());
    }
    // SAF metadata calls are synchronous. Keep this transfer's original future
    // and checkpoint lock together, while Tokio can schedule other work.
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(attempt.download_in(
            &host,
            grant.destination.clone(),
            grant,
        ))
    })
}

fn name(path: &Path) -> io::Result<&str> {
    path.file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "Missing document name"))
}

impl FolderGrant {
    fn staged_path(&self) -> std::sync::MutexGuard<'_, PathBuf> {
        self.staged.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn operation<T: DeserializeOwned>(
        &self,
        op: &str,
        path: &Path,
        to: Option<&Path>,
        allow_existing: bool,
    ) -> io::Result<T> {
        let staged = self.staged_path().as_path() == path;
        self.handle.run_mobile_plugin("fileOperation", json!({
            "tree": self.tree, "op": op, "name": name(path)?, "to": to.map(name).transpose()?,
            "allowExisting": allow_existing, "staged": staged,
        })).map_err(|e| {
            let kind = match code(&e) {
                Some("already_exists") => ErrorKind::AlreadyExists,
                Some("sftp_provider_siblings_unavailable") => ErrorKind::PermissionDenied,
                Some("sftp_folder_operations_unsupported") => ErrorKind::Unsupported,
                _ => ErrorKind::Other,
            };
            let message = match &e {
                PluginInvokeError::InvokeRejected(response) => response.message.clone(),
                _ => None,
            };
            io::Error::new(kind, message.unwrap_or_else(|| e.to_string()))
        })
    }

    fn file(&self, op: &str, path: &Path) -> io::Result<Option<File>> {
        let result: Option<i32> = self.operation(op, path, None, false)?;
        result
            .map(|fd| {
                if fd < 0 {
                    return Err(io::Error::other(
                        "Document provider returned an invalid descriptor",
                    ));
                }
                // The plugin detached a dup. Its original writer stays open
                // until close_writer signals completion to the provider.
                Ok(unsafe { File::from_raw_fd(fd) })
            })
            .transpose()
    }
}

impl DownloadDirectory for FolderGrant {
    fn document_key(&self) -> Option<String> {
        Some(format!(
            "{}|{}",
            self.tree.to_lowercase(),
            self.destination
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase()
        ))
    }

    fn snapshot(&self, path: &Path) -> io::Result<Option<FileSnapshot>> {
        let op = if self.staged_path().as_path() == path {
            "snapshotWritable"
        } else {
            "snapshot"
        };
        self.file(op, path)?
            .as_ref()
            .map(FileSnapshot::from_file)
            .transpose()
    }

    fn create(&self, path: &Path) -> io::Result<File> {
        let file = self
            .file("create", path)?
            .ok_or_else(|| io::Error::other("Document creation was not confirmed"))?;
        *self.staged_path() = path.to_path_buf();
        Ok(file)
    }

    fn open(&self, path: &Path) -> io::Result<File> {
        self.file("open", path)?
            .ok_or_else(|| ErrorKind::NotFound.into())
    }

    fn rename(&self, from: &Path, to: &Path, allow_existing: bool) -> io::Result<()> {
        self.operation::<()>("rename", from, Some(to), allow_existing)?;
        let mut staged = self.staged_path();
        if staged.as_path() == from {
            *staged = to.to_path_buf();
        }
        Ok(())
    }

    fn close_writer(&self, path: &Path) -> io::Result<()> {
        self.operation("closeWriter", path, None, false)
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        self.operation("remove", path, None, false)
    }

    fn discard(self: Arc<Self>, path: PathBuf, id: FileId) {
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = discard_owned_file(self.as_ref(), &path, &id) {
                log::warn!(
                    "Could not remove download partial {}: {error}",
                    path.display()
                );
            }
        });
    }
}
