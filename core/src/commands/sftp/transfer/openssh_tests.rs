//! Opt-in integration test using an installed OpenSSH sftp-server subsystem.
//! Set RSSH_TEST_SFTP_SERVER to override the macOS executable path.
use super::{Attempt, Direction, Request};
use crate::commands::lifecycle::{reserve_generated_resource, ReadySession};
use crate::emitter::Host;
use crate::error::AppResult;
use crate::ssh::client::{
    default_client_config, null_logger, ssh_connect_stream, DialCtx, SshHandle, SshPeer,
};
use crate::ssh::sftp::SftpHandle;
use crate::state::{SessionKind, SessionOwner};
use russh::{server, Channel, ChannelId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Restriction {
    None,
    DenyRequest(&'static str),
    SingleSession,
}

struct OpenSsh {
    executable: PathBuf,
    restriction: Restriction,
    opened: bool,
    channels: HashMap<ChannelId, Channel<server::Msg>>,
    occupy_after_backup: Option<PathBuf>,
}

impl server::Handler for OpenSsh {
    type Error = russh::Error;

    async fn auth_none(&mut self, _: &str) -> Result<server::Auth, Self::Error> {
        Ok(server::Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if self.opened && self.restriction == Restriction::SingleSession {
            reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        }
        self.opened = true;
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        assert_eq!(name, "sftp");
        let mut server = tokio::process::Command::new(&self.executable);
        if let Restriction::DenyRequest(name) = self.restriction {
            server.args(["-P", name]);
        }
        let mut child = server
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("set RSSH_TEST_SFTP_SERVER to the OpenSSH sftp-server executable");
        let mut input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        let (mut from_client, mut to_client) =
            tokio::io::split(self.channels.remove(&id).unwrap().into_stream());
        let mut fault = self.occupy_after_backup.take();
        tokio::spawn(async move {
            let requests = async {
                tokio::io::copy(&mut from_client, &mut input).await?;
                input.shutdown().await
            };
            let responses = async {
                let mut buf = [0; 32768];
                loop {
                    let n = output.read(&mut buf).await?;
                    if n == 0 {
                        break;
                    }
                    // An independent writer claims the final name after OpenSSH
                    // has moved the old file, before RSSH receives that reply.
                    if let Some(path) = &fault {
                        if !path.exists() {
                            let mut claimant = std::fs::OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .open(path)?;
                            std::io::Write::write_all(&mut claimant, b"concurrent writer")?;
                            fault = None;
                        }
                    }
                    to_client.write_all(&buf[..n]).await?;
                }
                to_client.shutdown().await
            };
            let _ = tokio::try_join!(requests, responses);
            let _ = child.kill().await;
        });
        session.channel_success(id)?;
        Ok(())
    }
}

struct Connection {
    session: String,
    link: JoinHandle<std::io::Result<(u64, u64)>>,
    _ssh: SshHandle,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.link.abort();
    }
}

// Two memory pipes and an abortable relay simulate the SSH wire. Each call
// establishes and authenticates a new SSH connection without a network listener.
async fn connect(
    host: &Host,
    root: &Path,
    seed: u8,
    restriction: Restriction,
    fault: Option<&Path>,
) -> Connection {
    const HOST: &str = "openssh-transfer.test";
    let key: russh::keys::PrivateKey =
        russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[seed; 32]).into();
    let known_hosts_path = root.join(format!("known_hosts-{seed}"));
    if !known_hosts_path.exists() {
        russh::keys::known_hosts::learn_known_hosts_path(
            HOST,
            22,
            key.public_key(),
            &known_hosts_path,
        )
        .unwrap();
    }
    let (client, mut upstream) = tokio::io::duplex(65536);
    let (mut downstream, server) = tokio::io::duplex(65536);
    let link =
        tokio::spawn(
            async move { tokio::io::copy_bidirectional(&mut upstream, &mut downstream).await },
        );
    let executable = std::env::var_os("RSSH_TEST_SFTP_SERVER")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/libexec/sftp-server"));
    assert!(
        executable.is_file(),
        "set RSSH_TEST_SFTP_SERVER to an installed OpenSSH sftp-server"
    );
    let handler = OpenSsh {
        executable,
        restriction,
        opened: false,
        channels: Default::default(),
        occupy_after_backup: fault.map(Path::to_path_buf),
    };
    tokio::spawn(async move {
        let config = Arc::new(server::Config {
            keys: vec![key],
            ..Default::default()
        });
        let _ = server::run_stream(config, server, handler)
            .await
            .unwrap()
            .await;
    });
    let dial = DialCtx {
        config: default_client_config(),
        known_hosts_path,
        timeout_secs: 5,
        log: null_logger(),
        prompt_ctx: None,
    };
    let (mut ssh, _, fingerprint) = ssh_connect_stream(dial, client, HOST.to_owned(), 22)
        .await
        .unwrap();
    assert!(ssh
        .authenticate_none("transfer-test")
        .await
        .unwrap()
        .success());
    let ssh = SshHandle::new(
        ssh,
        SshPeer {
            host: HOST.into(),
            port: 22,
            username: "transfer-test".into(),
            fingerprint,
        },
    );
    let owner = SessionOwner::Window("transfer-test".into());
    let reservation = reserve_generated_resource(host.state(), SessionKind::Sftp, owner).unwrap();
    let session = reservation.id().to_owned();
    let handle = SftpHandle::from_handle(&ssh, uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
    reservation
        .activate(ReadySession::Sftp(Arc::new(handle)))
        .unwrap();
    Connection {
        session,
        link,
        _ssh: ssh,
    }
}

struct Transfer {
    local: PathBuf,
    remote: PathBuf,
    direction: Direction,
    id: String,
}

impl Transfer {
    fn new(root: &Path, direction: Direction) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        Self {
            local: root.join(format!("local-{id}")),
            remote: root.join(format!("remote-{id}")),
            direction,
            id,
        }
    }

    fn endpoints(&self) -> (&Path, &Path) {
        match self.direction {
            Direction::Upload => (&self.local, &self.remote),
            Direction::Download => (&self.remote, &self.local),
        }
    }

    async fn run(&self, host: &Host, connection: &Connection) -> AppResult<u64> {
        let request = Request {
            transfer_id: self.id.clone(),
            original_session_id: None,
            local_path: self.local.to_str().unwrap().to_owned(),
            remote_path: self.remote.to_str().unwrap().to_owned(),
            direction: self.direction,
        };
        let attempt = Attempt::begin(
            host.state(),
            &SessionOwner::Window("transfer-test".into()),
            &connection.session,
            request,
        )?;
        match self.direction {
            Direction::Upload => {
                attempt
                    .upload(host, &mut tokio::fs::File::open(&self.local).await?)
                    .await
            }
            Direction::Download => attempt.download(host, &self.local).await,
        }
    }
}

type Events = UnboundedReceiver<(String, serde_json::Value)>;

// Change bytes inside the confirmed prefix but outside the sampled windows.
// Restore the exact mtime to exercise the full byte validation on retry.
fn flip_prefix_byte(path: &Path) {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let modified = file.metadata().unwrap().modified().unwrap();
    file.seek(SeekFrom::Start(96 * 1024)).unwrap();
    let mut byte = [0];
    file.read_exact(&mut byte).unwrap();
    file.seek(SeekFrom::Current(-1)).unwrap();
    file.write_all(&[byte[0] ^ 0xff]).unwrap();
    file.set_modified(modified).unwrap();
}

fn progress(events: &mut Events, transfer: &Transfer) -> Vec<u64> {
    let name = format!("sftp:progress:{}", transfer.id);
    let mut values = Vec::new();
    while let Ok((event, data)) = events.try_recv() {
        if event == name {
            values.push(data["transferred"].as_u64().unwrap());
        }
    }
    values
}

async fn disconnect_after_prefix(
    transfer: &Transfer,
    host: &Host,
    connection: &Connection,
    events: &mut Events,
) -> u64 {
    let event_name = format!("sftp:progress:{}", transfer.id);
    let interrupted = transfer.run(host, connection);
    tokio::pin!(interrupted);
    let count = loop {
        tokio::select! {
            result = &mut interrupted => panic!("finished before fault: {result:?}"),
            event = events.recv() => {
                let (event, payload) = event.unwrap();
                if event == event_name {
                    let count = payload["transferred"].as_u64().unwrap();
                    if count >= 256 * 1024 { connection.link.abort(); break count; }
                }
            }
        }
    };
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(25), interrupted)
            .await
            .unwrap()
            .is_err()
    );
    count
}

