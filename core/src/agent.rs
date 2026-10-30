//! MCP admission and cancellation. Command execution belongs to the AI actor.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::watch;

use crate::ai::{
    commands::AiSessionInfo,
    session::{DownloadFileResult, ExternalToolInput, ExternalToolResult, RunCommandResult},
    shell::ShellKind,
};
use crate::emitter::{Host, Sink};
use crate::error::{locked, AppError, AppResult};
use crate::state::{AppState, SessionKind, SessionOwner, SessionPhase, SessionRecord};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_COMMAND_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct ShellInfo {
    pub kind: ShellKind,
    pub source: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AgentSession {
    pub session_id: String,
    pub tab_id: String,
    pub nonce: String,
    pub name: String,
    pub kind: String,
    pub shell: ShellInfo,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenSessionInput {
    pub session_id: String,
    pub title: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionRequest {
    pub id: String,
    pub session_id: String,
    pub tab_id: String,
    pub nonce: String,
    pub kind: String,
    pub title: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct OpenSessionResult {
    pub session_id: String,
    pub conversation_id: String,
    pub instance_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunCommandInput {
    pub session_id: String,
    pub cmd: String,
    pub explain: String,
    pub side_effect: String,
    pub timeout_s: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CommandResult {
    pub session_id: String,
    #[serde(flatten)]
    pub command: RunCommandResult,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloadFileInput {
    pub session_id: String,
    pub remote_path: String,
    pub max_mb: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct DownloadResult {
    pub session_id: String,
    #[serde(flatten)]
    pub download: DownloadFileResult,
}

#[derive(Clone, Debug, Serialize)]
pub struct EndpointInfo {
    pub url: String,
    pub token: String,
}

struct Endpoint {
    nonce: uuid::Uuid,
    info: Option<EndpointInfo>,
    shutdown: Option<watch::Sender<bool>>,
}

struct Registration {
    owner: SessionOwner,
    info: AgentSession,
    sink: Sink,
}

struct Binding {
    owner: SessionOwner,
    info: AgentSession,
    opened: OpenSessionResult,
    shutdown: watch::Sender<bool>,
}

struct PendingSession {
    owner: SessionOwner,
    request: SessionRequest,
    sink: Sink,
    accepting: bool,
    binding: Option<Binding>,
    result: watch::Sender<Option<AppResult<OpenSessionResult>>>,
}

struct Command {
    owner: SessionOwner,
    instance_id: String,
    session_id: String,
    cancelled: watch::Sender<bool>,
}

#[derive(Default)]
struct Inner {
    registrations: HashMap<String, Registration>,
    pending: HashMap<String, PendingSession>,
    bindings: HashMap<String, Binding>,
    // Only in-flight cancellation lives here. Approval, validation,
    // terminal events, output redaction and persistence live in the AI actor.
    commands: HashMap<String, Command>,
    endpoints: HashMap<SessionOwner, Endpoint>,
}

#[derive(Default)]
pub struct AgentBridge {
    inner: Mutex<Inner>,
}

pub struct EndpointReservation<'a> {
    bridge: &'a AgentBridge,
    owner: SessionOwner,
    nonce: uuid::Uuid,
    committed: bool,
}

impl EndpointReservation<'_> {
    pub fn commit(
        mut self,
        info: EndpointInfo,
        shutdown: watch::Sender<bool>,
    ) -> AppResult<EndpointInfo> {
        let mut inner = locked(&self.bridge.inner)?;
        let endpoint = inner
            .endpoints
            .get_mut(&self.owner)
            .filter(|endpoint| endpoint.nonce == self.nonce)
            .ok_or_else(|| error("agent_endpoint_stopped"))?;
        endpoint.info = Some(info.clone());
        endpoint.shutdown = Some(shutdown);
        self.committed = true;
        Ok(info)
    }
}

impl Drop for EndpointReservation<'_> {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Ok(mut inner) = self.bridge.inner.lock() {
            if inner
                .endpoints
                .get(&self.owner)
                .is_some_and(|e| e.nonce == self.nonce)
            {
                inner.endpoints.remove(&self.owner);
            }
        }
    }
}

impl AgentBridge {
    pub fn endpoint(&self, owner: &SessionOwner) -> AppResult<Option<EndpointInfo>> {
        Ok(locked(&self.inner)?
            .endpoints
            .get(owner)
            .and_then(|e| e.info.clone()))
    }

    pub fn reserve_endpoint(&self, owner: &SessionOwner) -> AppResult<EndpointReservation<'_>> {
        let mut inner = locked(&self.inner)?;
        if inner.endpoints.contains_key(owner) {
            return Err(error("agent_endpoint_running"));
        }
        let nonce = uuid::Uuid::new_v4();
        inner.endpoints.insert(
            owner.clone(),
            Endpoint {
                nonce,
                info: None,
                shutdown: None,
            },
        );
        Ok(EndpointReservation {
            bridge: self,
            owner: owner.clone(),
            nonce,
            committed: false,
        })
    }

    pub fn stop_endpoint(&self, owner: &SessionOwner) -> AppResult<()> {
        let mut inner = locked(&self.inner)?;
        stop_endpoint(&mut inner, owner);
        Ok(())
    }

    pub fn stop_endpoint_if_token(&self, owner: &SessionOwner, token: &str) -> AppResult<()> {
        let mut inner = locked(&self.inner)?;
        if endpoint_matches(&inner, owner, token) {
            stop_endpoint(&mut inner, owner);
        }
        Ok(())
    }

    /// Lifecycle invokes this while holding its registry lock. Never acquire
    /// lifecycle or AI locks from this method.
    pub(crate) fn close_session(&self, owner: &SessionOwner, session_id: &str) -> AppResult<()> {
        let mut inner = locked(&self.inner)?;
        if inner
            .registrations
            .get(session_id)
            .is_some_and(|r| &r.owner == owner)
        {
            inner.registrations.remove(session_id);
        }
        close_bindings(&mut inner, |b| {
            &b.owner == owner && b.info.session_id == session_id
        });
        close_requests(
            &mut inner,
            |r| &r.owner == owner && r.request.session_id == session_id,
            "agent_session_unavailable",
        );
        cancel_commands(&mut inner, |c| {
            &c.owner == owner && c.session_id == session_id
        });
        Ok(())
    }

    pub(crate) fn close_ai_session(
        &self,
        owner: &SessionOwner,
        tab_id: &str,
        instance_id: &str,
    ) -> AppResult<()> {
        let mut inner = locked(&self.inner)?;
        close_bindings(&mut inner, |b| {
            &b.owner == owner && b.info.tab_id == tab_id && b.opened.instance_id == instance_id
        });
        close_requests(
            &mut inner,
            |r| {
                &r.owner == owner
                    && r.binding.as_ref().is_some_and(|b| {
                        b.info.tab_id == tab_id && b.opened.instance_id == instance_id
                    })
            },
            "agent_session_unavailable",
        );
        cancel_commands(&mut inner, |c| {
            &c.owner == owner && c.instance_id == instance_id
        });
        Ok(())
    }
}

fn error(code: &'static str) -> AppError {
    AppError::other(code, json!({}))
}

fn endpoint_matches(inner: &Inner, owner: &SessionOwner, token: &str) -> bool {
    inner
        .endpoints
        .get(owner)
        .and_then(|e| e.info.as_ref())
        .is_some_and(|e| e.token == token)
}

fn stop_endpoint(inner: &mut Inner, owner: &SessionOwner) {
    if let Some(shutdown) = inner.endpoints.remove(owner).and_then(|e| e.shutdown) {
        shutdown.send_replace(true);
    }
    close_bindings(inner, |b| &b.owner == owner);
    close_requests(inner, |r| &r.owner == owner, "agent_endpoint_stopped");
    cancel_commands(inner, |c| &c.owner == owner);
    inner.commands.retain(|_, c| &c.owner != owner);
}

fn close_bindings(inner: &mut Inner, matches: impl Fn(&Binding) -> bool) {
    inner.bindings.retain(|_, binding| {
        if !matches(binding) {
            return true;
        }
        binding.shutdown.send_replace(true);
        false
    });
}

fn close_requests(
    inner: &mut Inner,
    matches: impl Fn(&PendingSession) -> bool,
    code: &'static str,
) {
    inner.pending.retain(|_, pending| {
        if !matches(pending) {
            return true;
        }
        pending.result.send_replace(Some(Err(error(code))));
        if let Some(binding) = &pending.binding {
            binding.shutdown.send_replace(true);
        }
        (pending.sink)(
            &format!("agent:session_cancelled:{}", pending.request.session_id),
            json!({"id":pending.request.id}),
        );
        false
    });
}

fn cancel_commands(inner: &mut Inner, matches: impl Fn(&Command) -> bool) {
    for command in inner.commands.values().filter(|c| matches(c)) {
        command.cancelled.send_replace(true);
    }
}

fn valid_session(
    info: &AgentSession,
    owner: &SessionOwner,
    registry: &HashMap<String, SessionRecord>,
) -> bool {
    registry.get(&info.session_id).is_some_and(|r| {
        &r.owner == owner && r.phase() == SessionPhase::Ready && r.nonce.to_string() == info.nonce
    })
}

fn session_info(
    state: &AppState,
    id: &str,
    record: &SessionRecord,
) -> AppResult<Option<AgentSession>> {
    let (kind, shell) = match record.kind {
        SessionKind::Ssh => {
            let sessions = locked(&state.sessions)?;
            let session = sessions
                .get(id)
                .ok_or_else(|| error("agent_session_unavailable"))?;
            let auto = crate::db::settings::get(&state.db, "ai_auto_detect_remote_shell")?
                .as_deref()
                == Some("1");
            let cached = if auto {
                locked(&state.ai_remote_shell_cache)?
                    .get(session.profile_id())
                    .copied()
            } else {
                None
            };
            (
                "ssh",
                ShellInfo {
                    kind: cached.unwrap_or_default(),
                    source: if cached.is_some() {
                        "profile_cache"
                    } else {
                        "fallback"
                    }
                    .into(),
                },
            )
        }
        #[cfg(any(windows, macos, linux, ohos))]
        SessionKind::Pty => {
            let sessions = locked(&state.pty_sessions)?;
            let session = sessions
                .get(id)
                .ok_or_else(|| error("agent_session_unavailable"))?;
            (
                "local",
                ShellInfo {
                    kind: ShellKind::from_local_path(session.shell_path()),
                    source: "launch_config".into(),
                },
            )
        }
        #[cfg(any(windows, macos, linux, ohos))]
        SessionKind::Serial => (
            "serial",
            ShellInfo {
                kind: ShellKind::Serial,
                source: "transport".into(),
            },
        ),
        SessionKind::Telnet => (
            "telnet",
            ShellInfo {
                kind: ShellKind::Telnet,
                source: "transport".into(),
            },
        ),
        _ => return Ok(None),
    };
    Ok(Some(AgentSession {
        session_id: id.into(),
        tab_id: String::new(),
        nonce: record.nonce.to_string(),
        name: format!("{kind} {id}"),
        kind: kind.into(),
        shell,
    }))
}

/// Registration supplies UI routing, not permission. Every connected terminal
/// is discoverable; only an accepted open_session request permits commands.
pub fn register_session(
    state: &AppState,
    owner: &SessionOwner,
    sink: Sink,
    session_id: String,
    tab_id: String,
    label: String,
) -> AppResult<AgentSession> {
    if tab_id.is_empty() || tab_id.len() > 256 || label.len() > 512 {
        return Err(error("agent_invalid_session"));
    }
    let registry = locked(&state.lifecycle_sessions)?;
    let record = registry
        .get(&session_id)
        .filter(|r| &r.owner == owner && r.phase() == SessionPhase::Ready)
        .ok_or_else(|| error("agent_session_unavailable"))?;
    let mut info = session_info(state, &session_id, record)?
        .ok_or_else(|| error("agent_session_unsupported"))?;
    info.tab_id = tab_id;
    info.name = label;
    locked(&state.agent_bridge.inner)?.registrations.insert(
        session_id,
        Registration {
            owner: owner.clone(),
            info: info.clone(),
            sink,
        },
    );
    Ok(info)
}

pub fn unregister_session(
    state: &AppState,
    owner: &SessionOwner,
    session_id: &str,
) -> AppResult<()> {
    state.agent_bridge.close_session(owner, session_id)
}

pub fn list_sessions(state: &AppState, owner: &SessionOwner) -> AppResult<Vec<AgentSession>> {
    list_sessions_inner(state, owner, None)
}

pub fn list_mcp_sessions(
    state: &AppState,
    owner: &SessionOwner,
    token: &str,
) -> AppResult<Vec<AgentSession>> {
    list_sessions_inner(state, owner, Some(token))
}

fn list_sessions_inner(
    state: &AppState,
    owner: &SessionOwner,
    token: Option<&str>,
) -> AppResult<Vec<AgentSession>> {
    let registry = locked(&state.lifecycle_sessions)?;
    let inner = locked(&state.agent_bridge.inner)?;
    if token.is_some_and(|token| !endpoint_matches(&inner, owner, token)) {
        return Err(error("agent_endpoint_stopped"));
    }
    let mut sessions = Vec::new();
    for (id, record) in registry
        .iter()
        .filter(|(_, r)| &r.owner == owner && r.phase() == SessionPhase::Ready)
    {
        let Some(mut info) = session_info(state, id, record)? else {
            continue;
        };
        if let Some(registration) = inner
            .registrations
            .get(id)
            .filter(|r| &r.owner == owner && r.info.nonce == info.nonce)
        {
            info.tab_id = registration.info.tab_id.clone();
            info.name = registration.info.name.clone();
        }
        sessions.push(info);
    }
    sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    Ok(sessions)
}

pub fn pending_sessions(state: &AppState, owner: &SessionOwner) -> AppResult<Vec<SessionRequest>> {
    let inner = locked(&state.agent_bridge.inner)?;
    let mut requests: Vec<_> = inner
        .pending
        .values()
        .filter(|p| &p.owner == owner)
        .map(|p| p.request.clone())
        .collect();
    requests.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(requests)
}

struct OpenGuard<'a> {
    bridge: &'a AgentBridge,
    id: String,
}
impl Drop for OpenGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.bridge.inner.lock() {
            close_requests(
                &mut inner,
                |r| r.request.id == self.id,
                "agent_request_cancelled",
            );
        }
    }
}

