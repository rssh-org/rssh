//! Android SAF tree adapter. The provider owns document IDs and rename results;
//! no content URI is converted into a filesystem path.
use super::{LocalWalkEntry, ResolvedPath};
use crate::error::{AppError, AppResult};
use rssh_core::files::transfer::{
    FileIdentity, RenameError, TransferEndpoint, TransferMode, TransferStream,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tauri::{
    plugin::{Builder, PluginHandle, TauriPlugin},
    AppHandle, Manager, Wry,
};

struct Trees(PluginHandle<Wry>);

pub fn init() -> TauriPlugin<Wry> {
    Builder::new("file-trees")
        .setup(|app, api| {
            app.manage(Trees(
                api.register_android_plugin("com.rssh.app", "FileTreesPlugin")?,
            ));
            Ok(())
        })
        .build()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Request<'a> {
    op: &'a str,
    path: &'a str,
    to: Option<&'a str>,
    write: bool,
    replace: bool,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reply {
    location: Option<String>,
    uri: Option<String>,
    identity: Option<String>,
    name: Option<String>,
    size: Option<u64>,
    modified: Option<u64>,
    absent: Option<bool>,
    conflict: Option<bool>,
    error: Option<String>,
    recovery: Option<String>,
    entries: Option<Vec<TreeEntry>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TreeEntry {
    location: String,
    relative_path: String,
    size: u64,
}

fn provider_error(reply: &Reply) -> AppError {
    AppError::other(
        if reply.recovery.is_some() {
            "sftp_provider_recovery_preserved"
        } else {
            "sftp_provider_unsupported"
        },
        json!({"err": reply.error.as_deref().unwrap_or("Provider operation failed"), "path":reply.recovery}),
    )
}

async fn call(
    app: &AppHandle,
    op: &str,
    path: &str,
    to: Option<&str>,
    write: bool,
    replace: bool,
) -> AppResult<Reply> {
    let state = app.state::<Trees>();
    state
        .0
        .run_mobile_plugin_async(
            "operate",
            Request {
                op,
                path,
                to,
                write,
                replace,
            },
        )
        .await
        .map_err(|error| {
            AppError::other(
                "sftp_provider_unsupported",
                json!({"err":error.to_string()}),
            )
        })
}

pub async fn pick_folder(app: &AppHandle, write: bool) -> AppResult<Option<String>> {
    let reply: Reply = app
        .state::<Trees>()
        .0
        .run_mobile_plugin_async("pickFolder", json!({"write":write}))
        .await
        .map_err(|error| {
            AppError::other("sftp_folder_required", json!({"err":error.to_string()}))
        })?;
    if reply.error.is_some() {
        return Err(provider_error(&reply));
    }
    Ok(reply.location)
}

pub async fn resolve_paths(
    app: &AppHandle,
    root: String,
    paths: Vec<String>,
    write: bool,
) -> AppResult<Vec<ResolvedPath>> {
    let mut result = Vec::with_capacity(paths.len());
    for relative in paths {
        let location = async {
            super::validate_relative_path(&relative)?;
            let reply = call(app, "resolve", &root, Some(&relative), write, false).await?;
            if reply.error.is_some() {
                return Err(provider_error(&reply));
            }
            reply.location.ok_or_else(super::folder_required)
        }
        .await;
        result.push(ResolvedPath::new(relative, location));
    }
    Ok(result)
}

pub async fn walk_directory(app: &AppHandle, root: String) -> AppResult<Vec<LocalWalkEntry>> {
    let reply = call(app, "walk", &root, None, false, false).await?;
    if reply.error.is_some() {
        return Err(provider_error(&reply));
    }
    Ok(reply
        .entries
        .unwrap_or_default()
        .into_iter()
        .map(|entry| LocalWalkEntry {
            rel_path: entry.relative_path,
            size: entry.size,
            local_path: entry.location,
        })
        .collect())
}

pub async fn file_name(app: &AppHandle, location: &str) -> AppResult<Option<String>> {
    let reply = call(app, "name", location, None, false, false).await?;
    if reply.error.is_some() {
        return Err(provider_error(&reply));
    }
    Ok(reply.name)
}

pub async fn transfer_endpoint(
    app: &AppHandle,
    location: String,
    _write: bool,
) -> AppResult<(Arc<dyn TransferEndpoint>, String)> {
    Ok((Arc::new(AndroidTree { app: app.clone() }), location))
}

struct AndroidTree {
    app: AppHandle,
}

#[async_trait::async_trait]
impl TransferEndpoint for AndroidTree {
    async fn canonicalize_target(&self, path: &str) -> AppResult<String> {
        let reply = call(&self.app, "canonical", path, None, false, false).await?;
        if reply.error.is_some() {
            return Err(provider_error(&reply));
        }
        reply.identity.ok_or_else(super::folder_required)
    }
    async fn sibling(&self, path: &str, name: &str) -> AppResult<String> {
        let reply = call(&self.app, "sibling", path, Some(name), false, false).await?;
        if reply.error.is_some() {
            return Err(provider_error(&reply));
        }
        reply.location.ok_or_else(super::folder_required)
    }
    async fn stat(&self, path: &str) -> AppResult<Option<FileIdentity>> {
        let reply = call(&self.app, "stat", path, None, false, false).await?;
        if reply.error.is_some() {
            return Err(provider_error(&reply));
        }
        if reply.absent == Some(true) {
            return Ok(None);
        }
        Ok(Some(FileIdentity {
            size: reply.size.ok_or_else(|| provider_error(&reply))?,
            modified: reply
                .modified
                .and_then(|value| value.checked_mul(1_000_000)),
            stable_id: reply.identity,
        }))
    }
    async fn open(&self, path: &str, mode: TransferMode) -> AppResult<Box<dyn TransferStream>> {
        use tauri_plugin_fs::{FsExt, OpenOptions};
        use tokio::io::AsyncSeekExt;
        let write = !matches!(mode, TransferMode::Read);
        let reply = call(
            &self.app,
            if matches!(mode, TransferMode::CreateNew) {
                "create"
            } else {
                "open"
            },
            path,
            None,
            write,
            false,
        )
        .await?;
        if reply.error.is_some() {
            return Err(provider_error(&reply));
        }
        let uri = reply.uri.ok_or_else(super::folder_required)?;
        let mut options = OpenOptions::new();
        options.read(true).write(write);
        let file = self.app.fs().open(
            uri.parse::<tauri_plugin_fs::FilePath>()
                .expect("infallible FilePath"),
            options,
        )?;
        let mut file = tokio::fs::File::from_std(file);
        // Cloud providers may return pipes. Detect this before transferring any
        // bytes; strict prefix readback and resume require a seekable descriptor.
        file.seek(std::io::SeekFrom::Start(0))
            .await
            .map_err(|error| {
                AppError::other(
                    "sftp_provider_unsupported",
                    json!({"err":error.to_string()}),
                )
            })?;
        Ok(Box::new(file))
    }
    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<(), RenameError> {
        let reply = call(&self.app, "rename", from, Some(to), true, replace)
            .await
            .map_err(RenameError::Other)?;
        if reply.conflict == Some(true) {
            return Err(RenameError::Conflict(provider_error(&reply)));
        }
        if reply.error.is_some() {
            return Err(RenameError::Other(provider_error(&reply)));
        }
        Ok(())
    }
    async fn remove(&self, path: &str) -> AppResult<()> {
        let reply = call(&self.app, "remove", path, None, true, false).await?;
        if reply.error.is_some() {
            return Err(provider_error(&reply));
        }
        Ok(())
    }
}
