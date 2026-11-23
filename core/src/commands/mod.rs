use std::path::Path;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub mod aws_signin;
pub mod aws_ssm;
pub mod command_block;
pub mod discovery;
pub mod forward;
pub mod group;
pub mod incident_report;
pub mod lifecycle;
pub mod plugin;
pub mod profile;
#[cfg(any(windows, macos, linux, ohos))]
pub mod pty;
#[cfg(any(windows, macos, linux, ohos))]
pub mod serial;
pub mod session;
pub mod settings;
pub mod sftp;
pub mod sftp_transfer;
pub mod sync;
pub mod telnet;
pub mod update;

/// Run data-directory I/O and cross-process lock waits on the blocking pool.
pub async fn run_data_dir_task<T: Send + 'static>(
    state: &AppState,
    work: impl FnOnce(&Path) -> T + Send + 'static,
) -> AppResult<T> {
    let dir = state.data_dir.clone();
    tokio::task::spawn_blocking(move || work(&dir))
        .await
        .map_err(|e| {
            AppError::other(
                "task_join_failed",
                serde_json::json!({ "err": e.to_string() }),
            )
        })
}