#[tokio::test]
#[ignore = "requires an installed OpenSSH sftp-server"]
async fn openssh_new_replace_and_reconnect_in_both_directions() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (host, mut events) = crate::ssh::test_support::headless_host();
    let bytes: Vec<u8> = (0..2_500_000u64)
        .map(|i| ((i * 113) ^ (i >> 8) ^ (i >> 16)) as u8)
        .collect();

    for direction in [Direction::Upload, Direction::Download] {
        let label = if direction == Direction::Upload {
            "upload"
        } else {
            "download"
        };
        let mut transfer = Transfer::new(root, direction);
        let source = transfer.endpoints().0.to_owned();
        let target = transfer.endpoints().1.to_owned();
        let prefix = target.file_name().unwrap().to_str().unwrap();
        let existing_part = root.join(format!("{prefix}.part"));
        let existing_backup = root.join(format!("{prefix}.bak"));
        std::fs::write(&existing_part, b"not this transfer").unwrap();
        std::fs::write(&existing_backup, b"older backup").unwrap();
        std::fs::write(&source, b"first contents").unwrap();
        let first = connect(&host, root, 1, Restriction::None, None).await;
        assert_eq!(transfer.run(&host, &first).await.unwrap(), 14);
        assert_eq!(std::fs::read(&target).unwrap(), b"first contents");

        transfer.id = uuid::Uuid::new_v4().to_string();
        std::fs::write(&source, &bytes).unwrap();
        let interrupted_at = disconnect_after_prefix(&transfer, &host, &first, &mut events).await;
        assert_eq!(std::fs::read(&target).unwrap(), b"first contents");
        progress(&mut events, &transfer);
        // Same user, host and port, but a different independently trusted key.
        let other = connect(&host, root, 2, Restriction::None, None).await;
        assert_eq!(
            transfer.run(&host, &other).await.unwrap_err().code(),
            "sftp_transfer_changed"
        );
        let second = connect(&host, root, 1, Restriction::None, None).await;
        let parts: Vec<_> = std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".rssh-")
                    && path.extension().is_some_and(|ext| ext == "part")
            })
            .collect();
        assert_eq!(parts.len(), 1);
        for edited in [&parts[0], &source] {
            flip_prefix_byte(edited);
            let error = transfer.run(&host, &second).await.unwrap_err();
            assert_eq!(error.code(), "sftp_transfer_changed", "{label}: {edited:?}");
            assert_eq!(std::fs::read(&target).unwrap(), b"first contents");
            flip_prefix_byte(edited);
        }
        progress(&mut events, &transfer);
        assert_eq!(
            transfer.run(&host, &second).await.unwrap(),
            bytes.len() as u64
        );
        let resumed = progress(&mut events, &transfer);
        assert!(resumed[0] >= interrupted_at);
        assert!(resumed[0] < bytes.len() as u64);
        assert_eq!(resumed.last(), Some(&(bytes.len() as u64)));
        assert!(resumed.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
        assert_eq!(std::fs::read(&existing_part).unwrap(), b"not this transfer");
        assert_eq!(std::fs::read(&existing_backup).unwrap(), b"older backup");
        eprintln!(
            "{label}: cut at {interrupted_at}, resumed at {}, completed {} bytes",
            resumed[0],
            bytes.len()
        );
    }
}

