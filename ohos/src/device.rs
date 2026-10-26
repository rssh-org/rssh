use crate::error::AppResult;
use napi_derive_ohos::napi;
use openharmony_ability::{
    impl_bridge_napi_type, AsyncBridge, BridgeCallOptions, BridgeContextRequirement, BridgePlugin,
};

pub struct RuntimePlugin;
impl BridgePlugin for RuntimePlugin {
    type Mode = AsyncBridge;
    const ID: &'static str = "rssh.runtime";
    const REQUIRED_CONTEXTS: &'static [BridgeContextRequirement] =
        &[BridgeContextRequirement::Ability];
}

#[napi(object)]
pub struct DeviceRequest {}
impl_bridge_napi_type!(DeviceRequest, "rssh.runtime.DeviceRequest");

#[napi(object)]
pub struct DeviceResponse {
    pub device_type: String,
    pub multi_window: bool,
    pub folder_selection: bool,
    pub serial: bool,
}
impl_bridge_napi_type!(DeviceResponse, "rssh.runtime.DeviceResponse");

/// Both HAPs share one Rust library. Product device classes are runtime values,
/// never compile-time OS aliases or frontend layout breakpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceClass {
    Mobile,
    Desktop,
    Other,
}

impl DeviceResponse {
    pub fn class(&self) -> DeviceClass {
        match self.device_type.as_str() {
            "phone" | "tablet" => DeviceClass::Mobile,
            "2in1" => DeviceClass::Desktop,
            _ => DeviceClass::Other,
        }
    }
}

pub async fn query() -> AppResult<DeviceResponse> {
    super::app()?
        .bridge()
        .map_err(super::native_error)?
        .call_async::<RuntimePlugin, DeviceRequest, DeviceResponse>(
            "device",
            DeviceRequest {},
            BridgeCallOptions::default(),
        )
        .await
        .map_err(super::native_error)
}

pub use rssh_core::ohos::pty::local_pty_available;

/// Native capability and policy production stays inside the HarmonyOS adapter.
pub async fn capabilities(
    window_label: &str,
) -> AppResult<crate::platform::runtime::RuntimeCapabilities> {
    use crate::platform::runtime::{RuntimeCapabilities, TerminalRuntimePolicy};
    let device = query().await?;
    let pc = device.class() == DeviceClass::Desktop;
    let mut capabilities = RuntimeCapabilities::native();
    capabilities.serial = pc && device.serial;
    capabilities.local_pty = pc && local_pty_available().await;
    capabilities.local_discovery = capabilities.local_pty;
    capabilities.multi_window = pc && device.multi_window;
    capabilities.window_controls = capabilities.multi_window;
    capabilities.window_pin = capabilities.window_controls && window_label == "main";
    capabilities.directory_transfer = pc && device.folder_selection;
    capabilities.native_clipboard = true;
    capabilities.terminal_policy = if pc {
        TerminalRuntimePolicy::desktop()
    } else {
        TerminalRuntimePolicy::constrained()
    };
    Ok(capabilities)
}