pub async fn open_mcp_session(
    state: &AppState,
    owner: &SessionOwner,
    token: &str,
    input: OpenSessionInput,
) -> AppResult<OpenSessionResult> {
    if input.title.trim().is_empty() || input.title.len() > 512 {
        return Err(error("agent_invalid_session"));
    }
    let (request, sink, mut receiver, is_new) = {
        let registry = locked(&state.lifecycle_sessions)?;
        let mut inner = locked(&state.agent_bridge.inner)?;
        if !endpoint_matches(&inner, owner, token) {
            return Err(error("agent_endpoint_stopped"));
        }
        let registration = inner
            .registrations
            .get(&input.session_id)
            .filter(|r| &r.owner == owner && valid_session(&r.info, owner, &registry))
            .ok_or_else(|| error("agent_frontend_unavailable"))?;
        let info = registration.info.clone();
        let sink = registration.sink.clone();
        if let Some(binding) = inner.bindings.get(&input.session_id).filter(|b| {
            &b.owner == owner && valid_session(&b.info, owner, &registry) && !*b.shutdown.borrow()
        }) {
            return Ok(binding.opened.clone());
        }
        if let Some(pending) = inner
            .pending
            .values()
            .find(|p| &p.owner == owner && p.request.session_id == input.session_id)
        {
            if pending.request.title != input.title {
                return Err(error("agent_session_busy"));
            }
            (
                pending.request.clone(),
                sink,
                pending.result.subscribe(),
                false,
            )
        } else {
            let request = SessionRequest {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: input.session_id,
                tab_id: info.tab_id,
                nonce: info.nonce,
                kind: info.kind,
                title: input.title,
            };
            let (result, receiver) = watch::channel(None);
            inner.pending.insert(
                request.id.clone(),
                PendingSession {
                    owner: owner.clone(),
                    request: request.clone(),
                    sink: sink.clone(),
                    accepting: false,
                    binding: None,
                    result,
                },
            );
            (request, sink, receiver, true)
        }
    };
    let _guard = is_new.then(|| OpenGuard {
        bridge: &state.agent_bridge,
        id: request.id.clone(),
    });
    if is_new
        && !sink(
            &format!("agent:session_requested:{}", request.session_id),
            serde_json::to_value(&request).expect("session request serializes"),
        )
    {
        return Err(error("agent_frontend_unavailable"));
    }
    tokio::time::timeout(APPROVAL_TIMEOUT, async {
        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result;
            }
            receiver
                .changed()
                .await
                .map_err(|_| error("agent_session_unavailable"))?;
        }
    })
    .await
    .map_err(|_| error("agent_session_timeout"))?
}

