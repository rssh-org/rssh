use super::AppWindowPurpose;

pub(super) fn native_title(requested: &str) -> &str {
    requested
}

pub(super) async fn ensure_available(_purpose: AppWindowPurpose) -> Result<(), String> {
    Ok(())
}

pub(super) async fn confirm_creation(_window: tauri::WebviewWindow) -> Result<(), String> {
    // These backends finish native creation before build() returns.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_title_preserves_desktop_window_content() {
        for requested in [
            "首页",
            "Settings",
            "user@host:~",
            "RSSH — Local Analysis",
            "",
        ] {
            assert_eq!(native_title(requested), requested);
        }
    }
}
