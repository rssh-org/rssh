//! Persistent `pi --mode rpc` subprocess over an SSH exec channel.
//!
//! pi (https://pi.dev) is the user's coding agent. RPC mode speaks JSONL on
//! stdin/stdout:
//!   - commands   → stdin   `{"type":"prompt","message":...}`
//!   - responses  → stdout  `{"type":"response","command":"prompt","success":true}`
//!   - events     → stdout  `{"type":"message_update",...}`, `{"type":"agent_end",...}`
//!
//! This module keeps the process alive for the lifetime of a Pi tab:
//!   - stdout lines are forwarded to the frontend as `pi:line:{pi_id}` events,
//!   - frontend commands are forwarded to stdin via `PiHandle::send_line`.
//! The channel is opened on an *existing* SSH session (reusing `ssh_connect`'s
//! connection, so no re-authentication and bastion chains come for free).
//! Sessions are stored server-side under `~/.pi/agent/`, so another machine
//! connecting to the same host continues the same session.

use serde::Serialize;
use tokio::sync::mpsc;

use crate::emitter::Host;
use crate::error::{AppError, AppResult};
use crate::ssh::client::{spawn_ssh, SshHandle};
use serde_json::json;

pub enum PiCmd {
    SendLine(String),
    Close,
}

#[derive(Clone)]
pub struct PiHandle {
    tx: mpsc::UnboundedSender<PiCmd>,
    pub profile_id: String,
    pub parent_session_id: String,
}

#[derive(Clone, Serialize)]
pub struct PiLineEvent {
    pub pi_id: String,
    pub line: String,
}

#[derive(Clone, Serialize)]
pub struct PiStatusEvent {
    pub pi_id: String,
    /// "started" | "closed" | "error"
    pub status: String,
    pub message: Option<String>,
}

impl PiHandle {
    pub fn send_line(&self, line: &str) -> AppResult<()> {
        self.tx.send(PiCmd::SendLine(line.to_string())).map_err(|_| {
            AppError::other("pi_session_closed", json!({}))
        })
    }

    pub fn stop(&self) {
        let _ = self.tx.send(PiCmd::Close);
    }

    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub fn parent_session_id(&self) -> &str {
        &self.parent_session_id
    }
}

/// Spawn `pi --mode rpc` on an existing SSH connection and stream stdout lines
/// to the frontend as `pi:line:{pi_id}` events.
///
/// `extra_args` are appended verbatim to the pi command line (e.g. `--continue`
/// to resume the latest server-side session, `--name foo` to set a display name).
pub async fn spawn(
    host: Host,
    pi_id: String,
    ssh_handle: &SshHandle,
    profile_id: String,
    parent_session_id: String,
    extra_args: Vec<String>,
) -> AppResult<PiHandle> {
    let (tx, mut rx) = mpsc::unbounded_channel::<PiCmd>();

    let mut cmd = String::from("pi --mode rpc");
    for a in &extra_args {
        cmd.push(' ');
        cmd.push_str(a);
    }

    let channel = {
        let h = ssh_handle.lock().await;
        h.channel_open_session().await.map_err(|e| {
            AppError::ssh("pi_channel_failed", json!({ "err": e.to_string() }))
        })?
    };
    channel.exec(true, cmd.as_str()).await.map_err(|e| {
        AppError::ssh("pi_exec_failed", json!({ "err": e.to_string() }))
    })?;

    let handle = PiHandle {
        tx,
        profile_id,
        parent_session_id,
    };

    // Drive the channel on the SSH worker (LocalSet). Reader and writer are
    // split so the select loop can poll stdout and stdin commands concurrently.
    let stream = channel.into_stream();
    let (mut rd, mut wr) = tokio::io::split(stream);

    let host2 = host.clone();
    let pid = pi_id.clone();
    drop(spawn_ssh::<_, _, ()>(move || async move {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let mut buf: Vec<u8> = Vec::with_capacity(8192);
        let mut tmp = [0u8; 8192];
        loop {
            tokio::select! {
                c = rx.recv() => match c {
                    Some(PiCmd::SendLine(line)) => {
                        let mut bytes = line.into_bytes();
                        bytes.push(b'\n');
                        if let Err(e) = wr.write_all(&bytes).await {
                            let _ = host2.emit(
                                &format!("pi:status:{}", pid),
                                PiStatusEvent {
                                    pi_id: pid.clone(),
                                    status: "error".into(),
                                    message: Some(e.to_string()),
                                },
                            );
                            break;
                        }
                    }
                    Some(PiCmd::Close) | None => break,
                },
                n = rd.read(&mut tmp) => match n {
                    Ok(0) => break, // EOF — pi 退出
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                            let line: Vec<u8> = buf.drain(..=pos).collect();
                            let mut s =
                                String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
                            if s.ends_with('\r') {
                                s.pop();
                            }
                            if !s.trim().is_empty() {
                                let _ = host2.emit(
                                    &format!("pi:line:{}", pid),
                                    PiLineEvent {
                                        pi_id: pid.clone(),
                                        line: s,
                                    },
                                );
                            }
                        }
                    }
                    Err(_) => break,
                },
            }
        }
        let _ = wr.shutdown().await;
        let _ = host2.emit(
            &format!("pi:status:{}", pid),
            PiStatusEvent {
                pi_id: pid.clone(),
                status: "closed".into(),
                message: None,
            },
        );
        Ok(())
    }));

    Ok(handle)
}
