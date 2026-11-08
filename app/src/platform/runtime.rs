#[cfg(feature = "tauri")]
use crate::error::AppResult;

/// Host features and input affordances, independent of frontend layout width.
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
    pub system_back: bool,
    pub status_bar_hidden: bool,
    pub mcp_server: bool,
    pub terminal_keybar: bool,
    pub terminal_policy: TerminalRuntimePolicy,
}

impl RuntimeCapabilities {
    pub fn native() -> Self {
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
            system_back: cfg!(android),
            status_bar_hidden: cfg!(any(android, ios)),
            mcp_server: cfg!(any(windows, macos, linux)),
            terminal_keybar: cfg!(any(android, ios)),
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
            system_back: false,
            status_bar_hidden: false,
            // The browser adapter supplies its client's keyboard affordance.
            terminal_keybar: false,
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
    pub fn desktop() -> Self {
        Self {
            image_storage_limit_mb: 128,
            image_pixel_limit: 16_000_000,
            output_backlog_limit_bytes: 128 * 1024 * 1024,
            gpu_render_default: false,
        }
    }

    pub fn constrained() -> Self {
        Self {
            image_storage_limit_mb: 32,
            image_pixel_limit: 4_000_000,
            output_backlog_limit_bytes: 32 * 1024 * 1024,
            gpu_render_default: false,
        }
    }
}

#[cfg(feature = "tauri")]
pub async fn capabilities(window: &tauri::Window) -> AppResult<RuntimeCapabilities> {
    use tauri::Manager;
    super::host::platform(window.app_handle())
        .capabilities(window.label())
        .await
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
            "systemBack",
            "statusBarHidden",
            "mcpServer",
            "terminalKeybar",
        ] {
            assert!(value[key].is_boolean(), "missing boolean capability {key}");
        }
        assert_eq!(value.as_object().unwrap().len(), 19);
        assert_eq!(value["fileMultiSelect"], true);
        assert_eq!(value["releaseUpdateCheck"], !cfg!(ios));
        assert_eq!(value["systemBack"], cfg!(android));
        assert_eq!(value["statusBarHidden"], cfg!(any(android, ios)));
        assert_eq!(value["terminalKeybar"], cfg!(any(android, ios)));
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
        assert!(!capabilities.system_back);
        assert!(!capabilities.status_bar_hidden);
        assert!(!capabilities.terminal_keybar);
        assert!(capabilities.local_pty);
        assert!(capabilities.release_update_check);
    }
}
