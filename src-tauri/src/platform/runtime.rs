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
    pub native_clipboard: bool,
    pub release_update_check: bool,
    pub terminal_policy: TerminalRuntimePolicy,
}

impl RuntimeCapabilities {
    pub(crate) fn native() -> Self {
        Self {
            local_pty: cfg!(any(windows, macos, linux)),
            serial: cfg!(any(windows, macos, linux)),
            local_discovery: cfg!(any(windows, macos, linux)),
            ssh_agent: cfg!(any(windows, macos, linux)),
            default_key_files: cfg!(any(windows, macos, linux)),
            cli_install: cfg!(any(windows, macos, linux)),
            multi_window: cfg!(any(windows, macos, linux)),
            window_controls: cfg!(any(windows, macos, linux)),
            window_pin: cfg!(any(windows, macos, linux)),
            // Native file pickers support multiple sources on desktop and mobile.
            // Directory authorization remains a separate host capability.
            file_multi_select: true,
            directory_transfer: cfg!(any(windows, macos, linux)),
            plugins: true,
            native_clipboard: cfg!(any(windows, macos, linux)),
            // App Store/TestFlight manages native iOS releases. Browser clients
            // follow their RSSH server's policy, not the browser's operating system.
            release_update_check: !cfg!(ios),
            terminal_policy: if cfg!(any(android, ios)) {
                TerminalRuntimePolicy::constrained()
            } else {
                TerminalRuntimePolicy::desktop()
            },
        }
    }

    #[cfg(all(feature = "server", any(windows, macos, linux)))]
    pub(crate) fn headless() -> Self {
        Self {
            cli_install: false,
            window_controls: false,
            window_pin: false,
            plugins: false,
            native_clipboard: false,
            ..Self::native()
        }
    }
}

/// Resource and renderer defaults are host policy, never layout or input detection.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRuntimePolicy {
    pub image_storage_limit_mb: u32,
    pub image_pixel_limit: u32,
    pub output_backlog_limit_bytes: u32,
    pub gpu_render_default: bool,
}

impl TerminalRuntimePolicy {
    pub(crate) fn desktop() -> Self {
        Self {
            image_storage_limit_mb: 128,
            image_pixel_limit: 16_000_000,
            output_backlog_limit_bytes: 128 * 1024 * 1024,
            gpu_render_default: true,
        }
    }

    pub(crate) fn constrained() -> Self {
        Self {
            image_storage_limit_mb: 32,
            image_pixel_limit: 4_000_000,
            output_backlog_limit_bytes: 32 * 1024 * 1024,
            gpu_render_default: false,
        }
    }
}

pub async fn capabilities(window: &tauri::Window) -> AppResult<RuntimeCapabilities> {
    #[cfg(ohos)]
    {
        crate::ohos::device::capabilities(window.label()).await
    }
    #[cfg(not(ohos))]
    {
        let _ = window;
        Ok(RuntimeCapabilities::native())
    }
}

/// Android/iOS retain their existing diagnosis policy for private artifacts.
pub fn allows_diagnosis_downloads() -> bool {
    !cfg!(any(android, ios))
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
            "nativeClipboard",
            "releaseUpdateCheck",
        ] {
            assert!(value[key].is_boolean(), "missing boolean capability {key}");
        }
        assert_eq!(value.as_object().unwrap().len(), 15);
        assert_eq!(value["fileMultiSelect"], true);
        assert_eq!(value["releaseUpdateCheck"], !cfg!(ios));
        let expected = if cfg!(any(android, ios)) {
            TerminalRuntimePolicy::constrained()
        } else {
            TerminalRuntimePolicy::desktop()
        };
        assert_eq!(
            value["terminalPolicy"],
            serde_json::to_value(expected).unwrap()
        );
    }

    #[test]
    #[cfg(all(feature = "server", any(windows, macos, linux)))]
    fn headless_uses_browser_windows_and_host_managed_cli() {
        let capabilities = RuntimeCapabilities::headless();
        assert!(capabilities.multi_window);
        assert!(!capabilities.window_controls);
        assert!(!capabilities.window_pin);
        assert!(!capabilities.cli_install);
        assert!(!capabilities.plugins);
        assert!(capabilities.local_pty);
        assert!(capabilities.release_update_check);
    }
}
