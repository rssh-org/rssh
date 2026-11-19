//! The actor publishes the exact command before emitting its approval card.
//! Callers can only act on that card in its original transport session.
use std::collections::HashMap;
use std::future::{pending, Future};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use super::CommandOutcome;
use crate::commands::lifecycle::{owned_command_target, CommandTarget};
use crate::emitter::Host;
use crate::error::{locked, AppError, AppResult};
use crate::state::SessionOwner;
use crate::terminal::capture::{command_body, CapturedOutput, CommandCapture};

#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TerminalCommandAction {
    Approve { bracketed_paste: bool },
    Blocked { reason: String },
    Stop,
    Submit,
}

enum Request {
    User(SessionOwner, TerminalCommandAction),
    Rebound,
}

#[derive(Clone, Copy, PartialEq)]
enum Decision {
    Pending,
    Approved,
    Finishing,
}

struct Active {
    decision: Decision,
    id: String,
    target: String,
    requests: mpsc::UnboundedSender<Request>,
    receipt: watch::Receiver<bool>,
}

#[derive(Default)]
struct Registry {
    active: Option<Active>,
    // IDs only. The existing terminal mutation log owns the redacted result.
    completed: HashMap<String, String>,
    rebound_target: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct TerminalCommands(Arc<Mutex<Registry>>);

fn not_pending(id: &str) -> AppError {
    AppError::other("ai_tool_call_not_pending", json!({ "tool_call_id": id }))
}

impl TerminalCommands {
    pub(super) fn propose(
        &self,
        id: &str,
        target: &str,
        host: &Host,
    ) -> AppResult<TerminalProposal> {
        let nonce = locked(&host.state().lifecycle_sessions)?
            .get(target)
            .filter(|record| record.phase() == crate::state::SessionPhase::Ready)
            .map(|record| record.nonce);
        let mut registry = locked(&self.0)?;
        if registry.active.is_some() || registry.completed.contains_key(id) {
            return Err(AppError::other("ai_command_busy", json!({})));
        }
        let (requests, rx) = mpsc::unbounded_channel();
        let (finished, receipt) = watch::channel(false);
        if registry
            .rebound_target
            .as_deref()
            .is_some_and(|current| current != target)
        {
            let _ = requests.send(Request::Rebound);
        }
        registry.active = Some(Active {
            decision: Decision::Pending,
            id: id.to_owned(),
            target: target.to_owned(),
            requests,
            receipt,
        });
        Ok(TerminalProposal {
            registry: self.clone(),
            rx,
            finished,
            processed: false,
            target: target.to_owned(),
            nonce,
        })
    }

    pub(crate) fn request(
        &self,
        id: &str,
        target: &str,
        owner: &SessionOwner,
        action: TerminalCommandAction,
    ) -> AppResult<Option<watch::Receiver<bool>>> {
        let mut registry = locked(&self.0)?;
        if registry
            .completed
            .get(id)
            .is_some_and(|prior| prior == target)
        {
            return Ok(None);
        }
        let active = registry
            .active
            .as_mut()
            .filter(|active| active.id == id && active.target == target)
            .ok_or_else(|| not_pending(id))?;
        // Several callers may lose the IPC reply to the same approval. Attach
        // them to its receipt without queueing a second write or a stale stop.
        let next = match (active.decision, &action) {
            (Decision::Finishing, _)
            | (
                Decision::Approved,
                TerminalCommandAction::Approve { .. } | TerminalCommandAction::Blocked { .. },
            ) => return Ok(Some(active.receipt.clone())),
            (Decision::Pending, TerminalCommandAction::Approve { .. }) => Decision::Approved,
            _ => Decision::Finishing,
        };
        active
            .requests
            .send(Request::User(owner.clone(), action))
            .map_err(|_| not_pending(id))?;
        active.decision = next;
        Ok(Some(active.receipt.clone()))
    }

