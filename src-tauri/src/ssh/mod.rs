pub mod algorithms;
pub mod auth;
pub mod bastion;
pub mod client;
pub mod forward;
pub mod known_hosts;
pub mod prompt;
pub mod sftp;

#[cfg(test)]
mod auth_tests;

#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use crate::emitter::Host;
    use crate::state::AppState;

    pub fn headless_host() -> (
        Host,
        tokio::sync::mpsc::UnboundedReceiver<(String, serde_json::Value)>,
    ) {
        let db = Arc::new(crate::db::Db::open_in_memory().unwrap());
        let state = Arc::new(AppState {
            secret_store: Arc::new(crate::secret::DbStore::new(db.clone())),
            db,
            lifecycle_sessions: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            #[cfg(any(windows, macos, linux, ohos))]
            pty_sessions: Mutex::new(HashMap::new()),
            #[cfg(any(windows, macos, linux, ohos))]
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
            data_dir: PathBuf::new(),
        });
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let host = Host::Headless {
            sink: Arc::new(move |event, payload| tx.send((event.to_owned(), payload)).is_ok()),
            state,
        };
        (host, rx)
    }
}
