use std::fs::File;
use std::sync::{LazyLock, Mutex};

use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::{External, ExternalRef, FromNapiValue, Unknown};
use napi_ohos::{Env, Error, Result};
use openharmony_ability::{
    impl_bridge_napi_type, AsyncBridge, BridgeCallOptions, BridgeContextRequirement,
    BridgeMainThreadEvent, BridgeNapiType, BridgePlugin, PluginLifecycleEvent,
};
use openharmony_ability_plugin_files::{
    dialog_type, FileDialogFilter, FileDialogOptions, FilesExt,
};

use super::file_grants::FileGrants;
use crate::error::{AppError, AppResult};

static GRANTS: LazyLock<Mutex<FileGrants>> = LazyLock::new(|| Mutex::new(FileGrants::default()));

pub struct FilesAccessPlugin;

#[napi(object)]
pub struct NameRequest {
    pub uri: String,
}
impl_bridge_napi_type!(NameRequest, "rssh.files.NameRequest");

#[napi(object)]
pub struct FileName {
    pub name: String,
}
impl_bridge_napi_type!(FileName, "rssh.files.FileName");

#[napi(object)]
pub struct OpenRequest {
    pub uri: String,
    pub write: bool,
    pub create_in_directory: bool,
}
impl_bridge_napi_type!(OpenRequest, "rssh.files.OpenRequest");

#[napi(object)]
pub struct BorrowedFile {
    pub fd: i32,
}
impl_bridge_napi_type!(BorrowedFile, "rssh.files.BorrowedFile");

#[napi(object)]
pub struct DirectoryRequest {
    pub uri: String,
    pub write: bool,
    pub relative_paths: Vec<String>,
}
impl_bridge_napi_type!(DirectoryRequest, "rssh.files.DirectoryRequest");

#[napi(object)]
pub struct ResolvedPaths {
    pub files: Vec<String>,
}
impl_bridge_napi_type!(ResolvedPaths, "rssh.files.ResolvedPaths");

#[napi(object)]
pub struct FileTreeEntry {
    pub uri: String,
    pub relative_path: String,
    pub size: f64,
}

#[napi(object)]
pub struct FileTree {
    pub entries: Vec<FileTreeEntry>,
}
impl_bridge_napi_type!(FileTree, "rssh.files.FileTree");

/// Only the owned File crosses to a Rust worker. The N-API External stays on
/// the UI thread and closes an unclaimed File through its standard finalizer.
struct OpenedFile(File);
impl BridgeNapiType for OpenedFile {
    const TYPE_NAME: &'static str = "rssh.files.OpenedFile";

    fn into_bridge_value<'env>(self, env: &'env Env) -> Result<Unknown<'env>> {
        External::new(Some(self.0)).into_unknown(env)
    }

    fn from_bridge_value(value: Unknown<'_>) -> Result<Self> {
        let mut owned = ExternalRef::<Option<File>>::from_unknown(value)?;
        owned
            .take()
            .map(Self)
            .ok_or_else(|| Error::from_reason("File ownership was already transferred"))
    }
}

impl BridgePlugin for FilesAccessPlugin {
    type Mode = AsyncBridge;
    const ID: &'static str = "rssh.files-access";
    const REQUIRED_CONTEXTS: &'static [BridgeContextRequirement] =
        &[BridgeContextRequirement::Ability];

    fn on_main_thread_event<'env>(
        &self,
        event: BridgeMainThreadEvent<'env>,
    ) -> Result<Unknown<'env>> {
        if event.name() != "duplicate-file" {
            return Err(Error::from_reason("Unknown file-access event"));
        }
        let request = event.decode::<BorrowedFile>()?;
        let file = super::file_descriptor::duplicate_file(request.fd)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        event.respond(OpenedFile(file))
    }

    fn on_lifecycle(&self, event: &PluginLifecycleEvent) -> Result<()> {
        if matches!(
            event,
            PluginLifecycleEvent::AbilityDestroyed | PluginLifecycleEvent::AbilityCreated { .. }
        ) {
            GRANTS
                .lock()
                .map_err(|_| Error::from_reason("File grant lock poisoned"))?
                .clear();
        }
        Ok(())
    }
}

async fn pick(options: FileDialogOptions, write: bool) -> AppResult<Vec<String>> {
    let generation = GRANTS
        .lock()
        .map_err(|_| super::unavailable())?
        .generation();
    let selected = super::app()?
        .show_file_dialog(options)
        .await
        .map_err(super::native_error)?
        .files;
    if !GRANTS
        .lock()
        .map_err(|_| super::unavailable())?
        .grant(generation, &selected, write)
    {
        return Err(super::unavailable());
    }
    Ok(selected)
}

pub async fn pick_open() -> AppResult<Option<String>> {
    Ok(pick(FileDialogOptions::new(dialog_type::OPEN_FILE), false)
        .await?
        .into_iter()
        .next())
}

