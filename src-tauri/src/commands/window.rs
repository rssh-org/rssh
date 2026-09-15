use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use uuid::Uuid;

use serde_json::json;

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
        .additional_browser_args("--disable-gpu")
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

/// Write a text to the system clipboard.
/// Mirrors `clipboard_read`: goes through Rust (arboard) because in the
/// WKWebView `navigator.clipboard.writeText` is unreliable from a right-click
/// (contextmenu) / unfocused context — it silently rejects.
#[tauri::command]
pub fn clipboard_write(text: String) -> AppResult<()> {
    with_clipboard("write", |cb| cb.set_text(text))
}

/// Append a frontend-side error (window.onerror / unhandledrejection / view
/// init failure) to `<app-data>/fe-errors.log`. When the UI freezes or a file
/// editor won't open, this log is how we learn what actually happened in the
/// WebView — the browser console is not reachable from a release build.
#[tauri::command]
pub fn log_frontend_error(app: AppHandle, msg: String) -> AppResult<()> {
    use std::io::Write;
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::other("app_data_dir_failed", json!({ "err": e.to_string() })))?;
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join("fe-errors.log");
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("[{secs}] {msg}\n");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
    Ok(())
}

// ---- UI heartbeat watchdog -------------------------------------------------
// The frontend calls `frontend_heartbeat` every 2s. If the renderer thread
// hangs (e.g. a Svelte effect runaway like `effect_update_depth_exceeded`, or
// a WebView2 GPU stall), the heartbeat stops; after UI_WATCHDOG_TIMEOUT we
// reload the main window instead of leaving the user with a frozen window
// that can only be killed from Task Manager. The frontend keeps its own
// reload-safe watchdog too; this Rust-side one works even if the JS main
// thread is completely stuck.
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static LAST_HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[tauri::command]
pub fn frontend_heartbeat() {
    LAST_HEARTBEAT_MS.store(now_ms(), Ordering::Relaxed);
}

/// Spawn a background task that reloads the main window when the frontend
/// heartbeat stops for longer than `timeout`. Also guards new editor windows
/// (`rssh-*` labels). Call once from setup.
pub fn spawn_ui_watchdog(app: &AppHandle, timeout: Duration) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let last = LAST_HEARTBEAT_MS.load(Ordering::Relaxed);
            if last == 0 {
                continue; // frontend not booted yet
            }
            let elapsed = now_ms().saturating_sub(last);
            if elapsed > timeout.as_millis() as u64 {
                // Reset first so a crashed reload loop does not fire forever.
                LAST_HEARTBEAT_MS.store(now_ms(), Ordering::Relaxed);
                log::warn!("UI heartbeat stopped for {elapsed} ms — reloading windows");
                for label in handle
                    .webview_windows()
                    .keys()
                    .filter(|l| l.as_str() == "main" || l.starts_with("rssh-"))
                    .cloned()
                    .collect::<Vec<_>>()
                {
                    if let Some(win) = handle.get_webview_window(&label) {
                        let _ = win.eval("location.reload()");
                    }
                }
            }
        }
    });
}
