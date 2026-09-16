use crate::error::AppResult;

/// What this host implements. Device shape is a separate frontend concern.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCapabilities {
    pub local_pty: bool,
    pub serial: bool,
    pub local_discovery: bool,
    pub ssh_agent: bool,
    pub default_key_files: bool,
    pub cli_install: bool,
    pub multi_window: bool,
    pub window_controls: bool,
    pub window_pin: bool,
    pub file_multi_select: bool,
    pub directory_transfer: bool,
    pub plugins: bool,
}

impl RuntimeCapabilities {
    fn native() -> Self {
        Self {
            local_pty: cfg!(desktop),
            serial: cfg!(desktop),
            local_discovery: cfg!(desktop),
            ssh_agent: cfg!(desktop),
            default_key_files: cfg!(desktop),
            cli_install: cfg!(desktop),
            multi_window: cfg!(desktop),
            window_controls: cfg!(desktop),
            window_pin: cfg!(desktop),
            file_multi_select: cfg!(desktop),
            directory_transfer: cfg!(desktop),
            plugins: true,
        }
    }

    #[cfg(all(feature = "server", desktop))]
    pub(crate) fn headless() -> Self {
        Self {
            cli_install: false,
            window_controls: false,
            window_pin: false,
            plugins: false,
            ..Self::native()
        }
    }
}

#[tauri::command]
pub async fn get_runtime_capabilities(window: tauri::Window) -> AppResult<RuntimeCapabilities> {
    let capabilities = RuntimeCapabilities::native();
    #[cfg(target_env = "ohos")]
    {
        let mut capabilities = capabilities;
        let device = crate::ohos::device::query().await?;
        let pc = device.device_type == "2in1";
        capabilities.serial = pc && device.serial;
        capabilities.local_pty = pc && crate::ohos::device::local_pty_available().await;
        // Discovery commands retain their own executable/version checks.
        capabilities.local_discovery = capabilities.local_pty;
        capabilities.multi_window = pc && device.multi_window;
        capabilities.window_controls = pc && device.multi_window;
        capabilities.window_pin = capabilities.window_controls && window.label() == "main";
        capabilities.file_multi_select = true;
        capabilities.directory_transfer = pc && device.folder_selection;
        return Ok(capabilities);
    }
    #[cfg(not(target_env = "ohos"))]
    {
        let _ = window;
        Ok(capabilities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_capabilities_have_the_shared_wire_contract() {
        let value = serde_json::to_value(RuntimeCapabilities::native()).unwrap();
        for key in [
            "localPty",
            "serial",
            "localDiscovery",
            "sshAgent",
            "defaultKeyFiles",
            "cliInstall",
            "multiWindow",
            "windowControls",
            "windowPin",
            "fileMultiSelect",
            "directoryTransfer",
            "plugins",
        ] {
            assert!(value[key].is_boolean(), "missing boolean capability {key}");
        }
        assert_eq!(value.as_object().unwrap().len(), 12);
    }

    #[test]
    #[cfg(all(feature = "server", desktop))]
    fn headless_uses_browser_windows_and_host_managed_cli() {
        let capabilities = RuntimeCapabilities::headless();
        assert!(capabilities.multi_window);
        assert!(!capabilities.window_controls);
        assert!(!capabilities.window_pin);
        assert!(!capabilities.cli_install);
        assert!(!capabilities.plugins);
        assert!(capabilities.local_pty);
    }
}
