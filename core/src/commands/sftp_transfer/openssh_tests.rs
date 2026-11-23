//! Opt-in real OpenSSH test. RSSH_OPENSSH_TEST_CONFIG is a JSON file containing
//! {base, port, username}; base contains client private key and known_hosts.
//! The SSH server must run /usr/libexec/sftp-server and permit this temporary key.
use super::*;
use crate::files::transfer::{
    FileIdentity, NativeEndpoint, RenameError, TransferMode, TransferStream,
};
use crate::models::{Credential, CredentialType, Profile};
use std::sync::atomic::{AtomicBool, Ordering};

async fn connect(host: &Host, base: &std::path::Path, port: u16, username: &str) -> String {
    let state = host.state();
    let owner = SessionOwner::Window("openssh-test".into());
    let reservation = crate::commands::lifecycle::reserve_generated_resource(
        state,
        SessionKind::Ssh,
        owner.clone(),
    )
    .unwrap();
    let session_id = reservation.id().to_owned();
    let params = crate::ssh::client::ConnectParams {
        session_id: session_id.clone(),
        profile: Profile {
            id: "temporary-test".into(),
            name: "Temporary OpenSSH".into(),
            host: "127.0.0.1".into(),
            port,
            credential_id: "temporary".into(),
            bastion_profile_id: None,
            init_command: None,
            terminal_encoding: Default::default(),
            group_id: None,
            algorithms: Default::default(),
        },
        credential: Credential {
            id: "temporary".into(),
            name: "Temporary test key".into(),
            username: username.into(),
            credential_type: CredentialType::Key,
            secret: Some(std::fs::read_to_string(base.join("client")).unwrap()),
            save_to_remote: false,
        },
        bastion_chain: vec![],
        cols: 80,
        rows: 24,
        app: host.clone(),
        owner,
        recording_path: None,
        log_session_id: None,
        prompt_session_id: None,
        known_hosts_path: base.join("known_hosts"),
        timeout_secs: 5,
    };
    let result = crate::ssh::client::run_blocking_ssh(move || async move {
        crate::ssh::client::connect(params).await
    })
    .await
    .unwrap();
    reservation
        .activate(crate::commands::lifecycle::ReadySession::Ssh(result.handle))
        .unwrap();
    session_id
}

/// Test-only fault injection leaves all reads, writes and successful moves on
/// the real destination. The second rename fails after moving the old file.
struct FailInstall<'a> {
    inner: &'a dyn TransferEndpoint,
    moves: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl TransferEndpoint for FailInstall<'_> {
    async fn canonicalize_target(&self, p: &str) -> AppResult<String> {
        self.inner.canonicalize_target(p).await
    }
    async fn sibling(&self, p: &str, n: &str) -> AppResult<String> {
        self.inner.sibling(p, n).await
    }
    async fn stat(&self, p: &str) -> AppResult<Option<FileIdentity>> {
        self.inner.stat(p).await
    }
    async fn open(&self, p: &str, m: TransferMode) -> AppResult<Box<dyn TransferStream>> {
        self.inner.open(p, m).await
    }
    async fn rename(&self, f: &str, t: &str, replace: bool) -> Result<(), RenameError> {
        if replace {
            return Err(RenameError::Conflict(AppError::sftp(
                "test_conflict",
                json!({}),
            )));
        }
        if self.moves.fetch_add(1, Ordering::SeqCst) > 0 {
            return Err(RenameError::Other(AppError::sftp(
                "test_install_failed",
                json!({}),
            )));
        }
        self.inner.rename(f, t, false).await
    }
    async fn remove(&self, p: &str) -> AppResult<()> {
        self.inner.remove(p).await
    }
}

