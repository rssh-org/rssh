//! Native Tauri application adapter.

#[cfg(ohos)]
use crate::ohos;
#[cfg(any(windows, macos, linux))]
use rssh_core::db;
use rssh_core::{
    emitter::{Host, WindowHost},
    state::AppState,
};
use std::sync::Arc;
use tauri::{Emitter, Manager};

pub mod commands;

/// Attach the native event transport and window capabilities to the shared engine.
pub fn host(app: tauri::AppHandle) -> Host {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let events = app.clone();
    Host::new(
        state,
        Arc::new(move |event, payload| events.emit(event, payload).is_ok()),
    )
    .with_window_host(Arc::new(NativeWindows {
        #[cfg(any(windows, macos, linux, ohos))]
        app,
    }))
}

struct NativeWindows {
    #[cfg(any(windows, macos, linux, ohos))]
    app: tauri::AppHandle,
}

#[async_trait::async_trait]
impl WindowHost for NativeWindows {
    fn allows_diagnosis_downloads(&self) -> bool {
        crate::platform::runtime::allows_diagnosis_downloads()
    }

    async fn ensure_local_analysis_available(&self) -> Result<(), String> {
        #[cfg(any(windows, macos, linux, ohos))]
        {
            crate::platform::window::ensure_app_window_available(
                crate::platform::window::AppWindowPurpose::LocalAnalysis,
            )
            .await
        }
        #[cfg(any(android, ios))]
        Err("Additional windows are unavailable on this device".into())
    }

    async fn open_app_window(
        &self,
        label: &str,
        title: &str,
        init_script: &str,
    ) -> Result<(), String> {
        #[cfg(any(windows, macos, linux, ohos))]
        {
            crate::platform::window::open_app_window(
                &self.app,
                label,
                title,
                init_script,
                crate::platform::window::AppWindowPurpose::LocalAnalysis,
            )
            .await
        }
        #[cfg(any(android, ios))]
        {
            let _ = (label, title, init_script);
            Err("Additional windows are unavailable on this device".into())
        }
    }
}

// OHOS uses ArkWeb, not the Linux desktop WebKitGTK/Wayland backend.
#[cfg(linux)]
fn apply_linux_wayland_compat() {
    if std::env::var_os("RSSH_DISABLE_WAYLAND_COMPAT").is_some() {
        return;
    }

    let wayland_session = std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE")
            .map(|v| v.eq_ignore_ascii_case("wayland"))
            .unwrap_or(false);

    if !wayland_session {
        return;
    }

    // Wayland 兼容：部分 NVIDIA / wlroots 环境下，WebKitGTK 的 DMABUF renderer
    // 会在 Tauri 窗口创建前失败。Prefer reliable startup by default; users can
    // still override this variable explicitly.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    // GBM 后端兼容：全局导出的 GBM_BACKEND，尤其是 nvidia-drm，可能导致
    // Hyprland 下 GTK / WebKitGTK 报 "Failed to create GBM buffer"。Keep an
    // explicit opt-out for users whose stack needs this variable.
    if std::env::var_os("RSSH_KEEP_GBM_BACKEND").is_none() {
        std::env::remove_var("GBM_BACKEND");
    }
}

#[cfg(not(linux))]
fn apply_linux_wayland_compat() {}

