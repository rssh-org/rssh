use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{AppHandle, Emitter, Manager, Wry};

struct NativeNavigation {
    handle: PluginHandle<Wry>,
    on_back: Channel<BackEvent>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackEvent {
    window_label: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BackNavigationArgs<'a> {
    window_label: &'a str,
    enabled: bool,
    revision: u64,
    on_back: &'a Channel<BackEvent>,
}

pub fn init() -> TauriPlugin<Wry> {
    Builder::new("native-navigation")
        .setup(|app, api| {
            let handle = api.register_android_plugin("com.rssh.app", "NativeNavigationPlugin")?;
            let app_handle = app.clone();
            let on_back = Channel::new(move |message| {
                let event: BackEvent = message.deserialize()?;
                app_handle
                    .emit_to(
                        event.window_label.clone(),
                        &format!("navigation:back:{}", event.window_label),
                        (),
                    )
                    .inspect_err(|error| {
                        log::error!("Failed to deliver native back event: {error}")
                    })
            });
            app.manage(NativeNavigation { handle, on_back });
            Ok(())
        })
        .build()
}

pub async fn set_enabled(
    app: &AppHandle,
    window_label: &str,
    enabled: bool,
    revision: u64,
) -> AppResult<()> {
    let Some(navigation) = app.try_state::<NativeNavigation>() else {
        // The first page load can precede plugin setup; callbacks start disabled.
        if !enabled {
            return Ok(());
        }
        return Err(AppError::other(
            "navigation_update_failed",
            serde_json::json!({ "err": "Native navigation is not initialized" }),
        ));
    };
    navigation
        .handle
        .run_mobile_plugin_async(
            "setEnabled",
            BackNavigationArgs {
                window_label,
                enabled,
                revision,
                on_back: &navigation.on_back,
            },
        )
        .await
        .map_err(|error| {
            AppError::other(
                "navigation_update_failed",
                serde_json::json!({ "err": error.to_string() }),
            )
        })
}
