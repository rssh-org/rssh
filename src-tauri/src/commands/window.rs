use tauri::AppHandle;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::platform::window::{open_app_window, AppWindowPurpose};

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