pub async fn accept_session(
    state: &AppState,
    host: Host,
    owner: &SessionOwner,
    request_id: &str,
) -> AppResult<AiSessionInfo> {
    let request = {
        let registry = locked(&state.lifecycle_sessions)?;
        let mut inner = locked(&state.agent_bridge.inner)?;
        let pending = inner
            .pending
            .get_mut(request_id)
            .filter(|p| &p.owner == owner)
            .ok_or_else(|| error("agent_session_unavailable"))?;
        let current = registry.get(&pending.request.session_id).is_some_and(|r| {
            &r.owner == owner
                && r.phase() == SessionPhase::Ready
                && r.nonce.to_string() == pending.request.nonce
        });
        if !current || pending.accepting {
            return Err(error("agent_session_unavailable"));
        }
        pending.accepting = true;
        pending.request.clone()
    };
    let target = match request.kind.as_str() {
        "ssh" => crate::ai::commands::AiTarget::Ssh(request.session_id.clone()),
        "local" => crate::ai::commands::AiTarget::Local(request.session_id.clone()),
        "serial" => crate::ai::commands::AiTarget::Serial(request.session_id.clone()),
        "telnet" => crate::ai::commands::AiTarget::Telnet(request.session_id.clone()),
        _ => return Err(error("agent_session_unsupported")),
    };
    let info = match crate::ai::commands::ai_mcp_session_start_impl(
        state,
        host,
        owner.clone(),
        request.tab_id.clone(),
        target,
        request.title.clone(),
    )
    .await
    {
        Ok(info) => info,
        Err(err) => {
            let mut inner = locked(&state.agent_bridge.inner)?;
            close_requests(&mut inner, |p| p.request.id == request_id, err.code());
            return Err(err);
        }
    };
    let attached = {
        let shutdown = locked(&state.ai_sessions)?
            .get(&request.tab_id)
            .filter(|session| session.instance_id.to_string() == info.instance_id)
            .map(|session| session.shutdown_tx.clone());
        let registry = locked(&state.lifecycle_sessions)?;
        let mut inner = locked(&state.agent_bridge.inner)?;
        let registration = inner
            .registrations
            .get(&request.session_id)
            .filter(|r| {
                &r.owner == owner
                    && r.info.nonce == request.nonce
                    && valid_session(&r.info, owner, &registry)
            })
            .map(|r| r.info.clone());
        match (
            inner
                .pending
                .get_mut(request_id)
                .filter(|p| &p.owner == owner),
            registration,
            shutdown,
        ) {
            (Some(pending), Some(session), Some(shutdown)) => {
                pending.binding = Some(Binding {
                    owner: owner.clone(),
                    info: session,
                    opened: OpenSessionResult {
                        session_id: request.session_id,
                        conversation_id: info.conversation_id.clone(),
                        instance_id: info.instance_id.clone(),
                    },
                    shutdown,
                });
                true
            }
            _ => false,
        }
    };
    if attached {
        return Ok(info);
    }
    // Cancellation can win while the actor starts. No frontend has adopted
    // this actor yet, so complete its lifecycle cleanup here, not just shutdown.
    if let Err(cleanup) = crate::commands::lifecycle::close_ai_session(
        state,
        &request.tab_id,
        owner,
        Some(&info.instance_id),
    )
    .await
    {
        if cleanup.code() != "ai_session_not_found" {
            return Err(cleanup);
        }
    }
    Err(error("agent_session_unavailable"))
}