#[tokio::test]
#[ignore = "requires an installed OpenSSH sftp-server"]
async fn openssh_truncated_upload_after_last_write_ack_preserves_original() {
    use std::sync::Mutex;

    let dir = tempfile::tempdir().unwrap();
    let transfer = Transfer::new(dir.path(), Direction::Upload);
    let (source, target) = transfer.endpoints();
    std::fs::write(source, b"new contents").unwrap();
    std::fs::write(target, b"original contents").unwrap();
    let (host, _events) = crate::ssh::test_support::headless_host();
    let event = format!("sftp:progress:{}", transfer.id);
    let part = Mutex::new(None::<PathBuf>);
    let host = Host::new(
        host.weak_state().upgrade().unwrap(),
        Arc::new(move |name, payload| {
            if name == event {
                let mut part = part.lock().unwrap();
                if let Some(path) = payload["partialPath"].as_str() {
                    *part = Some(path.into());
                }
                // A transferring progress event is emitted synchronously after
                // OpenSSH has acknowledged these bytes, before RSSH closes them.
                if payload["phase"] == "transferring" && payload["transferred"] == 12 {
                    let file = std::fs::OpenOptions::new()
                        .write(true)
                        .open(part.as_ref().unwrap())
                        .unwrap();
                    assert_eq!(file.metadata().unwrap().len(), 12);
                    file.set_len(0).unwrap();
                }
            }
            true
        }),
    );
    let connection = connect(&host, dir.path(), 1, Restriction::None, None).await;
    assert_eq!(
        transfer.run(&host, &connection).await.unwrap_err().code(),
        "sftp_part_changed"
    );
    assert_eq!(std::fs::read(target).unwrap(), b"original contents");
    assert_eq!(std::fs::read(source).unwrap(), b"new contents");
}

