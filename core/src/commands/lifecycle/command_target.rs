//! Short-lived writes to an existing, owner-scoped interactive terminal.

use std::sync::Weak;

use serde_json::json;

use crate::emitter::Host;
use crate::error::{locked, AppError, AppResult};
use crate::state::{AppState, SessionKind, SessionOwner, SessionPhase};
use crate::terminal::capture::OutputTap;

/// The tap survives while waiting for completion; the native session and its
/// child process do not. Each write rechecks the reservation that supplied it.
#[derive(Clone)]
pub(crate) struct CommandTarget {
    pub output: OutputTap,
    state: Weak<AppState>,
    owner: SessionOwner,
    session_id: String,
    pub(crate) nonce: uuid::Uuid,
    kind: SessionKind,
}

enum Writer {
    Ssh(crate::ssh::client::SessionHandle),
    Ssm(crate::aws::ssm::SsmHandle),
    #[cfg(any(windows, macos, linux, ohos))]
    Pty(crate::terminal::pty::PtyWriter),
    #[cfg(any(windows, macos, linux, ohos))]
    Serial(crate::terminal::serial::SerialHandle),
    Telnet(crate::terminal::telnet::TelnetHandle),
}

// Reading or writing a blocked terminal must not hold the lifecycle lock. Get
// the scoped writer under the registry lock, then perform I/O after releasing it.
fn resolve(
    state: &AppState,
    owner: &SessionOwner,
    id: &str,
    nonce: Option<uuid::Uuid>,
) -> AppResult<(uuid::Uuid, SessionKind, OutputTap, Writer)> {
    let registry = locked(&state.lifecycle_sessions)?;
    let missing = || AppError::not_found("session_not_found", json!({"id": id}));
    let record = registry.get(id).ok_or_else(missing)?;
    if &record.owner != owner {
        return Err(AppError::config(
            "session_owner_mismatch",
            json!({"id": id}),
        ));
    }
    if record.phase() != SessionPhase::Ready || nonce.is_some_and(|nonce| record.nonce != nonce) {
        return Err(missing());
    }

    let handle = match record.kind {
        SessionKind::Ssh => locked(&state.sessions)?
            .get(id)
            .map(|h| (h.output.clone(), Writer::Ssh(h.clone()))),
        SessionKind::Ssm => locked(&state.ssm_sessions)?
            .get(id)
            .map(|h| (h.capture.clone(), Writer::Ssm(h.clone()))),
        #[cfg(any(windows, macos, linux, ohos))]
        SessionKind::Pty => locked(&state.pty_sessions)?
            .get(id)
            .map(|h| (h.output.clone(), Writer::Pty(h.writer.clone()))),
        #[cfg(any(windows, macos, linux, ohos))]
        SessionKind::Serial => locked(&state.serial_sessions)?
            .get(id)
            .map(|h| (h.output.clone(), Writer::Serial(h.clone()))),
        SessionKind::Telnet => locked(&state.telnet_sessions)?
            .get(id)
            .map(|h| (h.output.clone(), Writer::Telnet(h.clone()))),
        SessionKind::Sftp | SessionKind::Forward => {
            return Err(AppError::config("session_kind_mismatch", json!({"id": id})))
        }
    };
    let (output, writer) = handle.ok_or_else(missing)?;
    if output.is_closed() {
        return Err(missing());
    }
    Ok((record.nonce, record.kind, output, writer))
}

pub(crate) fn owned_command_target(
    host: &Host,
    owner: &SessionOwner,
    session_id: &str,
) -> AppResult<CommandTarget> {
    let (nonce, kind, output, _) = resolve(host.state(), owner, session_id, None)?;
    Ok(CommandTarget {
        output,
        state: host.weak_state(),
        owner: owner.clone(),
        session_id: session_id.to_owned(),
        nonce,
        kind,
    })
}

impl CommandTarget {
    pub fn is_raw(&self) -> bool {
        match self.kind {
            SessionKind::Telnet => true,
            #[cfg(any(windows, macos, linux, ohos))]
            SessionKind::Serial => true,
            _ => false,
        }
    }

    fn writer(&self) -> AppResult<Writer> {
        let state = self.state.upgrade().ok_or_else(|| {
            AppError::not_found("session_not_found", json!({"id": self.session_id}))
        })?;
        let (_, _, _, writer) = resolve(&state, &self.owner, &self.session_id, Some(self.nonce))?;
        Ok(writer)
    }

    pub async fn write_line(&self, text: &str) -> AppResult<()> {
        let writer = self.writer()?;
        if let Writer::Telnet(handle) = writer {
            // Telnet owns profile-specific CR/LF endings and NVT negotiation.
            let text = text.to_owned();
            return blocking_write(move || handle.write_line(&text)).await;
        }
        let mut bytes = Vec::with_capacity(text.len() + 1);
        bytes.extend_from_slice(text.as_bytes());
        bytes.push(b'\r');
        writer.write(bytes).await
    }