pub async fn file_name(uri: String) -> AppResult<String> {
    let bridge = super::app()?.bridge().map_err(super::native_error)?;
    let result = bridge
        .call_async::<FilesAccessPlugin, NameRequest, FileName>(
            "file-name",
            NameRequest { uri },
            BridgeCallOptions::default(),
        )
        .await
        .map_err(super::native_error)?;
    Ok(result.name)
}

pub async fn pick_open_files() -> AppResult<Option<Vec<String>>> {
    let files = pick(
        FileDialogOptions::new(dialog_type::OPEN_FILE).allow_many(true),
        false,
    )
    .await?;
    Ok((!files.is_empty()).then_some(files))
}

pub async fn pick_folder(write: bool) -> AppResult<Option<String>> {
    let generation = GRANTS
        .lock()
        .map_err(|_| super::unavailable())?
        .generation();
    let selected = super::app()?
        .show_file_dialog(FileDialogOptions::new(dialog_type::OPEN_FOLDER))
        .await
        .map_err(super::native_error)?
        .files
        .into_iter()
        .next();
    if let Some(uri) = &selected {
        if !GRANTS
            .lock()
            .map_err(|_| super::unavailable())?
            .grant_directory(generation, uri.clone(), write)
        {
            return Err(super::unavailable());
        }
    }
    Ok(selected)
}

pub async fn resolve_paths(
    uri: String,
    relative_paths: Vec<String>,
    write: bool,
) -> AppResult<Vec<String>> {
    let bridge = super::app()?.bridge().map_err(super::native_error)?;
    let generation = directory_generation(&uri, write)?;
    let resolved = bridge
        .call_async::<FilesAccessPlugin, DirectoryRequest, ResolvedPaths>(
            "resolve-paths",
            DirectoryRequest {
                uri,
                write,
                relative_paths,
            },
            BridgeCallOptions::default(),
        )
        .await
        .map_err(super::native_error)?;
    grant_files(generation, &resolved.files, write)?;
    Ok(resolved.files)
}

pub async fn walk_directory(uri: String) -> AppResult<Vec<crate::files::LocalWalkEntry>> {
    let bridge = super::app()?.bridge().map_err(super::native_error)?;
    let generation = directory_generation(&uri, false)?;
    let tree = bridge
        .call_async::<FilesAccessPlugin, DirectoryRequest, FileTree>(
            "walk-directory",
            DirectoryRequest {
                uri,
                write: false,
                relative_paths: Vec::new(),
            },
            BridgeCallOptions::default(),
        )
        .await
        .map_err(super::native_error)?;
    let files: Vec<_> = tree.entries.iter().map(|entry| entry.uri.clone()).collect();
    grant_files(generation, &files, false)?;
    Ok(tree
        .entries
        .into_iter()
        .map(|entry| crate::files::LocalWalkEntry {
            rel_path: entry.relative_path,
            size: entry.size as u64,
            local_path: entry.uri,
        })
        .collect())
}

fn directory_generation(uri: &str, write: bool) -> AppResult<u64> {
    let grants = GRANTS.lock().map_err(|_| super::unavailable())?;
    if !grants.permits_directory(uri, write) {
        return Err(selection_error());
    }
    Ok(grants.generation())
}

fn grant_files(generation: u64, files: &[String], write: bool) -> AppResult<()> {
    if !GRANTS
        .lock()
        .map_err(|_| super::unavailable())?
        .grant_directory_files(generation, files, write)
    {
        return Err(super::unavailable());
    }
    Ok(())
}

fn selection_error() -> AppError {
    AppError::other(
        "ohos_native_failed",
        serde_json::json!({"err": "File was not selected for this operation"}),
    )
}

pub async fn pick_save(
    default_name: String,
    filters: Vec<FileDialogFilter>,
) -> AppResult<Option<String>> {
    if !super::file_name::valid_file_name(&default_name) {
        return Err(AppError::other(
            "ohos_invalid_filename",
            serde_json::json!({}),
        ));
    }
    let options = FileDialogOptions::new(dialog_type::SAVE_FILE)
        .default_name(default_name)
        .filters(filters);
    Ok(pick(options, true).await?.into_iter().next())
}

pub async fn open_file(uri: String, write: bool) -> AppResult<File> {
    // Bind the request to this Ability session before checking its grants. A
    // destruction between the check and dispatch then cancels the old bridge,
    // instead of opening an old selection through a newly created Ability.
    let bridge = super::app()?.bridge().map_err(super::native_error)?;
    let create_in_directory = {
        let grants = GRANTS.lock().map_err(|_| super::unavailable())?;
        if !grants.permits(&uri, write) {
            return Err(selection_error());
        }
        write && grants.creates_in_directory(&uri)
    };
    let opened = bridge
        .call_async::<FilesAccessPlugin, OpenRequest, OpenedFile>(
            "open-file",
            OpenRequest {
                uri,
                write,
                create_in_directory,
            },
            BridgeCallOptions::default(),
        )
        .await
        .map_err(super::native_error)?;
    Ok(opened.0)
}