/// Called only after the AI timeline has subscribed to its normal events.
pub fn session_ready(
    state: &AppState,
    owner: &SessionOwner,
    request_id: &str,
    instance_id: &str,
) -> AppResult<()> {
    let registry = locked(&state.lifecycle_sessions)?;
    let mut inner = locked(&state.agent_bridge.inner)?;
    let pending = inner
        .pending
        .get(request_id)
        .filter(|p| &p.owner == owner)
        .ok_or_else(|| error("agent_session_unavailable"))?;
    let binding = pending
        .binding
        .as_ref()
        .filter(|b| {
            b.opened.instance_id == instance_id
                && valid_session(&b.info, owner, &registry)
                && !*b.shutdown.borrow()
        })
        .ok_or_else(|| error("agent_session_unavailable"))?;
    let session_id = binding.info.session_id.clone();
    let mut pending = inner.pending.remove(request_id).expect("validated above");
    let binding = pending.binding.take().expect("validated above");
    pending
        .result
        .send_replace(Some(Ok(binding.opened.clone())));
    inner.bindings.insert(session_id, binding);
    Ok(())
}

pub fn reject_session(state: &AppState, owner: &SessionOwner, request_id: &str) -> AppResult<()> {
    let mut inner = locked(&state.agent_bridge.inner)?;
    if inner
        .pending
        .get(request_id)
        .is_some_and(|p| &p.owner != owner)
    {
        return Err(error("agent_session_unavailable"));
    }
    close_requests(
        &mut inner,
        |p| p.request.id == request_id && &p.owner == owner,
        "agent_session_rejected",
    );
    Ok(())
}

