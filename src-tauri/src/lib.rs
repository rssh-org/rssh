mod ai;
mod commands;
#[cfg(any(target_env = "ohos", test))]
mod ohos;
#[cfg(desktop)]
pub use commands::cli::CLI_VERSION;
pub mod crypto;
pub mod db;
pub mod emitter;
pub mod error;
pub mod migration;
pub mod models;
mod redaction;
pub mod secret;
mod ssh;
pub use ssh::bastion;
#[cfg(all(feature = "server", desktop))]
pub mod server;
mod state;
pub mod sync;
mod telnet_profile;
mod terminal;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tauri::Manager;

use state::AppState;

#[cfg(target_os = "linux")]
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

#[cfg(not(target_os = "linux"))]
fn apply_linux_wayland_compat() {}

#[cfg_attr(all(mobile, not(target_env = "ohos")), tauri::mobile_entry_point)]
pub fn run() {
    apply_linux_wayland_compat();

    // 默认 info；用 RUST_LOG=debug 等覆盖
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .try_init();

    let builder = tauri::Builder::default();
    // Official plugins ship no OHOS backend (see the Cargo.toml target table
    // that gates them); every other target registers them exactly as before.
    #[cfg(any(
        target_os = "macos",
        target_os = "windows",
        all(target_os = "linux", not(target_env = "ohos")),
        target_os = "android",
        target_os = "ios"
    ))]
    let builder = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_clipboard_manager::init());
    builder
        // Android/iOS Activity recreation is not a logical window close.
        // The OHOS adapter emits Destroyed only for a real native window close.
        .on_window_event(|_window, event| {
            match event {
                // Mobile (Android/iOS): Activity lifecycle changes can fire
                // WindowEvent::Destroyed when the app merely goes to background
                // (e.g. a fullscreen file picker opens). Closing sessions there
                // would silently disconnect every SSH tab.
                #[cfg(any(desktop, target_env = "ohos"))]
                tauri::WindowEvent::Destroyed => {
                    let state = _window.state::<AppState>();
                    // Close only sessions belonging to this window.
                    commands::lifecycle::close_window_sessions(&state, _window.label());
                }
                _ => {}
            }
        })
        .setup(|app| {
            // The UIAbility registers its sandbox path before Tauri starts.
            #[cfg(target_env = "ohos")]
            let data_dir = ohos::data_dir()?;
            // fork defines mobile = ios|android|ohos, desktop = !mobile
            #[cfg(all(mobile, not(target_env = "ohos")))]
            let data_dir = app.path().app_data_dir()?;
            #[cfg(desktop)]
            let data_dir = db::data_dir()?;

            // 启动时扫一次本机可用 shell，结果缓存到进程退出。
            // 用户在 Shell 设置页打开时直接读缓存，没冷启动开销。
            // HarmonyOS also compiles PTY, but probes sandbox support before use.
            #[cfg(any(desktop, target_env = "ohos"))]
            terminal::pty::init_available_shells();
            let db = Arc::new(db::Db::open(&data_dir)?);
            // The plugin store follows the actual host data directory (including
            // application sandboxes), rather than a desktop-only path template.
            app.asset_protocol_scope()
                .allow_directory(data_dir.join("plugins"), true)?;
            // secret::open 可能失败：sticky backend 标记 keyring 但 keychain 现在
            // 拿不到（系统 keychain 损坏 / D-Bus 挂等）→ 硬 fail 启动。silently
            // fallback file 会用新主密钥让旧密文全部解不开，比启动失败更危险。
            let secret_system = secret::open(db.clone(), &data_dir)?;

            // 启动迁移。失败不阻塞启动（log warn，下次启动重试），跟原
            // passphrase 清理逻辑的"软失败"风格一致。
            if let Err(e) = migration::run_migrations(
                &db,
                secret_system.raw_keyring.as_deref(),
                secret_system.store.as_ref(),
            ) {
                log::warn!("migration failed (will retry on next startup): {e}");
            }

            app.manage(AppState {
                db,
                secret_store: secret_system.store,
                lifecycle_sessions: Mutex::new(HashMap::new()),
                sessions: Mutex::new(HashMap::new()),
                #[cfg(any(desktop, target_env = "ohos"))]
                pty_sessions: Mutex::new(HashMap::new()),
                #[cfg(any(desktop, target_env = "ohos"))]
                serial_sessions: Mutex::new(HashMap::new()),
                telnet_sessions: Mutex::new(HashMap::new()),
                sftp_sessions: Mutex::new(HashMap::new()),
                transfer_cancels: Mutex::new(HashMap::new()),
                active_forwards: Mutex::new(HashMap::new()),
                auth_waiters: Mutex::new(HashMap::new()),
                passphrase_waiters: Mutex::new(HashMap::new()),
                host_key_waiters: Mutex::new(HashMap::new()),
                passphrase_cache: Mutex::new(HashMap::new()),
                ai_sessions: Mutex::new(HashMap::new()),
                ai_session_owners: Arc::new(Mutex::new(HashMap::new())),
                ai_remote_shell_cache: Mutex::new(HashMap::new()),
                data_dir,
            });
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
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::pty::list_shells,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::pty::refresh_shells,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::pty::pty_spawn,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::pty::pty_spawn_connector,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::pty::pty_write,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::pty::pty_resize,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::pty::pty_close,
            // Serial (desktop hosts and capability-probed HarmonyOS PC)
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::serial::serial_get_capabilities,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::serial::serial_list_ports,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::serial::serial_open,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::serial::serial_write,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::serial::serial_close,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::serial::serial_set_dtr,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::serial::serial_set_rts,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::serial::serial_send_break,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::serial::list_serial_profiles,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::serial::get_serial_profile,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::serial::create_serial_profile,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::serial::update_serial_profile,
            #[cfg(any(desktop, target_env = "ohos"))]
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
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::sftp::sftp_pick_folder,
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::sftp::sftp_pick_open_files,
            commands::sftp::sftp_cancel_transfer,
            commands::files::save_text_file,
            commands::sftp::sftp_remove,
            commands::sftp::sftp_rename,
            commands::sftp::sftp_stat,
            // CLI install
            #[cfg(desktop)]
            commands::cli::cli_status,
            #[cfg(desktop)]
            commands::cli::cli_install,
            // Native multi-window hosts
            #[cfg(any(desktop, target_env = "ohos"))]
            commands::window::open_tab_in_new_window,
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
            ai::commands::ai_list_skills,
            ai::commands::ai_get_skill,
            ai::commands::ai_save_skill,
            ai::commands::ai_delete_skill,
            ai::commands::ai_list_redact_rules,
            ai::commands::ai_save_redact_rule,
            ai::commands::ai_delete_redact_rule,
            ai::commands::ai_list_command_blacklist,
            ai::commands::ai_replace_command_blacklist,
            ai::commands::ai_session_start,
            ai::commands::ai_session_prepare_stop,
            ai::commands::ai_session_stop,
            ai::commands::ai_session_clear_context,
            ai::commands::ai_session_rollback_context,
            ai::commands::ai_session_rebind_target,
            ai::commands::ai_remote_shell_probe_needed,
            ai::commands::ai_cache_remote_shell,
            ai::commands::ai_cancel_stream,
            ai::commands::ai_user_message,
            ai::commands::ai_command_result,
            ai::commands::ai_command_reject,
            ai::commands::ai_audit_save,
            ai::commands::ai_audit_log_text,
            ai::commands::ai_audit_get,
            ai::commands::ai_list_sessions,
            ai::commands::ai_conversations_list,
            ai::commands::ai_conversation_timeline,
            ai::commands::ai_conversation_save_timeline,
            ai::commands::ai_conversation_delete,
            ai::commands::ai_settings_get,
            ai::commands::ai_settings_set,
            ai::commands::ai_list_models,
            ai::commands::ai_provider_list,
            ai::commands::ai_provider_save,
            ai::commands::ai_provider_delete,
        ])
        .run(tauri::generate_context!())
        .expect("RSSH startup failed");
}
