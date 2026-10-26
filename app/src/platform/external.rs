//! External-link policy is shared; opening mechanics belong to the host.
use crate::error::{AppError, AppResult};

#[cfg(ohos)]
use crate::ohos::external as backend;
#[cfg(not(ohos))]
#[path = "tauri/external.rs"]
mod backend;

pub async fn open_url(app: &tauri::AppHandle, url: String) -> AppResult<()> {
    validate_url(&url)?;
    backend::open_url(app, url).await
}

fn validate_url(url: &str) -> AppResult<()> {
    // Preserve the existing http(s)-only command contract on every host.
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(AppError::config(
            "window_non_https_url",
            serde_json::json!({"url": url}),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_links_reach_a_platform_opener() {
        for url in ["https://example.com", "http://localhost:8000/path"] {
            assert!(validate_url(url).is_ok());
        }
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "mailto:a@example.com",
            "",
        ] {
            assert_eq!(
                validate_url(url).unwrap_err().code(),
                "window_non_https_url"
            );
        }
    }
}
