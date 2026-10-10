use super::AppWindowPurpose;

pub(super) async fn ensure_available(_purpose: AppWindowPurpose) -> Result<(), String> {
    Ok(())
}

pub(super) async fn confirm_creation(_window: tauri::WebviewWindow) -> Result<(), String> {
    // These backends finish native creation before build() returns.
    Ok(())
}
