use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

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
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("index.html".into()))
        .title("RSSH")
        .inner_size(1200.0, 800.0)
        .initialization_script(&init_script)
        .build()
        .map_err(|e| {
            AppError::other(
                "window_open_failed",
                serde_json::json!({ "err": e.to_string() }),
            )
        })?;
    Ok(())
}

#[cfg(all(test, desktop))]
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

/// One `arboard::Clipboard` for the whole process, created lazily.
///
/// On X11 the clipboard is a *selection ownership* protocol, not a store: the
/// process that wrote the text must stay alive to serve other apps' (and our
/// own paste's) `SelectionRequest`s. arboard owns the CLIPBOARD selection only
/// while at least one `Clipboard` instance is alive; the last one to drop tears
/// down its X11 window and hands the data off to a clipboard manager on a
/// best-effort basis — a race it usually loses ("Clipboard was dropped very
/// quickly after writing"). Creating a fresh `Clipboard` per call therefore
/// relinquished the selection the instant the call returned, so the next paste
/// read an empty clipboard.
///
/// Keeping one instance alive for the process lifetime means we stay the
/// selection owner: reads short-circuit to local data and external pastes are
/// served, with no per-call teardown/handoff race. `Clipboard` is `Send + Sync`
/// on every desktop platform, so a `static` behind a `Mutex` is sound.
static CLIPBOARD: std::sync::OnceLock<std::sync::Mutex<Option<arboard::Clipboard>>> =
    std::sync::OnceLock::new();

/// Run `op` against the process-wide clipboard, creating it on first use.
fn with_clipboard<R>(
    op: &'static str,
    f: impl FnOnce(&mut arboard::Clipboard) -> Result<R, arboard::Error>,
) -> AppResult<R> {
    let cell = CLIPBOARD.get_or_init(|| std::sync::Mutex::new(None));
    // A panic while holding the lock can't leave the clipboard in an unsafe
    // state, so recover from poisoning rather than failing the operation.
    let mut guard = cell.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(arboard::Clipboard::new().map_err(|e| {
            AppError::other(
                "window_clipboard_failed",
                serde_json::json!({ "op": "init", "err": e.to_string() }),
            )
        })?);
    }
    let cb = guard.as_mut().expect("clipboard initialized above");
    f(cb).map_err(|e| {
        AppError::other(
            "window_clipboard_failed",
            serde_json::json!({ "op": op, "err": e.to_string() }),
        )
    })
}

/// Read the system clipboard as text.
/// Goes through Rust (arboard) to bypass WebKit's permission prompt on
/// externally-sourced clipboard content — `navigator.clipboard.readText()`
/// pops a dialog every time on macOS unless the content was written by the
/// same page in this session.
#[tauri::command]
pub fn clipboard_read() -> AppResult<String> {
    with_clipboard("read", |cb| cb.get_text())
}

/// Write text to the system clipboard.
/// Mirrors `clipboard_read`: goes through Rust (arboard) because in the
/// WKWebView `navigator.clipboard.writeText` is unreliable from a right-click
/// (contextmenu) / unfocused context — it silently rejects.
#[tauri::command]
pub fn clipboard_write(text: String) -> AppResult<()> {
    with_clipboard("write", |cb| cb.set_text(text))
}
