//! Persistence backend for the in-memory localStorage polyfill.
//!
//! On OHOS, ArkWeb serves the app under the custom `tauri://localhost` scheme,
//! where `window.localStorage` is null. index.html installs an in-memory
//! polyfill whose contents round-trip through these commands into a single
//! JSON blob at `<data_dir>/ui_state.json`. Every other platform has a real
//! localStorage and never invokes these.

use std::path::Path;

use tauri::State;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

const UI_STATE_FILE: &str = "ui_state.json";

/// Read the persisted blob. `None` = never saved (first boot). A corrupt or
/// unreadable file is treated as "never saved" — UI state is disposable, a
/// hard failure here would brick startup over cosmetics.
fn load_from(data_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(data_dir.join(UI_STATE_FILE)).ok()?;
    // Sanity: it must parse as a JSON object, otherwise the frontend Map fill
    // would choke. `raw` comes from our own save; this guards manual edits.
    serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    Some(raw)
}

/// Atomic write: tmp file + rename, so a crash mid-save can never leave a
/// truncated blob behind.
fn save_to(data_dir: &Path, state: &str) -> AppResult<()> {
    let final_path = data_dir.join(UI_STATE_FILE);
    let tmp_path = data_dir.join(format!("{UI_STATE_FILE}.tmp"));
    std::fs::write(&tmp_path, state).map_err(|e| {
        AppError::other(
            "ui_state_write_failed",
            serde_json::json!({ "err": e.to_string() }),
        )
    })?;
    std::fs::rename(&tmp_path, &final_path).map_err(|e| {
        AppError::other(
            "ui_state_rename_failed",
            serde_json::json!({ "err": e.to_string() }),
        )
    })?;
    Ok(())
}

#[tauri::command]
pub fn ui_state_load(state: State<AppState>) -> AppResult<Option<String>> {
    Ok(load_from(&state.data_dir))
}

#[tauri::command]
pub fn ui_state_save(state: State<AppState>, ui_state: String) -> AppResult<()> {
    // Reject non-object payloads early: the polyfill serializes a Map of
    // string->string, so anything else is a bug on the caller side.
    let parsed: serde_json::Value = serde_json::from_str(&ui_state).map_err(|e| {
        AppError::other(
            "ui_state_invalid_json",
            serde_json::json!({ "err": e.to_string() }),
        )
    })?;
    if !parsed.is_object() {
        return Err(AppError::other(
            "ui_state_not_an_object",
            serde_json::json!({}),
        ));
    }
    save_to(&state.data_dir, &ui_state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_blob() {
        let dir = tempfile::tempdir().unwrap();
        let blob = r#"{"theme":"dark","panel":"open"}"#;
        save_to(dir.path(), blob).unwrap();
        assert_eq!(load_from(dir.path()).as_deref(), Some(blob));
    }

    #[test]
    fn load_returns_none_when_never_saved() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_from(dir.path()), None);
    }

    #[test]
    fn corrupt_file_reads_as_never_saved() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(UI_STATE_FILE), "not json{").unwrap();
        assert_eq!(load_from(dir.path()), None);
    }

    #[test]
    fn save_leaves_no_tmp_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        save_to(dir.path(), "{}").unwrap();
        assert!(!dir.path().join(format!("{UI_STATE_FILE}.tmp")).exists());
        assert!(dir.path().join(UI_STATE_FILE).exists());
    }

    #[test]
    fn save_overwrites_previous_state() {
        let dir = tempfile::tempdir().unwrap();
        save_to(dir.path(), r#"{"a":"1"}"#).unwrap();
        save_to(dir.path(), r#"{"b":"2"}"#).unwrap();
        assert_eq!(load_from(dir.path()).as_deref(), Some(r#"{"b":"2"}"#));
    }
}