#[cfg(any(macos, linux))]
#[tokio::test]
#[ignore = "requires an installed OpenSSH sftp-server"]
async fn openssh_upload_retry_rejects_an_unrelated_hard_link_with_different_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let transfer = Transfer::new(dir.path(), Direction::Upload);
    let (source, target) = transfer.endpoints();
    std::fs::write(source, vec![0x33; 1024 * 1024]).unwrap();
    std::fs::write(target, b"original contents").unwrap();
    let (host, mut events) = crate::ssh::test_support::headless_host();
    let first = connect(&host, dir.path(), 1, Restriction::None, None).await;
    disconnect_after_prefix(&transfer, &host, &first, &mut events).await;
    let part = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|p| p.extension().is_some_and(|ext| ext == "part"))
        .unwrap();
    let unrelated = dir.path().join("independent");
    let prefix = std::fs::read(&part).unwrap();
    assert!(prefix.len() >= 256 * 1024 && prefix.len() < 1024 * 1024);
    assert_ne!(
        std::fs::metadata(&part).unwrap().permissions().mode() & 0o777,
        0o744
    );
    std::fs::write(&unrelated, &prefix).unwrap();
    std::fs::set_permissions(&unrelated, std::fs::Permissions::from_mode(0o744)).unwrap();
    std::fs::remove_file(&part).unwrap();
    std::fs::hard_link(&unrelated, &part).unwrap();

    let second = connect(&host, dir.path(), 1, Restriction::None, None).await;
    assert_eq!(
        transfer.run(&host, &second).await.unwrap_err().code(),
        "sftp_part_changed"
    );
    assert_eq!(std::fs::read(&part).unwrap(), prefix);
    assert_eq!(std::fs::read(&unrelated).unwrap(), prefix);
    assert_eq!(std::fs::read(target).unwrap(), b"original contents");
}