struct CommandGuard<'a> {
    bridge: &'a AgentBridge,
    id: String,
}
impl Drop for CommandGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.bridge.inner.lock() {
            inner.commands.remove(&self.id);
        }
    }
}

pub async fn run_mcp_command(
    state: &AppState,
    owner: &SessionOwner,
    token: &str,
    input: RunCommandInput,
) -> AppResult<CommandResult> {
    if input.cmd.len() > MAX_COMMAND_BYTES
        || input.explain.len() > 4096
        || input.side_effect.len() > 4096
    {
        return Err(error("agent_invalid_command"));
    }
    let command = crate::ai::tools::RunCommandInput {
        cmd: input.cmd,
        explain: input.explain,
        side_effect: input.side_effect,
        timeout_s: input.timeout_s.map(|value| value.clamp(1, 300) as u32),
    };
    let result = call_mcp_tool(
        state,
        owner,
        token,
        &input.session_id,
        ExternalToolInput::RunCommand(command),
    )
    .await?;
    let ExternalToolResult::RunCommand(command) = result else {
        unreachable!("run_command returns its typed result");
    };
    Ok(CommandResult {
        session_id: input.session_id,
        command,
    })
}

pub async fn download_mcp_file(
    state: &AppState,
    owner: &SessionOwner,
    token: &str,
    input: DownloadFileInput,
) -> AppResult<DownloadResult> {
    let result = call_mcp_tool(
        state,
        owner,
        token,
        &input.session_id,
        ExternalToolInput::DownloadFile(crate::ai::tools::DownloadFileInput {
            remote_path: input.remote_path,
            max_mb: input.max_mb,
        }),
    )
    .await?;
    let ExternalToolResult::DownloadFile(download) = result else {
        unreachable!("download_file returns its typed result");
    };
    Ok(DownloadResult {
        session_id: input.session_id,
        download,
    })
}