    pub async fn interrupt(&self) -> AppResult<()> {
        if self.is_raw() {
            return Ok(());
        }
        self.writer()?.write(vec![3]).await
    }
}

impl Writer {
    async fn write(self, bytes: Vec<u8>) -> AppResult<()> {
        match self {
            Self::Ssh(handle) => handle.write(&bytes),
            Self::Ssm(handle) => handle.write(&bytes),
            #[cfg(any(windows, macos, linux, ohos))]
            Self::Pty(writer) => blocking_write(move || writer.write(&bytes)).await,
            #[cfg(any(windows, macos, linux, ohos))]
            Self::Serial(handle) => handle.write(&bytes).await,
            Self::Telnet(handle) => blocking_write(move || handle.write(&bytes)).await,
        }
    }
}

async fn blocking_write(work: impl FnOnce() -> AppResult<()> + Send + 'static) -> AppResult<()> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| AppError::other("task_join_failed", json!({"err": error.to_string()})))?
}

#[cfg(all(test, any(macos, linux)))]
mod tests {
    use super::*;
    use crate::commands::lifecycle::{close_resource, reserve_resource, ReadySession};
    use crate::terminal::pty::{self, PtyOut};
    use base64::{engine::general_purpose::STANDARD, Engine};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time::{timeout, Instant};

    async fn displayed_until(
        events: &mut tokio::sync::mpsc::UnboundedReceiver<(String, serde_json::Value)>,
        id: &str,
        expected: &str,
    ) -> String {
        let mut visible = Vec::new();
        timeout(Duration::from_secs(5), async {
            while !String::from_utf8_lossy(&visible).contains(expected) {
                let (event, payload) = events.recv().await.unwrap();
                assert_eq!(event, format!("pty:data:{id}"));
                visible.extend(STANDARD.decode(payload.as_str().unwrap()).unwrap());
            }
        })
        .await
        .unwrap();
        String::from_utf8(visible).unwrap()
    }

    #[tokio::test]
    async fn pty_command_observes_visible_output_and_releases_the_child_on_close() {
        let (host, mut events) = crate::ssh::test_support::headless_host();
        let owner = SessionOwner::Window("main".into());
        let id = uuid::Uuid::new_v4().to_string();
        let reservation =
            reserve_resource(host.state(), &id, SessionKind::Pty, owner.clone()).unwrap();
        let app = host.clone();
        let sink = Arc::new(move |id: &str, event| match event {
            PtyOut::Data(data) => {
                let _ = app.emit_bytes(&format!("pty:data:{id}"), &data);
            }
            PtyOut::Close => {
                let _ = app.emit(&format!("pty:close:{id}"), ());
            }
        });
        let (_, handle) = pty::spawn_command(
            id.clone(),
            80,
            24,
            sink,
            "/bin/sh".into(),
            "/usr/bin:/bin".into(),
            vec!["-i".into()],
        )
        .unwrap();
        reservation.activate(ReadySession::Pty(handle)).unwrap();

        let other = SessionOwner::Window("other".into());
        assert!(owned_command_target(&host, &other, &id).is_err());
        let target = owned_command_target(&host, &owner, &id).unwrap();
        // A regular keystroke stream establishes shell state before approval.
        crate::commands::pty::pty_write(
            host.state(),
            id.clone(),
            b"RSSH_FIXTURE=from_user; printf 'READY:%s\\n' configured\r".to_vec(),
        )
        .unwrap();
        displayed_until(&mut events, &id, "READY:configured").await;

        let mut capture = target.output.start("__rssh_done_test").unwrap();
        let command = r#"printf 'OUTPUT:%s\n' "$RSSH_FIXTURE"; IFS= read -r line; echo "__rssh_done_test:$?""#;
        target.write_line(command).await.unwrap();
        // The shell is blocked in read. xterm's existing event already has
        // its input echo and its output, including the user's shell variable.
        let visible = displayed_until(&mut events, &id, "OUTPUT:from_user").await;
        assert!(visible.contains("printf 'OUTPUT:"), "{visible}");
        target.write_line("second").await.unwrap();
        let result = capture.wait(Instant::now() + Duration::from_secs(5)).await;
        assert_eq!(result.exit_code, 0);
        assert!(
            result.output.contains("OUTPUT:from_user"),
            "{:?}",
            result.output
        );
        drop(capture);

        let mut pending = target.output.start("__rssh_done_unfinished").unwrap();
        close_resource(host.state(), &id, SessionKind::Pty, &owner).unwrap();
        assert!(target.output.is_closed());
        let stopped = pending.wait(Instant::now() + Duration::from_secs(1)).await;
        assert!(stopped.early_terminated && !stopped.timed_out);
        assert!(target.interrupt().await.is_err());
        assert!(target.write_line("stale").await.is_err());
        // EOF arrives despite target and pending remaining alive.
        timeout(Duration::from_secs(5), async {
            while events.recv().await.unwrap().0 != format!("pty:close:{id}") {}
        })
        .await
        .unwrap();
    }
}
