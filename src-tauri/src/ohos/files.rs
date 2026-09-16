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
pub struct OpenRequest {
    pub uri: String,
    pub write: bool,
}
impl_bridge_napi_type!(OpenRequest, "rssh.files.OpenRequest");

#[napi(object)]
pub struct BorrowedFile {
    pub fd: i32,
}
impl_bridge_napi_type!(BorrowedFile, "rssh.files.BorrowedFile");

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
    if !GRANTS
        .lock()
        .map_err(|_| super::unavailable())?
        .permits(&uri, write)
    {
        return Err(AppError::other(
            "ohos_native_failed",
            serde_json::json!({"err": "File was not selected for this operation"}),
        ));
    }
    let opened = bridge
        .call_async::<FilesAccessPlugin, OpenRequest, OpenedFile>(
            "open-file",
            OpenRequest { uri, write },
            BridgeCallOptions::default(),
        )
        .await
        .map_err(super::native_error)?;
    Ok(opened.0)
}