#[cfg_attr(any(android, ios), tauri::mobile_entry_point)]
pub fn run() {
    apply_linux_wayland_compat();

    // 默认 info；用 RUST_LOG=debug 等覆盖
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    let builder = tauri::Builder::default();
    // Official plugins ship no OHOS backend (see the Cargo.toml target table
    // that gates them); every other target registers them exactly as before.
    #[cfg(any(macos, windows, linux, android, ios))]
    let builder = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init());
    builder
        // Android/iOS Activity recreation is not a logical window close.
        // The OHOS adapter emits Destroyed only for a real native window close.
        .on_window_event(|_window, event| {
            match event {
                // Mobile (Android/iOS): Activity lifecycle changes can fire
                // WindowEvent::Destroyed when the app merely goes to background
                // (e.g. a fullscreen file picker opens). Closing sessions there
                // would silently disconnect every SSH tab.
                #[cfg(any(windows, macos, linux, ohos))]
                tauri::WindowEvent::Destroyed => {
                    let runtime = tauri::async_runtime::handle();
                    let _guard = runtime.inner().enter();
                    let state = _window.state::<Arc<AppState>>();
                    // Close only sessions belonging to this window.
                    rssh_core::commands::lifecycle::close_window_sessions(&state, _window.label());
                }
                _ => {}
            }
        })
        .setup(|app| {
            // The UIAbility registers its sandbox path before Tauri starts.
            #[cfg(ohos)]
            let data_dir = ohos::data_dir()?;
            // Android/iOS use Tauri's sandbox path; OHOS registers its own above.
            #[cfg(any(android, ios))]
            let data_dir = app.path().app_data_dir()?;
            #[cfg(any(windows, macos, linux))]
            let data_dir = db::data_dir()?;

            // The native host owns asset access; the engine owns shared state.
            app.asset_protocol_scope()
                .allow_directory(data_dir.join("plugins"), true)?;
            app.manage(Arc::new(AppState::open(data_dir)?));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::runtime::get_runtime_capabilities,
            commands::files::resolve_local_paths,
            // profile & credential
            commands::profile::list_profiles,
            commands::profile::get_profile,
            commands::profile::create_profile,
            commands::profile::update_profile,
            commands::profile::delete_profile,
            commands::profile::ssh_algorithm_catalog,
            commands::profile::list_credentials,
            commands::profile::get_credential,
            commands::profile::create_credential,
            commands::profile::update_credential,
            commands::profile::delete_credential,
            commands::profile::read_default_key_file,
            // groups
            commands::group::list_groups,
            commands::group::create_group,
            commands::group::update_group,
            commands::group::delete_group,
            // forward CRUD
            commands::forward::list_forwards,
            commands::forward::get_forward,
            commands::forward::create_forward,
            commands::forward::update_forward,
            commands::forward::delete_forward,
            // forward active
            commands::forward::forward_start,
            commands::forward::forward_stats,
            commands::forward::forward_rule_start,
            commands::forward::forward_rule_stop,
            commands::forward::forward_stop,
            // dynamic discovery
            commands::discovery::list_dynamic_discovery_sources,
            commands::discovery::save_dynamic_discovery_sources,
            commands::discovery::dynamic_discovery_tool_status,
            commands::discovery::list_dynamic_discovery_contexts,
            commands::discovery::discover_dynamic_targets,
            // settings & snippets & highlights
            commands::settings::get_setting,
            commands::settings::set_setting,
            commands::settings::list_highlights,
            commands::settings::add_highlight,
            commands::settings::remove_highlight,
            commands::settings::update_highlight,
            commands::settings::load_snippets,
            commands::settings::save_snippets,
            commands::settings::reset_highlights,
            commands::settings::list_recordings,
            commands::settings::read_recording,
            commands::settings::secret_backend,
            commands::settings::list_fonts,
            // plugins
            commands::plugin::plugins_root,
            commands::plugin::install_plugin,
            commands::plugin::list_plugins,
            commands::plugin::set_plugin_enabled,
            commands::plugin::set_plugin_order,
            commands::plugin::uninstall_plugin,
            commands::plugin::plugin_exec,
            commands::command_block::command_block_list_redact_rules,
            commands::command_block::command_block_save_redact_rule,
            commands::command_block::command_block_delete_redact_rule,
            // SSH session
            commands::session::ssh_connect,
            commands::session::ssh_write,
            commands::session::ssh_resize,
            commands::session::ssh_disconnect,
            commands::session::ssh_auth_respond,
            commands::session::ssh_auth_cancel,
            commands::session::ssh_passphrase_respond,
            commands::session::ssh_passphrase_cancel,
            commands::session::ssh_host_key_respond,
            commands::session::ssh_host_key_cancel,
            // session lifecycle
            commands::lifecycle::reconcile_sessions,
            // PTY (desktop hosts and capability-probed HarmonyOS PC)
            #[cfg(any(windows, macos, linux, ohos))]
            commands::pty::list_shells,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::pty::refresh_shells,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::pty::pty_spawn,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::pty::pty_spawn_connector,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::pty::pty_write,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::pty::pty_resize,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::pty::pty_close,
            // Serial (desktop hosts and capability-probed HarmonyOS PC)
            #[cfg(any(windows, macos, linux, ohos))]
            commands::serial::serial_get_capabilities,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::serial::serial_list_ports,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::serial::serial_open,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::serial::serial_write,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::serial::serial_close,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::serial::serial_set_dtr,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::serial::serial_set_rts,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::serial::serial_send_break,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::serial::list_serial_profiles,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::serial::get_serial_profile,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::serial::create_serial_profile,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::serial::update_serial_profile,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::serial::delete_serial_profile,
            // Telnet (all platforms — plain TCP)
            commands::telnet::telnet_open,
            commands::telnet::telnet_write,
            commands::telnet::telnet_write_line,
            commands::telnet::telnet_resize,
            commands::telnet::telnet_close,
            commands::telnet::list_telnet_profiles,
            commands::telnet::get_telnet_profile,
            commands::telnet::create_telnet_profile,
            commands::telnet::update_telnet_profile,
            commands::telnet::delete_telnet_profile,
            // SFTP
            commands::sftp::sftp_connect,
            commands::sftp::sftp_connect_session,
            commands::sftp::sftp_home,
            commands::sftp::sftp_list,
            commands::sftp::sftp_walk_remote_dir,
            commands::sftp::walk_local_dir,
            commands::sftp::sftp_download,
            commands::sftp::sftp_upload,
            commands::sftp::sftp_mkdir,
            commands::sftp::sftp_close,
            // Stream transfer to/from a path (desktop) or content:// URI (mobile).
            commands::sftp::sftp_download_to,
            commands::sftp::sftp_upload_from,
            commands::sftp::sftp_pick_save_path,
            commands::sftp::sftp_pick_open_path,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::sftp::sftp_pick_folder,
            commands::sftp::sftp_pick_open_files,
            commands::sftp::sftp_cancel_transfer,
            commands::files::save_text_file,
            commands::sftp::sftp_remove,
            commands::sftp::sftp_rename,
            commands::sftp::sftp_stat,
            // CLI install
            #[cfg(any(windows, macos, linux))]
            commands::cli::cli_status,
            #[cfg(any(windows, macos, linux))]
            commands::cli::cli_install,
            // Native multi-window hosts
            #[cfg(any(windows, macos, linux, ohos))]
            commands::window::open_tab_in_new_window,
            #[cfg(any(windows, macos, linux, ohos))]
            commands::window::set_app_window_title,
            commands::clipboard::clipboard_read,
            commands::clipboard::clipboard_write,
            // external URL opener — cross-platform via tauri-plugin-opener
            commands::external::open_external_url,
            // update check (cross-platform — separate mod from window)
            commands::update::fetch_latest_release_tag,
            // sync
            commands::sync::export_config,
            commands::sync::import_config,
            commands::sync::github_push,
            commands::sync::github_pull,
            commands::sync::webdav_push,
            commands::sync::webdav_pull,
            commands::sync::sync_check,
            commands::sync::sync_refresh_local_metadata,
            commands::sync::sync_check_remotes,
            commands::sync::get_sync_auto_pull_status,
            commands::sync::set_sync_auto_pull,
            // AI 排障
            commands::ai::ai_list_skills,
            commands::ai::ai_get_skill,
            commands::ai::ai_save_skill,
            commands::ai::ai_delete_skill,
            commands::ai::ai_list_redact_rules,
            commands::ai::ai_save_redact_rule,
            commands::ai::ai_delete_redact_rule,
            commands::ai::ai_list_command_blacklist,
            commands::ai::ai_replace_command_blacklist,
            commands::ai::ai_session_start,
            commands::ai::ai_session_prepare_stop,
            commands::ai::ai_session_stop,
            commands::ai::ai_session_clear_context,
            commands::ai::ai_session_rollback_context,
            commands::ai::ai_session_rebind_target,
            commands::ai::ai_remote_shell_probe_needed,
            commands::ai::ai_cache_remote_shell,
            commands::ai::ai_cancel_stream,
            commands::ai::ai_user_message,
            commands::ai::ai_command_result,
            commands::ai::ai_command_reject,
            commands::ai::ai_audit_save,
            commands::ai::ai_audit_log_text,
            commands::ai::ai_audit_get,
            commands::ai::ai_list_sessions,
            commands::ai::ai_conversations_list,
            commands::ai::ai_conversation_timeline,
            commands::ai::ai_conversation_save_timeline,
            commands::ai::ai_conversation_delete,
            commands::ai::ai_settings_get,
            commands::ai::ai_settings_set,
            commands::ai::ai_list_models,
            commands::ai::ai_provider_list,
            commands::ai::ai_provider_save,
            commands::ai::ai_provider_delete,
            commands::ai::ai_codex_auth_status,
            commands::ai::ai_codex_login_start,
            commands::ai::ai_codex_login_poll,
            commands::ai::ai_codex_login_cancel,
            commands::ai::ai_codex_logout,
        ])
        .run(tauri::generate_context!())
        .expect("RSSH startup failed");
}