#[tokio::test]
#[ignore = "requires temporary Mac OpenSSH server; RSSH_OPENSSH_TEST_CONFIG"]
async fn real_openssh_replaces_and_resumes_upload_and_download() {
    let config_path =
        std::env::var("RSSH_OPENSSH_TEST_CONFIG").expect("temporary OpenSSH test config");
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config_path).unwrap()).unwrap();
    let base = std::path::PathBuf::from(config["base"].as_str().unwrap());
    let port = config["port"].as_u64().unwrap() as u16;
    let username = config["username"].as_str().unwrap();
    let local = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let (initial_host, _) = crate::ssh::test_support::headless_host();
    let state = initial_host.weak_state().upgrade().unwrap();
    let armed = Arc::new(AtomicBool::new(false));
    let current_session = Arc::new(Mutex::new(String::new()));
    let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let sink_state = state.clone();
    let sink_armed = armed.clone();
    let sink_session = current_session.clone();
    let sink_events = events.clone();
    let host = Host::new(
        state.clone(),
        Arc::new(move |name, payload| {
            if name.starts_with("sftp:progress:") {
                sink_events.lock().unwrap().push(payload.clone());
                if payload["phase"] == "transferring"
                    && payload["transferred"].as_u64().unwrap_or(0) >= 262144
                    && sink_armed.swap(false, Ordering::SeqCst)
                {
                    let session = sink_session.lock().unwrap().clone();
                    if let Some(handle) = sink_state.sessions.lock().unwrap().get(&session) {
                        handle.force_disconnect();
                    }
                }
            }
            true
        }),
    );
    let owner = SessionOwner::Window("openssh-test".into());
    let mut session = connect(&host, &base, port, username).await;
    *current_session.lock().unwrap() = session.clone();
    let data: Vec<u8> = (0..4 * 1024 * 1024)
        .map(|i| ((i * 17 + i / 65536) % 251) as u8)
        .collect();
    for direction in [Direction::Upload, Direction::Download] {
        let (src, dst) = match direction {
            Direction::Upload => (
                local.path().join("upload-source"),
                remote.path().join("upload-target"),
            ),
            Direction::Download => (
                remote.path().join("download-source"),
                local.path().join("download-target"),
            ),
        };
        std::fs::write(&src, &data).unwrap();
        for scenario in ["new", "replace", "disconnect"] {
            if scenario != "new" {
                std::fs::write(&dst, b"old contents remain until commit").unwrap();
            }
            let part = std::path::PathBuf::from(format!("{}.part", dst.display()));
            let bak = std::path::PathBuf::from(format!("{}.bak", dst.display()));
            std::fs::write(&part, b"unrelated part").unwrap();
            std::fs::write(&bak, b"unrelated backup").unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            register(&state, &owner, &session, &id).unwrap();
            let channel = super::super::sftp::sftp_connect_session(
                owner.clone(),
                &state,
                session.clone(),
                Some(id.clone()),
            )
            .await
            .unwrap();
            let (local_path, remote_path) = match direction {
                Direction::Upload => (src.to_str().unwrap(), dst.to_str().unwrap()),
                Direction::Download => (dst.to_str().unwrap(), src.to_str().unwrap()),
            };
            events.lock().unwrap().clear();
            armed.store(scenario == "disconnect", Ordering::SeqCst);
            let result = run(
                &state,
                &owner,
                &host,
                &channel,
                &NativeEndpoint,
                local_path,
                remote_path,
                &id,
                direction,
            )
            .await;
            if scenario == "disconnect" {
                assert!(result.is_err(), "disconnect must fail the attempt");
                let before = events
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|p| p["phase"] == "transferring")
                    .filter_map(|p| p["transferred"].as_u64())
                    .max()
                    .unwrap();
                assert!(before >= 262144 && before < data.len() as u64);
                assert_eq!(
                    std::fs::read(&dst).unwrap(),
                    b"old contents remain until commit"
                );
                session = connect(&host, &base, port, username).await;
                *current_session.lock().unwrap() = session.clone();
                let channel = super::super::sftp::sftp_connect_session(
                    owner.clone(),
                    &state,
                    session.clone(),
                    Some(id.clone()),
                )
                .await
                .unwrap();
                events.lock().unwrap().clear();
                run(
                    &state,
                    &owner,
                    &host,
                    &channel,
                    &NativeEndpoint,
                    local_path,
                    remote_path,
                    &id,
                    direction,
                )
                .await
                .unwrap();
                let ticks = events.lock().unwrap();
                let resumed = ticks.iter().find(|p| p["phase"] == "transferring").unwrap()
                    ["transferred"]
                    .as_u64()
                    .unwrap();
                assert!(
                    resumed >= before,
                    "validated breakpoint must retain acknowledged bytes"
                );
                assert!(ticks.iter().any(|p| p["phase"] == "verifying"));
                println!("{direction:?}: disconnected at {before}, resumed at {resumed}, final SHA-256 {}",hex_digest(&std::fs::read(&dst).unwrap()));
            } else {
                result.unwrap();
            }
            assert_eq!(std::fs::read(&src).unwrap(), data);
            assert_eq!(std::fs::read(&dst).unwrap(), data);
            assert_eq!(std::fs::read(part).unwrap(), b"unrelated part");
            assert_eq!(std::fs::read(bak).unwrap(), b"unrelated backup");
            state.sftp_transfers.forget(&owner, &id).unwrap();
        }
        // The fault boundary follows a real backup rename on each receiver.
        std::fs::write(&dst, b"original preserved at backup").unwrap();
        let channel =
            super::super::sftp::sftp_connect_session(owner.clone(), &state, session.clone(), None)
                .await
                .unwrap();
        let sftp = super::super::sftp::get_sftp(&state, &channel).unwrap();
        let (source, receiver): (&dyn TransferEndpoint, &dyn TransferEndpoint) = match direction {
            Direction::Upload => (&NativeEndpoint, sftp.as_ref()),
            Direction::Download => (sftp.as_ref(), &NativeEndpoint),
        };
        let fault = FailInstall {
            inner: receiver,
            moves: 0.into(),
        };
        let mut checkpoint = TransferState::default();
        let error = crate::ssh::transfer::transfer(
            source,
            src.to_str().unwrap(),
            &fault,
            dst.to_str().unwrap(),
            &mut checkpoint,
            &host,
            &uuid::Uuid::new_v4().to_string(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap_err();
        let backup = checkpoint.backup_path().unwrap();
        assert!(error.to_string().contains(backup));
        assert_eq!(
            std::fs::read(backup).unwrap(),
            b"original preserved at backup"
        );
        assert_eq!(std::fs::read(&src).unwrap(), data);
        crate::ssh::transfer::transfer(
            source,
            src.to_str().unwrap(),
            receiver,
            dst.to_str().unwrap(),
            &mut checkpoint,
            &host,
            &uuid::Uuid::new_v4().to_string(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), data);
    }
    // Source links retain the existing download/upload behavior. Receiver
    // links remain forbidden by the endpoint and are never followed for writes.
    for direction in [Direction::Upload, Direction::Download] {
        let source_root = if direction == Direction::Upload {
            local.path()
        } else {
            remote.path()
        };
        let destination_root = if direction == Direction::Upload {
            remote.path()
        } else {
            local.path()
        };
        let source = source_root.join("symlink-original");
        let link = source_root.join("symlink-source");
        let destination = destination_root.join("symlink-result");
        std::fs::write(&source, &data).unwrap();
        std::os::unix::fs::symlink(&source, &link).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        register(&state, &owner, &session, &id).unwrap();
        let channel = super::super::sftp::sftp_connect_session(
            owner.clone(),
            &state,
            session.clone(),
            Some(id.clone()),
        )
        .await
        .unwrap();
        let (local_path, remote_path) = if direction == Direction::Upload {
            (link.to_str().unwrap(), destination.to_str().unwrap())
        } else {
            (destination.to_str().unwrap(), link.to_str().unwrap())
        };
        run(
            &state,
            &owner,
            &host,
            &channel,
            &NativeEndpoint,
            local_path,
            remote_path,
            &id,
            direction,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), data);
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(&source).unwrap(), data);
        state.sftp_transfers.forget(&owner, &id).unwrap();
    }
    crate::commands::lifecycle::close_owner(&state, &owner);
}

fn hex_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
