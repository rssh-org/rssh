use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

#[derive(Clone, Copy)]
pub(crate) enum AppWindowPurpose {
    Tab,
    LocalAnalysis,
}

#[cfg(any(test, ohos))]
fn check_window_capabilities(
    desktop_device: bool,
    multi_window: bool,
    local_pty: bool,
    purpose: AppWindowPurpose,
) -> Result<(), String> {
    if !desktop_device || !multi_window {
        return Err("Additional windows are unavailable on this device".into());
    }
    if matches!(purpose, AppWindowPurpose::LocalAnalysis) && !local_pty {
        return Err("Local analysis requires a working local terminal".into());
    }
    Ok(())
}

pub(crate) async fn ensure_app_window_available(purpose: AppWindowPurpose) -> Result<(), String> {
    #[cfg(ohos)]
    {
        let device = crate::ohos::device::query().await.map_err(|error| {
            log::warn!("Cannot query native window capabilities: {error}");
            "HarmonyOS system services are unavailable; restart RSSH and try again".to_string()
        })?;
        let desktop_device = device.class() == crate::ohos::device::DeviceClass::Desktop;
        let local_pty = desktop_device
            && device.multi_window
            && matches!(purpose, AppWindowPurpose::LocalAnalysis)
            && crate::ohos::device::local_pty_available().await;
        check_window_capabilities(desktop_device, device.multi_window, local_pty, purpose)
    }
    #[cfg(not(ohos))]
    {
        let _ = purpose;
        Ok(())
    }
}

/// Both IPC tab cloning and AI handoff use this boundary. The window's unique
/// label becomes its session owner when the frontend creates its resources.
pub(crate) async fn open_app_window(
    app: &AppHandle,
    label: &str,
    title: &str,
    init_script: &str,
    purpose: AppWindowPurpose,
) -> Result<(), String> {
    ensure_app_window_available(purpose).await?;
    WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(title)
        .inner_size(1200.0, 800.0)
        .initialization_script(init_script)
        .build()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// Encode the frontend clone payload as a JavaScript string literal.
///
/// The frontend reads `window.__rssh_clone` as a string and parses it once
/// during startup, so the JSON payload must remain quoted in the script.
fn clone_init_script(clone: &str) -> AppResult<String> {
    let json_literal = serde_json::to_string(clone).map_err(|e| {
        AppError::other(
            "window_clone_encode_failed",
            serde_json::json!({ "err": e.to_string() }),
        )
    })?;
    Ok(format!("window.__rssh_clone = {};", json_literal))
}

/// Open a new in-process Tauri window with a clone payload.
///
/// The new window boots the same frontend; `AppShell` reads
/// `window.__rssh_clone` on mount and auto-creates the cloned tab. Windows
/// share `AppState` (sessions, DB, PTY registry) via `Arc<Mutex<..>>`, so
/// spawning a new window is cheap and does not fork the backend.
///
/// MUST stay `async`: on Windows, `WebviewWindowBuilder::build()` deadlocks
/// when called from a synchronous command because WebView2 needs the main
/// thread's message loop to create the webview controller. Async commands run
/// off the main event-loop thread, so the build completes.
#[tauri::command]
pub async fn open_tab_in_new_window(app: AppHandle, clone: String) -> AppResult<()> {
    let init_script = clone_init_script(&clone)?;
    let label = format!("rssh-{}", Uuid::new_v4().simple());
    open_app_window(&app, &label, "RSSH", &init_script, AppWindowPurpose::Tab)
        .await
        .map_err(|e| {
            AppError::other(
                "window_open_failed",
                serde_json::json!({ "err": e.to_string() }),
            )
        })?;
    Ok(())
}

#[cfg(all(test, any(windows, macos, linux)))]
mod tests {
    use super::*;

    #[test]
    fn phone_tablet_and_unavailable_window_service_reject_both_entry_points() {
        for (desktop, multi_window) in [(false, false), (false, true), (true, false)] {
            for purpose in [AppWindowPurpose::Tab, AppWindowPurpose::LocalAnalysis] {
                assert_eq!(
                    check_window_capabilities(desktop, multi_window, true, purpose),
                    Err("Additional windows are unavailable on this device".into()),
                );
            }
        }
    }

    #[test]
    fn pty_restrictions_block_analysis_but_not_remote_tab_windows() {
        assert!(check_window_capabilities(true, true, false, AppWindowPurpose::Tab).is_ok());
        assert_eq!(
            check_window_capabilities(true, true, false, AppWindowPurpose::LocalAnalysis),
            Err("Local analysis requires a working local terminal".into()),
        );
        assert!(
            check_window_capabilities(true, true, true, AppWindowPurpose::LocalAnalysis).is_ok()
        );
    }

    #[test]
    fn clone_payload_becomes_quoted_init_script() {
        let clone = r#"{"type":"ssh","label":"prod"}"#;
        let script = clone_init_script(clone).unwrap();
        assert_eq!(
            script,
            r#"window.__rssh_clone = "{\"type\":\"ssh\",\"label\":\"prod\"}";"#
        );
    }

    #[test]
    fn clone_payload_escapes_script_sensitive_text() {
        let clone = "{\"label\":\"line\nquote \\\"\"}";
        let script = clone_init_script(clone).unwrap();
        let literal = script
            .strip_prefix("window.__rssh_clone = ")
            .and_then(|value| value.strip_suffix(';'))
            .unwrap();
        assert_eq!(serde_json::from_str::<String>(literal).unwrap(), clone);
    }
}
