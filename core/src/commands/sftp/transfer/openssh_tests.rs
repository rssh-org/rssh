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

struct OpenSsh {
    executable: PathBuf,
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
        let mut child = tokio::process::Command::new(&self.executable)
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
async fn connect(host: &Host, root: &Path, seed: u8, fault: Option<&Path>) -> Connection {
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
        let first = connect(&host, root, 1, None).await;
        assert_eq!(transfer.run(&host, &first).await.unwrap(), 14);
        assert_eq!(std::fs::read(&target).unwrap(), b"first contents");

        transfer.id = uuid::Uuid::new_v4().to_string();
        std::fs::write(&source, &bytes).unwrap();
        let event_name = format!("sftp:progress:{}", transfer.id);
        let interrupted = transfer.run(&host, &first);
        tokio::pin!(interrupted);
        let interrupted_at = loop {
            tokio::select! {
                result = &mut interrupted => panic!("finished before fault: {result:?}"),
                event = events.recv() => {
                    let (event, payload) = event.unwrap();
                    if event == event_name {
                        let count = payload["transferred"].as_u64().unwrap();
                        if count >= 256 * 1024 { first.link.abort(); break count; }
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
        assert_eq!(std::fs::read(&target).unwrap(), b"first contents");
        progress(&mut events, &transfer);
        // Same user, host and port, but a different independently trusted key.
        let other = connect(&host, root, 2, None).await;
        assert_eq!(
            transfer.run(&host, &other).await.unwrap_err().code(),
            "sftp_transfer_changed"
        );
        let second = connect(&host, root, 1, None).await;
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
async fn openssh_late_publish_failure_retains_original_at_reported_backup() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (host, _events) = crate::ssh::test_support::headless_host();
    let transfer = Transfer::new(root, Direction::Upload);
    let (source, target) = transfer.endpoints();
    std::fs::write(source, b"new bytes").unwrap();
    std::fs::write(target, b"original bytes").unwrap();
    let connection = connect(&host, root, 1, Some(target)).await;
    let error = transfer.run(&host, &connection).await.unwrap_err();
    let crate::error::AppError::Sftp(error) = error else {
        panic!("wrong failure: {error}")
    };
    assert_eq!(error.code, "sftp_replace_failed");
    let backup = error.params["backup"].as_str().unwrap();
    assert!(backup.ends_with(".bak"));
    assert_eq!(std::fs::read(backup).unwrap(), b"original bytes");
    assert_eq!(std::fs::read(target).unwrap(), b"concurrent writer");
    assert_eq!(std::fs::read(source).unwrap(), b"new bytes");
}
