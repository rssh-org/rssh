use crate::error::{AppError, AppResult};

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
pub fn read_text() -> AppResult<String> {
    with_clipboard("read", |cb| cb.get_text())
}

/// Write text to the system clipboard.
/// Mirrors `clipboard_read`: goes through Rust (arboard) because in the
/// WKWebView `navigator.clipboard.writeText` is unreliable from a right-click
/// (contextmenu) / unfocused context — it silently rejects.
pub fn write_text(text: String) -> AppResult<()> {
    with_clipboard("write", |cb| cb.set_text(text))
}