    pub(crate) fn rebind(&self, target: &str) -> AppResult<()> {
        let mut registry = locked(&self.0)?;
        if let Some(active) = &registry.active {
            if active.target != target {
                let _ = active.requests.send(Request::Rebound);
            }
        }
        registry.rebound_target = Some(target.to_owned());
        Ok(())
    }
}

pub(super) struct TerminalProposal {
    registry: TerminalCommands,
    target: String,
    nonce: Option<uuid::Uuid>,
    rx: mpsc::UnboundedReceiver<Request>,
    finished: watch::Sender<bool>,
    pub(super) processed: bool,
}

// The proposal stays in the handler through its synchronous audit/event update.
// Dropping an unprocessed proposal (shutdown or cancelled MCP request) closes
// its receipts with an error and releases the capture.
impl Drop for TerminalProposal {
    fn drop(&mut self) {
        if let Ok(mut registry) = self.registry.0.lock() {
            if let Some(active) = registry.active.take() {
                // An abandoned card also consumes its ID. A late approval
                // must never be able to run a replacement tool request.
                registry.completed.insert(active.id, active.target);
                if self.processed {
                    self.finished.send_replace(true);
                }
            }
        }
    }
}

impl From<CapturedOutput> for CommandOutcome {
    fn from(value: CapturedOutput) -> Self {
        Self::Result {
            exit_code: value.exit_code,
            output: value.output,
            timed_out: value.timed_out,
            early_terminated: value.early_terminated,
        }
    }
}

fn stopped() -> CommandOutcome {
    CommandOutcome::Result {
        exit_code: -1,
        output: String::new(),
        early_terminated: true,
        timed_out: false,
    }
}

fn failed(reason: String) -> CommandOutcome {
    CommandOutcome::Result {
        exit_code: -1,
        output: reason,
        early_terminated: false,
        timed_out: false,
    }
}

type WriteFuture = Pin<Box<dyn Future<Output = AppResult<()>> + Send>>;

// A blocking writer may already have queued the command when the actor is
// cancelled. Finish that same write before sending ETX, keeping the capture
// exclusive until both writes settle. The target holds no PTY child handle.
struct Running {
    capture: Option<CommandCapture>,
    target: CommandTarget,
    write: Option<WriteFuture>,
    started: bool,
}

impl Running {
    fn cleanup(&mut self, interrupt: bool) -> Option<tokio::task::JoinHandle<()>> {
        let capture = self.capture.take()?;
        let interrupt = interrupt && !self.target.is_raw();
        if !self.started || (self.write.is_none() && !interrupt) {
            return None;
        }
        let write = self.write.take();
        let target = self.target.clone();
        Some(tokio::spawn(async move {
            if let Some(write) = write {
                if write.await.is_err() {
                    return;
                }
            }
            if interrupt {
                if let Err(error) = target.interrupt().await {
                    log::debug!("terminal interrupt: {error}");
                }
            }
            drop(capture);
        }))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let interrupt = self
            .capture
            .as_mut()
            .is_some_and(|c| c.interrupt().is_some());
        self.cleanup(interrupt);
    }
}

impl TerminalProposal {
    pub(super) async fn execute(
        &mut self,
        host: &Host,
        full_command: &str,
        sentinel: &str,
        timeout_s: u32,
    ) -> CommandOutcome {
        let (owner, paste) = match self.rx.recv().await {
            Some(Request::User(owner, TerminalCommandAction::Approve { bracketed_paste })) => {
                (owner, bracketed_paste)
            }
            Some(Request::User(_, TerminalCommandAction::Blocked { reason })) => {
                return failed(reason)
            }
            Some(Request::User(_, TerminalCommandAction::Submit)) if sentinel.is_empty() => {
                return CommandOutcome::Result {
                    exit_code: 0,
                    output: String::new(),
                    timed_out: false,
                    early_terminated: false,
                }
            }
            _ => return stopped(),
        };
        let target = match owned_command_target(host, &owner, &self.target) {
            Ok(target) if Some(target.nonce) == self.nonce => target,
            Ok(_) => return stopped(),
            Err(error) => return failed(error.to_string()),
        };
        if target.is_raw() != sentinel.is_empty() {
            return failed(AppError::other("ai_shell_invalid", json!({})).to_string());
        }
        let capture = match target.output.start(sentinel) {
            Ok(capture) => capture,
            Err(error) => return failed(error.to_string()),
        };
        let writer = target.clone();
        let body = command_body(full_command, paste);
        let mut running = Running {
            capture: Some(capture),
            target,
            write: Some(Box::pin(async move { writer.write_line(&body).await })),
            started: false,
        };
        let deadline = Instant::now() + Duration::from_secs(timeout_s.clamp(1, 300).into());
        let (result, interrupt) = loop {
            let capture = running.capture.as_mut().expect("active command");
            tokio::select! {
                biased;
                output = capture.wait(deadline) => break (output, false),
                request = self.rx.recv() => match request {
                    Some(Request::User(_, TerminalCommandAction::Stop)) | Some(Request::Rebound) | None => {
                        if let Some(output) = capture.interrupt() {
                            break (output, true);
                        }
                    }
                    Some(Request::User(_, TerminalCommandAction::Submit)) => {
                        if let Some(output) = capture.submit() {
                            break (output, false);
                        }
                    }
                    Some(Request::User(_, TerminalCommandAction::Approve { .. } | TerminalCommandAction::Blocked { .. })) => {}
                },
                written = async {
                    let Some(write) = &mut running.write else { return pending().await };
                    running.started = true;
                    write.await
                } => {
                    running.write.take();
                    if let Err(error) = written {
                        running.cleanup(false);
                        return failed(error.to_string());
                    }
                }
            }
        };
        let write_finished = running.write.is_none();
        if let Some(cleanup) = running.cleanup(interrupt) {
            // Normal input was delivered. Finish a requested interrupt before
            // acknowledging it. A still-blocked writer must not delay close or
            // timeout; its cleanup retains only that writer and the capture.
            if write_finished {
                let _ = cleanup.await;
            }
        }
        result.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::commands::{
        ai_command_result_impl, ai_mcp_session_start_impl, ai_mcp_tool_impl, ai_terminal_command,
        AiTarget,
    };
    use crate::ai::session::{ExternalToolInput, ExternalToolResult};
    use crate::ai::tools::RunCommandInput;
    use crate::commands::{lifecycle, telnet};
    use base64::{engine::general_purpose::STANDARD, Engine};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn raw_approval_retries_return_the_committed_result_and_never_rewrite_the_device() {
        tokio::time::timeout(Duration::from_secs(10), exercise_device())
            .await
            .expect("terminal approval and each retry must settle");
    }

    async fn exercise_device() {
        let (host, mut events) = crate::ssh::test_support::headless_host();
        let state = host.state();
        let owner = SessionOwner::Window("terminal".into());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = telnet::telnet_open(
            host.clone(),
            owner.clone(),
            state,
            "127.0.0.1".into(),
            listener.local_addr().unwrap().port(),
            80,
            24,
            Some("lf".into()),
            None,
        )
        .await
        .unwrap();
        let (mut device, _) = listener.accept().await.unwrap();
        let session = ai_mcp_session_start_impl(
            state,
            host.clone(),
            owner.clone(),
            "terminal-tab".into(),
            AiTarget::Telnet(target.clone()),
            "device".into(),
            None,
        )
        .await
        .unwrap();

        let request = |id: &str| {
            ai_mcp_tool_impl(
                state,
                &session.tab_id,
                &owner,
                &session.instance_id,
                id.into(),
                ExternalToolInput::RunCommand(RunCommandInput {
                    cmd: "status".into(),
                    side_effect: "read-only".into(),
                    explain: "Read status".into(),
                    timeout_s: None,
                }),
            )
        };
        let action = |id: &str, actor: &str, owner: &SessionOwner, action| {
            ai_terminal_command(
                owner.clone(),
                state,
                session.tab_id.clone(),
                actor.into(),
                id.into(),
                target.clone(),
                action,
            )
        };
        let approve = || TerminalCommandAction::Approve {
            bracketed_paste: false,
        };

        for (id, finish) in [
            ("manual", TerminalCommandAction::Submit),
            ("stop", TerminalCommandAction::Stop),
            ("preflight", TerminalCommandAction::Stop),
            ("stale", TerminalCommandAction::Stop),
        ] {
            let run = request(id);
            tokio::pin!(run);
            tokio::select! {
                result = &mut run => panic!("accepted before approval: {result:?}"),
                _ = async { while events.recv().await.unwrap().0 != "ai:command_proposed:terminal-tab" {} } => {}
            }
            assert!(action(id, "previous-actor", &owner, approve())
                .await
                .is_err());
            assert!(action(
                id,
                &session.instance_id,
                &SessionOwner::Window("other".into()),
                approve()
            )
            .await
            .is_err());
            let forged = ai_command_result_impl(
                state,
                &session.tab_id,
                &owner,
                id.into(),
                0,
                "forged".into(),
                false,
                false,
                Some(&session.instance_id),
            )
            .await
            .unwrap_err();
            assert_eq!(forged.code(), "ai_tool_call_not_pending");

            if id == "preflight" {
                let response = action(id, &session.instance_id, &owner, finish)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(response.payload["early_terminated"], true);
                let retry = action(id, &session.instance_id, &owner, approve())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(retry.payload, response.payload);
            } else if id == "stale" {
                // Even an ID reused by an adapter cannot authorize a new
                // transport incarnation with the old card.
                locked(&state.lifecycle_sessions)
                    .unwrap()
                    .get_mut(&target)
                    .unwrap()
                    .nonce = uuid::Uuid::new_v4();
                let response = action(id, &session.instance_id, &owner, approve())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(response.payload["early_terminated"], true);
            } else {
                let primary = action(id, &session.instance_id, &owner, approve());
                let retry = action(id, &session.instance_id, &owner, approve());
                let expected = format!("device online ({id})");
                let drive_device = async {
                    let mut written = [0; 7];
                    device.read_exact(&mut written).await.unwrap();
                    assert_eq!(&written, b"status\n");
                    let reply = format!("{expected}\r\n");
                    device.write_all(reply.as_bytes()).await.unwrap();
                    let channel = format!("telnet:data:{target}");
                    let mut displayed = Vec::new();
                    while !displayed.ends_with(reply.as_bytes()) {
                        let (event, payload) = events.recv().await.unwrap();
                        if event == channel {
                            displayed.extend(STANDARD.decode(payload.as_str().unwrap()).unwrap());
                        }
                    }
                    action(id, &session.instance_id, &owner, finish)
                        .await
                        .unwrap()
                        .unwrap()
                };
                let (receipt, second, completion) = tokio::join!(primary, retry, drive_device);
                assert_eq!(receipt.unwrap().unwrap().payload, completion.payload);
                assert_eq!(second.unwrap().unwrap().payload, completion.payload);
                assert_eq!(completion.payload["output"], expected);
                assert_eq!(completion.payload["early_terminated"], id == "stop");
                let after = action(id, &session.instance_id, &owner, approve())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(after.payload, completion.payload);
            }
            let ExternalToolResult::RunCommand(result) = run.await.unwrap() else {
                panic!("wrong tool")
            };
            assert_eq!(result.early_terminated, id != "manual");
        }
        lifecycle::close_ai_session(state, &session.tab_id, &owner, Some(&session.instance_id))
            .await
            .unwrap();
        telnet::telnet_close(owner, state, target).unwrap();
        let mut remaining = Vec::new();
        device.read_to_end(&mut remaining).await.unwrap();
        assert!(
            remaining.is_empty(),
            "unexpected device bytes {remaining:?}"
        );
    }
}
