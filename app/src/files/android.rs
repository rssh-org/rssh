//! SAF identifies children by document ID; a tree URI is never converted to a local path.
use super::directory_required;
use crate::error::{AppError, AppResult};
use rssh_core::ssh::sftp::transfer::{DownloadDirectory, SourceStamp};
use serde::Deserialize;
use serde_json::{json, Value};
use std::os::fd::FromRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{AppHandle, Manager, Wry};
use tokio::fs::File;

struct Documents(PluginHandle<Wry>);

pub(crate) fn init() -> TauriPlugin<Wry> {
    Builder::new("download-directory")
        .setup(|app, api| {
            app.manage(Documents(api.register_android_plugin(
                "com.rssh.app",
                "DownloadDirectoryPlugin",
            )?));
            Ok(())
        })
        .build()
}

#[derive(Deserialize)]
struct Reply {
    fd: Option<i32>,
    uri: Option<String>,
    identity: Option<String>,
    size: Option<u64>,
    error: Option<String>,
    path: Option<String>,
    detail: Option<String>,
}

async fn call(
    app: &AppHandle,
    op: &'static str,
    args: Value,
    recovery_path: Option<String>,
) -> AppResult<Reply> {
    let handle = app.state::<Documents>();
    let response: Reply = handle
        .0
        .run_mobile_plugin_async(op, &args)
        .await
        .map_err(|e| {
            AppError::sftp(
                match op {
                    "publish" => "sftp_replace_uncertain",
                    "create" => "sftp_staging_retained",
                    _ => "sftp_directory_unsupported",
                },
                json!({"err": e.to_string(), "path": recovery_path}),
            )
        })?;
    if let Some(code) = &response.error {
        let code = match code.as_str() {
            "sftp_directory_required" => "sftp_directory_required",
            "sftp_staging_retained" => "sftp_staging_retained",
            "sftp_backup_cleanup" => "sftp_backup_cleanup",
            "sftp_replace_backup" => "sftp_replace_backup",
            "sftp_replace_uncertain" => "sftp_replace_uncertain",
            _ => "sftp_directory_unsupported",
        };
        return Err(AppError::sftp(
            code,
            json!({"err": response.detail, "path": response.path}),
        ));
    }
    Ok(response)
}

pub(super) async fn pick(app: &AppHandle) -> AppResult<Option<String>> {
    Ok(call(app, "pick", json!({}), None).await?.uri)
}

struct SafDirectory {
    app: AppHandle,
    parent: String,
    identity: String,
    scope: String,
    descriptor_open: AtomicBool,
}

pub(super) async fn directory(
    app: &AppHandle,
    root: &str,
    parent: &str,
) -> AppResult<Box<dyn DownloadDirectory>> {
    let reply = call(
        app,
        "directory",
        json!({"parent": root, "name": parent}),
        None,
    )
    .await?;
    Ok(Box::new(SafDirectory {
        app: app.clone(),
        parent: reply.uri.ok_or_else(directory_required)?,
        identity: reply.identity.ok_or_else(directory_required)?,
        scope: uuid::Uuid::new_v4().to_string(),
        descriptor_open: AtomicBool::new(false),
    }))
}

impl SafDirectory {
    async fn release_descriptor(&self) -> AppResult<()> {
        if self.descriptor_open.load(Ordering::Acquire) {
            call(&self.app, "release", json!({"scope": self.scope}), None).await?;
            self.descriptor_open.store(false, Ordering::Release);
        }
        Ok(())
    }

    async fn open(&self, op: &'static str, name: &str) -> AppResult<File> {
        self.release_descriptor().await?;
        self.descriptor_open.store(true, Ordering::Release);
        let reply = call(
            &self.app,
            op,
            json!({"parent": self.parent, "name": name, "scope": self.scope}),
            Some(self.display_path(name)),
        )
        .await?;
        let fd = reply
            .fd
            .filter(|fd| *fd >= 0)
            .ok_or_else(directory_required)?;
        // The plugin detaches a duplicate for this process; Rust is its sole owner.
        Ok(File::from_std(unsafe { std::fs::File::from_raw_fd(fd) }))
    }
}

impl Drop for SafDirectory {
    fn drop(&mut self) {
        // Errors and cancellations can leave the provider's close notifier outstanding.
        if self.descriptor_open.load(Ordering::Acquire) {
            let plugin = self.app.state::<Documents>();
            if let Err(error) = plugin
                .0
                .run_mobile_plugin::<Reply>("release", json!({"scope": self.scope}))
            {
                log::warn!("Could not release SAF staging descriptor: {error}");
            }
        }
    }
}

#[async_trait::async_trait]
impl DownloadDirectory for SafDirectory {
    async fn create_part(&self, name: &str) -> AppResult<File> {
        self.open("create", name).await
    }
    async fn open_part(&self, name: &str) -> AppResult<File> {
        self.open("reopen", name).await
    }
    async fn target_stamp(&self, name: &str) -> AppResult<Option<SourceStamp>> {
        self.release_descriptor().await?;
        let reply = call(
            &self.app,
            "inspect",
            json!({"parent": self.parent, "name": name}),
            None,
        )
        .await?;
        Ok(reply.identity.map(|id| SourceStamp::new(reply.size, id)))
    }
    async fn publish(&self, part: &str, name: &str) -> AppResult<()> {
        self.release_descriptor().await?;
        let backup = format!("{}.bak", part.trim_end_matches(".part"));
        let recovery = format!(
            "{}, {}",
            self.display_path(part),
            self.display_path(&backup)
        );
        call(
            &self.app,
            "publish",
            json!({"parent": self.parent, "part": part, "name": name}),
            Some(recovery),
        )
        .await?;
        Ok(())
    }
    fn display_path(&self, name: &str) -> String {
        format!("{}/{}", self.identity, name)
    }
}