async fn call_mcp_tool(
    state: &AppState,
    owner: &SessionOwner,
    token: &str,
    session_id: &str,
    input: ExternalToolInput,
) -> AppResult<ExternalToolResult> {
    let (id, tab_id, instance_id, mut receiver) = {
        let registry = locked(&state.lifecycle_sessions)?;
        let mut inner = locked(&state.agent_bridge.inner)?;
        if !endpoint_matches(&inner, owner, token) {
            return Err(error("agent_endpoint_stopped"));
        }
        let binding = inner
            .bindings
            .get(session_id)
            .filter(|b| {
                &b.owner == owner
                    && valid_session(&b.info, owner, &registry)
                    && !*b.shutdown.borrow()
            })
            .ok_or_else(|| error("agent_session_unavailable"))?;
        let tab_id = binding.info.tab_id.clone();
        let instance_id = binding.opened.instance_id.clone();
        if inner.commands.values().any(|c| c.session_id == session_id) {
            return Err(error("agent_session_busy"));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let (cancelled, receiver) = watch::channel(false);
        inner.commands.insert(
            id.clone(),
            Command {
                owner: owner.clone(),
                instance_id: instance_id.clone(),
                session_id: session_id.into(),
                cancelled,
            },
        );
        (id, tab_id, instance_id, receiver)
    };
    let _guard = CommandGuard {
        bridge: &state.agent_bridge,
        id: id.clone(),
    };
    let execution =
        crate::ai::commands::ai_mcp_tool_impl(state, &tab_id, owner, &instance_id, id, input);
    tokio::select! {
        result = execution => result,
        _ = receiver.changed() => Err(error("agent_request_cancelled")),
    }
}

#[cfg(all(test, any(windows, macos, linux, ohos)))]
mod tests {
    use super::*;
    use crate::ai::commands;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    struct Fixture {
        host: Host,
        owner: SessionOwner,
        info: AgentSession,
        events: mpsc::UnboundedReceiver<(String, serde_json::Value)>,
    }

    impl Fixture {
        fn new() -> Self {
            let (host, events) = crate::ssh::test_support::headless_host();
            let target = host.clone();
            let sink: Sink = Arc::new(move |event, payload| target.emit(event, payload).is_ok());
            let owner = SessionOwner::Headless(uuid::Uuid::new_v4());
            let id = crate::commands::pty::pty_spawn(
                host.clone(),
                owner.clone(),
                host.state(),
                80,
                24,
                None,
            )
            .unwrap();
            let info = register_session(
                host.state(),
                &owner,
                sink,
                id,
                "local:test".into(),
                "Test shell".into(),
            )
            .unwrap();
            let (shutdown, _) = watch::channel(false);
            host.state()
                .agent_bridge
                .reserve_endpoint(&owner)
                .unwrap()
                .commit(
                    EndpointInfo {
                        url: "http://127.0.0.1:1/mcp".into(),
                        token: "test-token".into(),
                    },
                    shutdown,
                )
                .unwrap();
            Self {
                host,
                owner,
                info,
                events,
            }
        }

        async fn event(&mut self, prefix: &str) -> serde_json::Value {
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let (event, payload) = self.events.recv().await.unwrap();
                    if event.starts_with(prefix) {
                        return payload;
                    }
                }
            })
            .await
            .unwrap()
        }

        async fn open(&mut self) -> OpenSessionResult {
            let host = self.host.clone();
            let owner = self.owner.clone();
            let input = OpenSessionInput {
                session_id: self.info.session_id.clone(),
                title: "Investigate the terminal".into(),
            };
            let opening = open_mcp_session(host.state(), &owner, "test-token", input);
            tokio::pin!(opening);
            let request = tokio::select! {
                result = &mut opening => panic!("must request permission: {result:?}"),
                request = self.event("agent:session_requested:") => request,
            };
            let id = request["id"].as_str().unwrap();
            let info = accept_session(host.state(), host.clone(), &owner, id)
                .await
                .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(5), &mut opening)
                    .await
                    .is_err(),
                "open_session must wait for frontend subscriptions"
            );
            session_ready(host.state(), &owner, id, &info.instance_id).unwrap();
            opening.await.unwrap()
        }

        fn input(&self) -> RunCommandInput {
            RunCommandInput {
                session_id: self.info.session_id.clone(),
                cmd: "echo sample".into(),
                explain: "Inspect terminal output".into(),
                side_effect: "read-only".into(),
                timeout_s: None,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            crate::commands::lifecycle::close_owner(self.host.state(), &self.owner);
        }
    }

    #[tokio::test]
    async fn discovery_includes_unregistered_terminals_without_granting_control() {
        let fixture = Fixture::new();
        unregister_session(
            fixture.host.state(),
            &fixture.owner,
            &fixture.info.session_id,
        )
        .unwrap();
        let listed = list_mcp_sessions(fixture.host.state(), &fixture.owner, "test-token").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, fixture.info.session_id);
        assert!(listed[0].tab_id.is_empty());
        assert!(list_sessions(
            fixture.host.state(),
            &SessionOwner::Headless(uuid::Uuid::new_v4())
        )
        .unwrap()
        .is_empty());
        let error = run_mcp_command(
            fixture.host.state(),
            &fixture.owner,
            "test-token",
            fixture.input(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "agent_session_unavailable");
    }

    #[tokio::test]
    async fn permission_is_owner_scoped_and_cancellation_removes_late_accept() {
        let mut fixture = Fixture::new();
        let host = fixture.host.clone();
        let owner = fixture.owner.clone();
        let input = OpenSessionInput {
            session_id: fixture.info.session_id.clone(),
            title: "Inspect".into(),
        };
        let mut opening = Box::pin(open_mcp_session(host.state(), &owner, "test-token", input));
        let request = tokio::select! { _ = &mut opening => panic!("must wait"), request = fixture.event("agent:session_requested:") => request };
        let id = request["id"].as_str().unwrap();
        assert!(reject_session(
            host.state(),
            &SessionOwner::Headless(uuid::Uuid::new_v4()),
            id
        )
        .is_err());
        drop(opening);
        assert!(pending_sessions(host.state(), &owner).unwrap().is_empty());
        assert!(accept_session(host.state(), host.clone(), &owner, id)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn accepted_commands_use_ai_events_redaction_history_and_release_request() {
        let mut fixture = Fixture::new();
        crate::ai::redact_rules::save(
            &fixture.host.state().db,
            &crate::ai::redact_rules::RedactRuleRecord {
                id: "mcp-test".into(),
                pattern: "private-value".into(),
                replacement: "[hidden]".into(),
            },
        )
        .unwrap();
        let opened = fixture.open().await;
        let host = fixture.host.clone();
        let owner = fixture.owner.clone();
        let input = fixture.input();
        let running = run_mcp_command(host.state(), &owner, "test-token", input.clone());
        tokio::pin!(running);
        let proposal = tokio::select! { _ = &mut running => panic!("must wait for shared AI approval"), proposal = fixture.event("ai:command_proposed:") => proposal };
        let command_id = proposal["id"]
            .as_str()
            .or_else(|| proposal["command_id"].as_str())
            .unwrap();
        let busy = run_mcp_command(host.state(), &owner, "test-token", fixture.input())
            .await
            .unwrap_err();
        assert_eq!(busy.code(), "agent_session_busy");
        let busy_download = download_mcp_file(
            host.state(),
            &owner,
            "test-token",
            DownloadFileInput {
                session_id: fixture.info.session_id.clone(),
                remote_path: "/tmp/sample.log".into(),
                max_mb: 1,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(busy_download.code(), "agent_session_busy");
        commands::ai_command_result_impl(
            host.state(),
            &fixture.info.tab_id,
            &owner,
            command_id.into(),
            0,
            "private-value\nsample\n".into(),
            false,
            false,
            Some(&opened.instance_id),
        )
        .await
        .unwrap();
        let result = running.await.unwrap();
        assert_eq!(result.command.output, "[hidden]\nsample\n");
        assert!(locked(&host.state().agent_bridge.inner)
            .unwrap()
            .commands
            .is_empty());
        let row = crate::db::ai_conversation::get(&host.state().db, &opened.conversation_id)
            .unwrap()
            .unwrap();
        assert_eq!(row.history_json, "[]");
        assert!(row.audit_json.contains("sample"));
        assert!(!row.audit_json.contains("private-value"));
        assert!(
            row.audit_json.contains("command_executed")
                || row.audit_json.contains("CommandExecuted")
        );
    }

    #[tokio::test]
    async fn downloads_require_session_acceptance_and_release_failed_requests() {
        let mut fixture = Fixture::new();
        let input = DownloadFileInput {
            session_id: fixture.info.session_id.clone(),
            remote_path: "/tmp/sample.log".into(),
            max_mb: 1,
        };
        let denied = download_mcp_file(
            fixture.host.state(),
            &fixture.owner,
            "test-token",
            input.clone(),
        )
        .await
        .unwrap_err();
        assert_eq!(denied.code(), "agent_session_unavailable");

        fixture.open().await;
        let result = download_mcp_file(fixture.host.state(), &fixture.owner, "test-token", input)
            .await
            .unwrap();
        assert_eq!(result.session_id, fixture.info.session_id);
        assert!(!result.download.ok, "a local shell has no SFTP connection");
        assert!(result.download.local_path.is_none());
        assert!(locked(&fixture.host.state().agent_bridge.inner)
            .unwrap()
            .commands
            .is_empty());
    }

    #[tokio::test]
    async fn stop_endpoint_cancels_actor_request_and_revokes_control() {
        let mut fixture = Fixture::new();
        let opened = fixture.open().await;
        let host = fixture.host.clone();
        let owner = fixture.owner.clone();
        let running = run_mcp_command(host.state(), &owner, "test-token", fixture.input());
        tokio::pin!(running);
        tokio::select! { _ = &mut running => panic!("must wait"), _ = fixture.event("ai:command_proposed:") => () };
        host.state().agent_bridge.stop_endpoint(&owner).unwrap();
        assert!(running.await.is_err());
        let sessions = locked(&host.state().ai_sessions).unwrap();
        assert!(sessions
            .get(&fixture.info.tab_id)
            .is_none_or(
                |session| session.instance_id.to_string() != opened.instance_id
                    || session.is_stopping()
            ));
    }

    #[tokio::test]
    async fn ai_close_revokes_binding_until_a_new_session_is_accepted() {
        let mut fixture = Fixture::new();
        let opened = fixture.open().await;
        let host = fixture.host.clone();
        let owner = fixture.owner.clone();
        let input = fixture.input();
        let running = run_mcp_command(host.state(), &owner, "test-token", input.clone());
        tokio::pin!(running);
        let proposal = tokio::select! { _ = &mut running => panic!("must wait"), proposal = fixture.event("ai:command_proposed:") => proposal };
        commands::ai_command_reject_impl(
            host.state(),
            &fixture.info.tab_id,
            &owner,
            proposal["id"].as_str().unwrap().into(),
            "User declined".into(),
            Some(&opened.instance_id),
        )
        .await
        .unwrap();
        running.await.unwrap();
        fixture.event("ai:command_rejected:").await;
        crate::commands::lifecycle::close_ai_session(
            host.state(),
            &fixture.info.tab_id,
            &owner,
            Some(&opened.instance_id),
        )
        .await
        .unwrap();
        assert_eq!(
            run_mcp_command(host.state(), &owner, "test-token", fixture.input())
                .await
                .unwrap_err()
                .code(),
            "agent_session_unavailable"
        );
        let replacement = fixture.open().await;
        assert_ne!(replacement.instance_id, opened.instance_id);
        let mut running = Box::pin(run_mcp_command(host.state(), &owner, "test-token", input));
        tokio::select! { _ = &mut running => panic!("newly accepted session must allow a proposal"), _ = fixture.event("ai:command_proposed:") => () };
        drop(running);
        fixture.event("ai:command_rejected:").await;
        assert!(locked(&host.state().agent_bridge.inner)
            .unwrap()
            .commands
            .is_empty());
    }

    #[tokio::test]
    async fn replacement_terminal_nonce_cannot_inherit_an_accepted_conversation() {
        let mut fixture = Fixture::new();
        fixture.open().await;
        let host = fixture.host.clone();
        locked(&host.state().lifecycle_sessions)
            .unwrap()
            .get_mut(&fixture.info.session_id)
            .unwrap()
            .nonce = uuid::Uuid::new_v4();
        assert_eq!(
            run_mcp_command(host.state(), &fixture.owner, "test-token", fixture.input())
                .await
                .unwrap_err()
                .code(),
            "agent_session_unavailable"
        );
        let listed = list_sessions(host.state(), &fixture.owner).unwrap();
        assert_eq!(listed.len(), 1);
        assert!(listed[0].tab_id.is_empty());
        assert_ne!(listed[0].nonce, fixture.info.nonce);
    }

    #[tokio::test]
    async fn discovery_includes_raw_devices_and_excludes_nonterminal_resources() {
        let fixture = Fixture::new();
        let state = fixture.host.state();
        for (id, kind, phase) in [
            (
                "serial-test",
                SessionKind::Serial,
                crate::state::ResourceState::Ready,
            ),
            (
                "telnet-test",
                SessionKind::Telnet,
                crate::state::ResourceState::Ready,
            ),
            (
                "forward-test",
                SessionKind::Forward,
                crate::state::ResourceState::Ready,
            ),
            (
                "pending-test",
                SessionKind::Telnet,
                crate::state::ResourceState::Pending(None),
            ),
        ] {
            locked(&state.lifecycle_sessions).unwrap().insert(
                id.into(),
                SessionRecord {
                    nonce: uuid::Uuid::new_v4(),
                    kind,
                    owner: fixture.owner.clone(),
                    state: phase,
                    parent: None,
                },
            );
        }
        let listed = list_sessions(state, &fixture.owner).unwrap();
        assert_eq!(listed.len(), 3);
        assert!(listed
            .iter()
            .any(|s| s.kind == "serial" && s.shell.kind == ShellKind::Serial));
        assert!(listed
            .iter()
            .any(|s| s.kind == "telnet" && s.shell.kind == ShellKind::Telnet));
    }
}
