#[cfg(not(target_env = "ohos"))]
use tauri::AppHandle;

use crate::error::{AppError, AppResult};

/// Open an external http(s) URL in the user's default browser/app.
///
/// Goes through `tauri-plugin-opener` so the same call works on desktop,
/// Android and iOS — on mobile the old `Command::new("open" | "xdg-open" | "cmd")`
/// route had no implementation and the invoke silently failed for users.
///
/// Refuses non-http(s) schemes to prevent abuse (file://, javascript:, …).
#[cfg(any(desktop, all(mobile, not(target_env = "ohos"))))]
#[tauri::command]
pub fn open_external_url(app: AppHandle, url: String) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;

    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(AppError::config(
            "window_non_https_url",
            serde_json::json!({ "url": url }),
        ));
    }
    app.opener().open_url(&url, None::<&str>).map_err(|e| {
        AppError::other(
            "window_open_url_failed",
            serde_json::json!({ "err": e.to_string() }),
        )
    })
}

/// HarmonyOS invokes UIAbilityContext.openLink on its owning UI thread.
#[cfg(target_env = "ohos")]
#[tauri::command]
pub async fn open_external_url(url: String) -> AppResult<()> {
    let parsed = url::Url::parse(&url)
        .map_err(|_| AppError::config("window_non_https_url", serde_json::json!({ "url": url })))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(AppError::config(
            "window_non_https_url",
            serde_json::json!({ "url": url }),
        ));
    }
    use openharmony_ability_plugin_url::UrlExt;
    crate::ohos::app()?
        .open_url(url)
        .await
        .map_err(crate::ohos::native_error)
}