#[cfg(any(macos, linux))]
#[tokio::test]
#[ignore = "requires OpenSSH sftp-server and an unprivileged account"]
async fn openssh_download_requires_permission_on_the_parent_folder() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("downloads");
    std::fs::create_dir(&folder).unwrap();
    let mut transfer = Transfer::new(dir.path(), Direction::Download);
    transfer.local = folder.join("report");
    std::fs::write(&transfer.remote, b"updated").unwrap();
    std::fs::write(&transfer.local, b"original").unwrap();
    let (host, mut events) = crate::ssh::test_support::headless_host();
    let connection = connect(&host, dir.path(), 1, Restriction::None, None).await;

    let writable = std::fs::metadata(&folder).unwrap().permissions();
    for mode in [0o555, 0o000] {
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(mode)).unwrap();
        let result = transfer.run(&host, &connection).await;
        std::fs::set_permissions(&folder, writable.clone()).unwrap();
        assert_eq!(
            result.unwrap_err().code(),
            "sftp_provider_siblings_unavailable"
        );
        assert_eq!(std::fs::read(&transfer.local).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);
        assert!(progress(&mut events, &transfer).iter().all(|n| *n == 0));
    }
    assert_eq!(transfer.run(&host, &connection).await.unwrap(), 7);
    assert_eq!(std::fs::read(&transfer.local).unwrap(), b"updated");
}

#[tokio::test]
#[ignore = "requires an installed OpenSSH sftp-server"]
async fn openssh_late_publish_failure_retains_original_at_reported_backup() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (host, _events) = crate::ssh::test_support::headless_host();
    let transfer = Transfer::new(root, Direction::Upload);
    let (source, target) = transfer.endpoints();
    std::fs::write(source, b"new bytes").unwrap();
    std::fs::write(target, b"original bytes").unwrap();
    let connection = connect(
        &host,
        root,
        1,
        Restriction::DenyRequest("posix-rename"),
        Some(target),
    )
    .await;
    let error = transfer.run(&host, &connection).await.unwrap_err();
    let crate::error::AppError::Sftp(error) = error else {
        panic!("wrong failure: {error}")
    };
    assert_eq!(error.code, "sftp_replace_failed");
    let backup = error.params["backup"].as_str().unwrap();
    let backup_path = Path::new(backup);
    assert_eq!(
        std::fs::canonicalize(backup_path.parent().unwrap()).unwrap(),
        std::fs::canonicalize(target.parent().unwrap()).unwrap(),
    );
    let backup_id = backup_path
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .strip_prefix("rssh-")
        .unwrap()
        .strip_suffix(".bak")
        .unwrap();
    uuid::Uuid::parse_str(backup_id).unwrap();
    assert_eq!(std::fs::read(backup).unwrap(), b"original bytes");
    assert_eq!(std::fs::read(target).unwrap(), b"concurrent writer");
    assert_eq!(std::fs::read(source).unwrap(), b"new bytes");
}

#[tokio::test]
#[ignore = "requires an installed OpenSSH sftp-server"]
async fn openssh_overwrites_with_posix_rename_when_standard_rename_is_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let (host, _events) = crate::ssh::test_support::headless_host();
    let transfer = Transfer::new(root, Direction::Upload);
    let (source, target) = transfer.endpoints();
    std::fs::write(source, b"new contents").unwrap();
    std::fs::write(target, b"original contents").unwrap();
    let backup = target.with_extension("bak");
    std::fs::write(&backup, b"prior backup").unwrap();
    let connection = connect(&host, root, 1, Restriction::DenyRequest("rename"), None).await;
    assert_eq!(transfer.run(&host, &connection).await.unwrap(), 12);
    assert_eq!(std::fs::read(target).unwrap(), b"new contents");
    assert_eq!(std::fs::read(source).unwrap(), b"new contents");
    assert_eq!(std::fs::read(backup).unwrap(), b"prior backup");
}

#[tokio::test]
#[ignore = "requires an installed OpenSSH sftp-server"]
async fn openssh_replaces_over_the_original_channel_when_extra_sessions_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (host, _events) = crate::ssh::test_support::headless_host();
    let transfer = Transfer::new(dir.path(), Direction::Upload);
    let (source, target) = transfer.endpoints();
    std::fs::write(source, b"updated").unwrap();
    std::fs::write(target, b"original").unwrap();
    let connection = connect(&host, dir.path(), 1, Restriction::SingleSession, None).await;
    assert_eq!(transfer.run(&host, &connection).await.unwrap(), 7);
    assert_eq!(std::fs::read(target).unwrap(), b"updated");
}
