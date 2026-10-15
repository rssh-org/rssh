use crate::error::AppResult;
use napi_derive_ohos::napi;
use openharmony_ability::{
    impl_bridge_napi_type, AsyncBridge, BridgeCallOptions, BridgeContextRequirement, BridgePlugin,
};
use openharmony_ability_plugin_permission::PermissionExt;

pub struct ClipboardPlugin;
impl BridgePlugin for ClipboardPlugin {
    type Mode = AsyncBridge;
    const ID: &'static str = "rssh.clipboard";
    const REQUIRED_CONTEXTS: &'static [BridgeContextRequirement] =
        &[BridgeContextRequirement::Ability];
}

#[napi(object)]
pub struct ReadRequest {}
impl_bridge_napi_type!(ReadRequest, "rssh.clipboard.ReadRequest");
#[napi(object)]
pub struct TextResponse {
    pub text: String,
}
impl_bridge_napi_type!(TextResponse, "rssh.clipboard.TextResponse");
#[napi(object)]
pub struct WriteRequest {
    pub text: String,
}
impl_bridge_napi_type!(WriteRequest, "rssh.clipboard.WriteRequest");
#[napi(object)]
pub struct WriteResponse {}
impl_bridge_napi_type!(WriteResponse, "rssh.clipboard.WriteResponse");

pub async fn read_text() -> AppResult<String> {
    let app = super::app()?;
    let permissions = app
        .request_permission("ohos.permission.READ_PASTEBOARD")
        .await
        .map_err(super::native_error)?;
    if permissions.len() != 1 || permissions[0].code != 0 {
        return Err(super::native_error("Clipboard access was not granted"));
    }
    let response = app
        .bridge()
        .map_err(super::native_error)?
        .call_async::<ClipboardPlugin, ReadRequest, TextResponse>(
            "read-text",
            ReadRequest {},
            BridgeCallOptions::default(),
        )
        .await
        .map_err(super::native_error)?;
    Ok(response.text)
}

pub async fn write_text(text: String) -> AppResult<()> {
    super::app()?
        .bridge()
        .map_err(super::native_error)?
        .call_async::<ClipboardPlugin, WriteRequest, WriteResponse>(
            "write-text",
            WriteRequest { text },
            BridgeCallOptions::default(),
        )
        .await
        .map_err(super::native_error)?;
    Ok(())
}
