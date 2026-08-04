use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use russh::Disconnect;
use serde::Serialize;
use serde_json::json;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

use crate::error::{locked, AppError, AppResult};
use crate::models::Profile;
use crate::models::{Credential, Forward, ForwardRule, ForwardType};
use crate::ssh::client::{self, LogFn};
use std::sync::Arc as StdArc;

/// Everything needed to open an authenticated SSH connection to a forward's
/// target host through its bastion chain. `start_local` / `start_remote` /
/// `start_dynamic` all took these six parameters identically — they are one
/// concept ("how to reach the endpoint"), so they travel as one value.
pub struct ConnTarget {
    pub profile: Profile,
    pub credential: Credential,
    pub bastion_chain: Vec<(Profile, Credential)>,
    pub known_hosts_path: PathBuf,
    pub timeout_secs: u64,
}

pub struct ForwardHandle {
    /// Present while this value owns the detached task. `stop()` transfers
    /// cancellation to the grace-period timer; an ordinary drop aborts now.
    abort: Option<tokio::task::AbortHandle>,
    /// Notify-based disconnect signal. `stop()` fires this; the accept-loop
    /// task `select!`s on it and runs `handle.disconnect(...)` before
    /// breaking out. Without this, abort()ing the task drops the future
    /// holding the SSH `Handle` — russh never sends the `SSH_MSG_DISCONNECT`
    /// and the server leaks a half-open session until TCP keepalive expires.
    disconnect: Arc<Notify>,
    rules: Vec<Arc<RuleControl>>,
    commands: tokio::sync::mpsc::UnboundedSender<RuleCommand>,
    connected: Arc<std::sync::atomic::AtomicBool>,
}

impl ForwardHandle {
    fn from_task(
        task: tokio::task::JoinHandle<()>,
        disconnect: Arc<Notify>,
        rules: Vec<Arc<RuleControl>>,
        commands: tokio::sync::mpsc::UnboundedSender<RuleCommand>,
        connected: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            abort: Some(task.abort_handle()),
            disconnect,
            rules,
            commands,
            connected,
        }
    }

    pub fn stop(mut self) {
        self.disconnect.notify_one();
        // Give the task up to 2 s to send the disconnect message before
        // we force-abort. Picked over a hard sync-wait so `stop()` stays
        // non-blocking for the Tauri command thread; picked over no abort
        // at all so a wedged disconnect await (e.g. dead remote) can't
        // strand the forward task forever.
        if let Some(abort) = self.abort.take() {
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                abort.abort();
            });
        }
    }
}

impl Drop for ForwardHandle {
    fn drop(&mut self) {
        if let Some(abort) = self.abort.take() {
            abort.abort();
        }
    }
}

#[derive(Serialize)]
pub struct ForwardStats {
    pub bytes_tx: u64,
    pub bytes_rx: u64,
    pub connections: u32,
    pub connected: bool,
    pub rules: Vec<ForwardRuleStats>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardRuleStatus {
    Starting,
    Active,
    Stopped,
    Error,
    StoppingError,
}

#[derive(Serialize)]
pub struct ForwardRuleStats {
    pub index: usize,
    pub rule: ForwardRule,
    pub status: ForwardRuleStatus,
    pub bytes_tx: u64,
    pub bytes_rx: u64,
    pub connections: u32,
    pub effective_port: Option<u32>,
    pub error: Option<String>,
}

struct RuleState {
    status: ForwardRuleStatus,
    effective_port: Option<u32>,
    error: Option<String>,
}

struct RuleControl {
    rule: ForwardRule,
    bytes_tx: Arc<AtomicU64>,
    bytes_rx: Arc<AtomicU64>,
    connections: Arc<AtomicU32>,
    state: Mutex<RuleState>,
}

impl RuleControl {
    fn new(rule: ForwardRule) -> Self {
        Self {
            rule,
            bytes_tx: Arc::new(AtomicU64::new(0)),
            bytes_rx: Arc::new(AtomicU64::new(0)),
            connections: Arc::new(AtomicU32::new(0)),
            state: Mutex::new(RuleState {
                status: ForwardRuleStatus::Starting,
                effective_port: None,
                error: None,
            }),
        }
    }

    fn set_state(
        &self,
        status: ForwardRuleStatus,
        effective_port: Option<u32>,
        error: Option<String>,
    ) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.status = status;
        state.effective_port = effective_port;
        state.error = error;
    }
}

enum RuleCommand {
    Start {
        index: usize,
        reply: tokio::sync::oneshot::Sender<AppResult<()>>,
    },
    Stop {
        index: usize,
        reply: tokio::sync::oneshot::Sender<AppResult<()>>,
    },
}

/// Bind a local-forward listener on loopback for both families. IPv4
/// (127.0.0.1) is required; IPv6 (::1) is best-effort and simply skipped on
/// hosts without an IPv6 stack. Both stay loopback-only — a local forward is
/// never exposed to the network.
async fn bind_loopback(port: u16) -> AppResult<(TcpListener, Option<TcpListener>)> {
    let v4 = TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| {
        AppError::ssh(
            "ssh_port_bind_failed",
            json!({ "port": port, "err": e.to_string() }),
        )
    })?;
    // Bind v6 to v4's *actual* port so an ephemeral request (port 0) lands on
    // the same port for both families. If v4's port can't be read, skip v6
    // rather than risk binding it to a different (e.g. random) port.
    let v6 = match v4.local_addr() {
        Ok(a) => TcpListener::bind(("::1", a.port())).await.ok(),
        Err(_) => None,
    };
    Ok((v4, v6))
}

/// `accept()` on an optional listener; pends forever when the listener is
/// absent so it can sit in a `select!` arm that never fires.
async fn accept_opt(
    listener: &Option<TcpListener>,
) -> std::io::Result<(TcpStream, std::net::SocketAddr)> {
    match listener {
        Some(l) => l.accept().await,
        None => std::future::pending().await,
    }
}

impl ForwardHandle {
    pub fn stats(&self) -> ForwardStats {
        let rules: Vec<_> = self
            .rules
            .iter()
            .enumerate()
            .map(|(index, control)| {
                let state = control.state.lock().unwrap_or_else(|e| e.into_inner());
                ForwardRuleStats {
                    index,
                    rule: control.rule.clone(),
                    status: state.status,
                    bytes_tx: control.bytes_tx.load(Ordering::Relaxed),
                    bytes_rx: control.bytes_rx.load(Ordering::Relaxed),
                    connections: control.connections.load(Ordering::Relaxed),
                    effective_port: state.effective_port,
                    error: state.error.clone(),
                }
            })
            .collect();
        ForwardStats {
            bytes_tx: rules.iter().map(|rule| rule.bytes_tx).sum(),
            bytes_rx: rules.iter().map(|rule| rule.bytes_rx).sum(),
            connections: rules.iter().map(|rule| rule.connections).sum(),
            connected: self.connected.load(Ordering::Relaxed),
            rules,
        }
    }

    pub fn request_rule_start(
        &self,
        index: usize,
    ) -> AppResult<tokio::sync::oneshot::Receiver<AppResult<()>>> {
        self.request_rule_command(index, true)
    }

    pub fn request_rule_stop(
        &self,
        index: usize,
    ) -> AppResult<tokio::sync::oneshot::Receiver<AppResult<()>>> {
        self.request_rule_command(index, false)
    }

    fn request_rule_command(
        &self,
        index: usize,
        start: bool,
    ) -> AppResult<tokio::sync::oneshot::Receiver<AppResult<()>>> {
        let (reply, response) = tokio::sync::oneshot::channel();
        let command = if start {
            RuleCommand::Start { index, reply }
        } else {
            RuleCommand::Stop { index, reply }
        };
        self.commands
            .send(command)
            .map_err(|_| AppError::ssh("fwd_session_closed", json!({})))?;
        Ok(response)
    }
}

async fn counted_copy<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    counter: &AtomicU64,
) -> std::io::Result<()> {
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n]).await?;
        counter.fetch_add(n as u64, Ordering::Relaxed);
    }
    Ok(())
}

/// Open an authenticated SSH connection to a forward's endpoint through its
/// bastion chain. `start_local` / `start_remote` / `start_dynamic` all opened
/// the connection with this exact prologue; keeping the dial policy
/// (known_hosts / timeout / log / prompt-ctx) in one place stops the three
/// from drifting apart. `start_remote` is the only caller that uses the
/// returned remote-channel router dispatches incoming `-R` channels by port.
async fn connect_authed(
    target: ConnTarget,
) -> AppResult<(
    russh::client::Handle<client::SshHandler>,
    client::ForwardedChannelRouter,
)> {
    let ConnTarget {
        profile,
        credential,
        bastion_chain,
        known_hosts_path,
        timeout_secs,
    } = target;
    let log: LogFn = StdArc::new(|_: String| ());
    let (mut handle, fwd_sender) = client::establish_via_chain(
        bastion_chain,
        profile,
        known_hosts_path,
        timeout_secs,
        log,
        None,
    )
    .await?;
    client::authenticate(&mut handle, credential, None).await?;
    Ok((handle, fwd_sender))
}

type SharedHandle = Arc<russh::client::Handle<client::SshHandler>>;
const SOCKS5_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

enum PreparedRule {
    Local(ForwardRule, TcpListener, Option<TcpListener>),
    Remote(
        ForwardRule,
        u32,
        tokio::sync::mpsc::UnboundedReceiver<russh::Channel<russh::client::Msg>>,
    ),
    Dynamic(ForwardRule, TcpListener, Option<TcpListener>),
}

async fn bridge(
    tcp_stream: TcpStream,
    ssh_stream: impl AsyncRead + AsyncWrite + Unpin,
    tx: Arc<AtomicU64>,
    rx: Arc<AtomicU64>,
    conns: Arc<AtomicU32>,
) {
    struct ConnectionGuard(Arc<AtomicU32>);
    impl Drop for ConnectionGuard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::Relaxed);
        }
    }
    let _connection = ConnectionGuard(conns);
    let (mut tcp_r, mut tcp_w) = tokio::io::split(tcp_stream);
    let (mut ssh_r, mut ssh_w) = tokio::io::split(ssh_stream);
    let _ = tokio::join!(
        counted_copy(&mut tcp_r, &mut ssh_w, &tx),
        counted_copy(&mut ssh_r, &mut tcp_w, &rx),
    );
}

async fn accept_connection(
    listener: &TcpListener,
    listener6: &Option<TcpListener>,
    port: u16,
) -> TcpStream {
    loop {
        let result = tokio::select! {
            result = listener.accept() => result,
            result = accept_opt(listener6) => result,
        };
        match result {
            Ok((stream, _)) => return stream,
            Err(error) => {
                log::warn!("forward listener on port {port} failed to accept: {error}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

async fn run_local_rule(
    rule: ForwardRule,
    listener: TcpListener,
    listener6: Option<TcpListener>,
    handle: SharedHandle,
    tx: Arc<AtomicU64>,
    rx: Arc<AtomicU64>,
    conns: Arc<AtomicU32>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
) {
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut stop => break,
            tcp_stream = accept_connection(&listener, &listener6, rule.local_port) => {
                let handle = handle.clone();
                let rule = rule.clone();
                let tx = tx.clone();
                let rx = rx.clone();
                let conns = conns.clone();
                tasks.spawn(async move {
                    let Ok(channel) = handle
                        .channel_open_direct_tcpip(
                            &rule.remote_host,
                            rule.remote_port as u32,
                            "127.0.0.1",
                            rule.local_port as u32,
                        )
                        .await else { return };
                    conns.fetch_add(1, Ordering::Relaxed);
                    bridge(tcp_stream, channel.into_stream(), tx, rx, conns).await;
                });
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
}

async fn run_remote_rule(
    rule: ForwardRule,
    mut channels: tokio::sync::mpsc::UnboundedReceiver<russh::Channel<russh::client::Msg>>,
    tx: Arc<AtomicU64>,
    rx: Arc<AtomicU64>,
    conns: Arc<AtomicU32>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
) {
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut stop => break,
            message = channels.recv() => {
                let Some(channel) = message else { break };
                let target = rule.remote_host.clone();
                let port = rule.local_port;
                let tx = tx.clone();
                let rx = rx.clone();
                let conns = conns.clone();
                tasks.spawn(async move {
                    let local = match TcpStream::connect((target.as_str(), port)).await {
                        Ok(stream) => stream,
                        Err(_) => {
                            let _ = channel.close().await;
                            return;
                        }
                    };
                    conns.fetch_add(1, Ordering::Relaxed);
                    bridge(local, channel.into_stream(), tx, rx, conns).await;
                });
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
}

// ---------------------------------------------------------------------------
// Dynamic SOCKS5 forwarding
// ---------------------------------------------------------------------------

/// Parse a SOCKS5 connection request and return (target_host, target_port).
async fn socks5_handshake(stream: &mut TcpStream) -> std::io::Result<(String, u16)> {
    // 1. Read greeting: version + nmethods + methods
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).await?;
    if header[0] != 0x05 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Not SOCKS5",
        ));
    }
    let nmethods = header[1] as usize;
    let mut methods = vec![0u8; nmethods];
    stream.read_exact(&mut methods).await?;

    // 2. Reply: no auth required
    stream.write_all(&[0x05, 0x00]).await?;

    // 3. Read connect request: ver(1) + cmd(1) + rsv(1) + atyp(1) + addr + port(2)
    let mut req = [0u8; 4];
    stream.read_exact(&mut req).await?;
    if req[0] != 0x05 || req[1] != 0x01 {
        // Only CONNECT (0x01) supported
        stream
            .write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await?;
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Only CONNECT supported",
        ));
    }

    let (host, port) = match req[3] {
        0x01 => {
            // IPv4
            let mut addr = [0u8; 4];
            stream.read_exact(&mut addr).await?;
            let mut port_buf = [0u8; 2];
            stream.read_exact(&mut port_buf).await?;
            let host = format!("{}.{}.{}.{}", addr[0], addr[1], addr[2], addr[3]);
            let port = u16::from_be_bytes(port_buf);
            (host, port)
        }
        0x03 => {
            // Domain name
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            let mut domain = vec![0u8; len[0] as usize];
            stream.read_exact(&mut domain).await?;
            let mut port_buf = [0u8; 2];
            stream.read_exact(&mut port_buf).await?;
            let host = String::from_utf8_lossy(&domain).to_string();
            let port = u16::from_be_bytes(port_buf);
            (host, port)
        }
        0x04 => {
            // IPv6
            let mut addr = [0u8; 16];
            stream.read_exact(&mut addr).await?;
            let mut port_buf = [0u8; 2];
            stream.read_exact(&mut port_buf).await?;
            let host = format!(
                "{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}",
                u16::from_be_bytes([addr[0], addr[1]]),
                u16::from_be_bytes([addr[2], addr[3]]),
                u16::from_be_bytes([addr[4], addr[5]]),
                u16::from_be_bytes([addr[6], addr[7]]),
                u16::from_be_bytes([addr[8], addr[9]]),
                u16::from_be_bytes([addr[10], addr[11]]),
                u16::from_be_bytes([addr[12], addr[13]]),
                u16::from_be_bytes([addr[14], addr[15]]),
            );
            let port = u16::from_be_bytes(port_buf);
            (host, port)
        }
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Unknown address type",
            ));
        }
    };

    // 4. Reply: success
    stream
        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await?;

    Ok((host, port))
}

async fn run_dynamic_rule(
    rule: ForwardRule,
    listener: TcpListener,
    listener6: Option<TcpListener>,
    handle: SharedHandle,
    tx: Arc<AtomicU64>,
    rx: Arc<AtomicU64>,
    conns: Arc<AtomicU32>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
) {
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut stop => break,
            tcp_stream = accept_connection(&listener, &listener6, rule.local_port) => {
                let handle = handle.clone();
                let tx = tx.clone();
                let rx = rx.clone();
                let conns = conns.clone();
                let local_port = rule.local_port;
                tasks.spawn(async move {
                    let mut tcp_stream = tcp_stream;
                    let Ok(Ok((host, port))) = tokio::time::timeout(
                        SOCKS5_HANDSHAKE_TIMEOUT,
                        socks5_handshake(&mut tcp_stream),
                    ).await else { return };
                    let Ok(channel) = handle
                        .channel_open_direct_tcpip(&host, port as u32, "127.0.0.1", local_port as u32)
                        .await else { return };
                    conns.fetch_add(1, Ordering::Relaxed);
                    bridge(tcp_stream, channel.into_stream(), tx, rx, conns).await;
                });
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
}

async fn prepare_remote_rule(
    rule: ForwardRule,
    handle: &russh::client::Handle<client::SshHandler>,
    routes: &client::ForwardedChannelRouter,
) -> AppResult<PreparedRule> {
    let requested_port = rule.remote_port as u32;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    if requested_port != 0 {
        {
            let mut guard = locked(routes)?;
            if guard.routes.contains_key(&requested_port) {
                return Err(AppError::config(
                    "fwd_duplicate_remote_port",
                    json!({ "port": requested_port }),
                ));
            }
            guard.routes.insert(requested_port, tx.clone());
        }
        if let Err(error) = handle.tcpip_forward("", requested_port).await {
            locked(routes)?.routes.remove(&requested_port);
            return Err(AppError::ssh(
                "ssh_tcpip_forward_failed",
                json!({ "err": error.to_string() }),
            ));
        }
        return Ok(PreparedRule::Remote(rule, requested_port, rx));
    }

    {
        let mut guard = locked(routes)?;
        if guard.pending.is_some() {
            return Err(AppError::config(
                "fwd_multiple_dynamic_remote_ports",
                json!({}),
            ));
        }
        guard.pending = Some(tx.clone());
    }
    match handle.tcpip_forward("", 0).await {
        Ok(bound_port) => {
            let mut guard = locked(routes)?;
            guard.pending = None;
            guard.routes.insert(bound_port, tx);
            Ok(PreparedRule::Remote(rule, bound_port, rx))
        }
        Err(error) => {
            locked(routes)?.pending = None;
            Err(AppError::ssh(
                "ssh_tcpip_forward_failed",
                json!({ "err": error.to_string() }),
            ))
        }
    }
}

async fn prepare_rule(
    rule: ForwardRule,
    handle: &russh::client::Handle<client::SshHandler>,
    routes: &client::ForwardedChannelRouter,
) -> AppResult<PreparedRule> {
    match rule.forward_type {
        ForwardType::Local => bind_loopback(rule.local_port)
            .await
            .map(|(v4, v6)| PreparedRule::Local(rule, v4, v6)),
        ForwardType::Dynamic => bind_loopback(rule.local_port)
            .await
            .map(|(v4, v6)| PreparedRule::Dynamic(rule, v4, v6)),
        ForwardType::Remote => prepare_remote_rule(rule, handle, routes).await,
    }
}

struct RunningRule {
    generation: u64,
    task: Option<tokio::task::JoinHandle<()>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    remote_port: Option<u32>,
}

fn completion_matches(
    running: &HashMap<usize, RunningRule>,
    index: usize,
    generation: u64,
) -> bool {
    running
        .get(&index)
        .is_some_and(|runtime| runtime.generation == generation && runtime.task.is_some())
}

fn spawn_rule(
    index: usize,
    generation: u64,
    runtime: PreparedRule,
    handle: SharedHandle,
    control: Arc<RuleControl>,
    completed: tokio::sync::mpsc::UnboundedSender<(usize, u64)>,
) -> RunningRule {
    let tx = control.bytes_tx.clone();
    let rx = control.bytes_rx.clone();
    let conns = control.connections.clone();
    let (stop, stop_rx) = tokio::sync::oneshot::channel();
    let (future, effective_port, remote_port): (
        std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
        Option<u32>,
        Option<u32>,
    ) = match runtime {
        PreparedRule::Local(rule, v4, v6) => {
            let port = v4.local_addr().ok().map(|address| address.port() as u32);
            (
                Box::pin(run_local_rule(rule, v4, v6, handle, tx, rx, conns, stop_rx)),
                port,
                None,
            )
        }
        PreparedRule::Dynamic(rule, v4, v6) => {
            let port = v4.local_addr().ok().map(|address| address.port() as u32);
            (
                Box::pin(run_dynamic_rule(
                    rule, v4, v6, handle, tx, rx, conns, stop_rx,
                )),
                port,
                None,
            )
        }
        PreparedRule::Remote(rule, port, channels) => (
            Box::pin(run_remote_rule(rule, channels, tx, rx, conns, stop_rx)),
            Some(port),
            Some(port),
        ),
    };
    control.set_state(ForwardRuleStatus::Active, effective_port, None);
    let task = tokio::spawn(async move {
        future.await;
        let _ = completed.send((index, generation));
    });
    RunningRule {
        generation,
        task: Some(task),
        stop: Some(stop),
        remote_port,
    }
}

async fn stop_running_rule(
    index: usize,
    running: &mut HashMap<usize, RunningRule>,
    controls: &[Arc<RuleControl>],
    handle: &russh::client::Handle<client::SshHandler>,
    routes: &client::ForwardedChannelRouter,
) -> AppResult<()> {
    let Some(mut runtime) = running.remove(&index) else {
        controls
            .get(index)
            .ok_or_else(|| AppError::not_found("fwd_rule_not_found", json!({ "index": index })))?
            .set_state(ForwardRuleStatus::Stopped, None, None);
        return Ok(());
    };
    if let Some(stop) = runtime.stop.take() {
        let _ = stop.send(());
    }
    if let Some(task) = runtime.task.take() {
        let _ = task.await;
    }
    controls[index].connections.store(0, Ordering::Relaxed);
    if let Some(port) = runtime.remote_port {
        locked(routes)?.routes.remove(&port);
        if let Err(error) = handle.cancel_tcpip_forward("", port).await {
            controls[index].set_state(
                ForwardRuleStatus::StoppingError,
                None,
                Some(
                    AppError::ssh(
                        "ssh_cancel_tcpip_forward_failed",
                        json!({ "err": error.to_string() }),
                    )
                    .to_string(),
                ),
            );
            running.insert(index, runtime);
            return Err(AppError::ssh(
                "ssh_cancel_tcpip_forward_failed",
                json!({ "err": error.to_string() }),
            ));
        }
    }
    controls[index].set_state(ForwardRuleStatus::Stopped, None, None);
    Ok(())
}

pub async fn start(forward: Forward, target: ConnTarget) -> AppResult<ForwardHandle> {
    if forward.rules.is_empty() {
        return Err(AppError::config("fwd_rules_empty", json!({})));
    }
    let (handle, routes) = connect_authed(target).await?;
    let controls: Vec<_> = forward
        .rules
        .iter()
        .cloned()
        .map(RuleControl::new)
        .map(Arc::new)
        .collect();
    let mut prepared = Vec::with_capacity(forward.rules.len());

    for (index, rule) in forward.rules.into_iter().enumerate() {
        match prepare_rule(rule, &handle, &routes).await {
            Ok(runtime) => prepared.push((index, runtime)),
            Err(error) => {
                controls[index].set_state(ForwardRuleStatus::Error, None, Some(error.to_string()))
            }
        }
    }

    let disconnect = Arc::new(Notify::new());
    let disconnect_task = disconnect.clone();
    let shared = Arc::new(handle);
    let task_handle = shared.clone();
    let task_controls = controls.clone();
    let connected = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let task_connected = connected.clone();
    let (commands, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
    let (completed, mut completed_rx) = tokio::sync::mpsc::unbounded_channel();

    let task = tokio::spawn(async move {
        let mut running = HashMap::new();
        let mut generation = 0u64;
        for (index, runtime) in prepared {
            generation += 1;
            running.insert(
                index,
                spawn_rule(
                    index,
                    generation,
                    runtime,
                    task_handle.clone(),
                    task_controls[index].clone(),
                    completed.clone(),
                ),
            );
        }
        let mut transport_check = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            tokio::select! {
                _ = disconnect_task.notified() => break,
                _ = transport_check.tick() => {
                    if task_handle.is_closed() {
                        task_connected.store(false, Ordering::Relaxed);
                        for (index, mut runtime) in running.drain() {
                            if let Some(stop) = runtime.stop.take() {
                                let _ = stop.send(());
                            }
                            if let Some(task) = runtime.task.take() {
                                let _ = task.await;
                            }
                            task_controls[index].connections.store(0, Ordering::Relaxed);
                            task_controls[index].set_state(
                                ForwardRuleStatus::Error,
                                None,
                                Some(AppError::ssh("ssh_disconnected", json!({})).to_string()),
                            );
                        }
                        break;
                    }
                }
                Some((index, completed_generation)) = completed_rx.recv() => {
                    if !completion_matches(&running, index, completed_generation) {
                        continue;
                    }
                    let Some(runtime) = running.remove(&index) else { continue };
                    if let Some(port) = runtime.remote_port {
                        if let Ok(mut router) = locked(&routes) {
                            router.routes.remove(&port);
                        }
                        if let Err(error) = task_handle.cancel_tcpip_forward("", port).await {
                            task_controls[index].set_state(
                                ForwardRuleStatus::StoppingError,
                                None,
                                Some(AppError::ssh(
                                    "ssh_cancel_tcpip_forward_failed",
                                    json!({ "err": error.to_string() }),
                                ).to_string()),
                            );
                            running.insert(index, RunningRule {
                                task: None,
                                stop: None,
                                ..runtime
                            });
                            continue;
                        }
                    }
                    task_controls[index].connections.store(0, Ordering::Relaxed);
                    task_controls[index].set_state(
                        ForwardRuleStatus::Error,
                        None,
                        Some(AppError::ssh("fwd_rule_stopped_unexpectedly", json!({})).to_string()),
                    );
                }
                Some(command) = command_rx.recv() => {
                    match command {
                        RuleCommand::Start { index, reply } => {
                            let result = if let Some(runtime) = running.get(&index) {
                                if runtime.task.is_some() {
                                    Ok(())
                                } else {
                                    Err(AppError::config("fwd_rule_stop_pending", json!({ "index": index })))
                                }
                            } else if let Some(control) = task_controls.get(index) {
                                control.set_state(ForwardRuleStatus::Starting, None, None);
                                match prepare_rule(control.rule.clone(), &task_handle, &routes).await {
                                    Ok(runtime) => {
                                        generation += 1;
                                        running.insert(
                                            index,
                                            spawn_rule(
                                                index,
                                                generation,
                                                runtime,
                                                task_handle.clone(),
                                                control.clone(),
                                                completed.clone(),
                                            ),
                                        );
                                        Ok(())
                                    }
                                    Err(error) => {
                                        control.set_state(
                                            ForwardRuleStatus::Error,
                                            None,
                                            Some(error.to_string()),
                                        );
                                        Err(error)
                                    }
                                }
                            } else {
                                Err(AppError::not_found("fwd_rule_not_found", json!({ "index": index })))
                            };
                            let _ = reply.send(result);
                        }
                        RuleCommand::Stop { index, reply } => {
                            let result = stop_running_rule(
                                index,
                                &mut running,
                                &task_controls,
                                &task_handle,
                                &routes,
                            ).await;
                            let _ = reply.send(result);
                        }
                    }
                }
            }
        }
        for mut runtime in running.into_values() {
            if let Some(stop) = runtime.stop.take() {
                let _ = stop.send(());
            }
            if let Some(task) = runtime.task.take() {
                let _ = task.await;
            }
        }
        if let Ok(mut router) = locked(&routes) {
            router.routes.clear();
            router.pending = None;
        }
        let _ = task_handle
            .disconnect(Disconnect::ByApplication, "rssh forward stopped", "")
            .await;
        task_connected.store(false, Ordering::Relaxed);
    });

    Ok(ForwardHandle::from_task(
        task, disconnect, controls, commands, connected,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::{TcpListener, TcpStream};

    struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            if let Some(signal) = self.0.take() {
                let _ = signal.send(());
            }
        }
    }

    fn test_handle(task: tokio::task::JoinHandle<()>, disconnect: Arc<Notify>) -> ForwardHandle {
        let (commands, _commands_rx) = tokio::sync::mpsc::unbounded_channel();
        ForwardHandle::from_task(
            task,
            disconnect,
            Vec::new(),
            commands,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
        )
    }

    #[tokio::test]
    async fn stats_aggregate_independent_rule_snapshots() {
        let first = Arc::new(RuleControl::new(ForwardRule {
            forward_type: ForwardType::Local,
            local_port: 8080,
            remote_host: "db".into(),
            remote_port: 5432,
        }));
        let second = Arc::new(RuleControl::new(ForwardRule {
            forward_type: ForwardType::Dynamic,
            local_port: 1080,
            remote_host: "127.0.0.1".into(),
            remote_port: 0,
        }));
        first.bytes_tx.store(12, Ordering::Relaxed);
        first.bytes_rx.store(34, Ordering::Relaxed);
        first.connections.store(2, Ordering::Relaxed);
        first.set_state(ForwardRuleStatus::Active, Some(8080), None);
        second.bytes_tx.store(5, Ordering::Relaxed);
        second.set_state(ForwardRuleStatus::Stopped, None, None);
        let task = tokio::spawn(std::future::pending::<()>());
        let (commands, _commands_rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = ForwardHandle::from_task(
            task,
            Arc::new(Notify::new()),
            vec![first, second],
            commands,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
        );

        let stats = handle.stats();

        assert_eq!(stats.bytes_tx, 17);
        assert_eq!(stats.bytes_rx, 34);
        assert_eq!(stats.connections, 2);
        assert_eq!(stats.rules[0].index, 0);
        assert_eq!(stats.rules[0].status, ForwardRuleStatus::Active);
        assert_eq!(stats.rules[1].index, 1);
        assert_eq!(stats.rules[1].status, ForwardRuleStatus::Stopped);
    }

    #[tokio::test]
    async fn completion_only_matches_the_current_running_generation() {
        let mut running = HashMap::new();
        let task = tokio::spawn(std::future::pending::<()>());
        running.insert(
            0,
            RunningRule {
                generation: 2,
                task: Some(task),
                stop: None,
                remote_port: None,
            },
        );

        assert!(!completion_matches(&running, 0, 1));
        assert!(completion_matches(&running, 0, 2));

        let runtime = running.get_mut(&0).unwrap();
        runtime.task.take().unwrap().abort();
        assert!(!completion_matches(&running, 0, 2));
    }

    #[tokio::test]
    async fn dropping_handle_aborts_background_task() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _drop_signal = DropSignal(Some(dropped_tx));
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        });
        let handle = test_handle(task, Arc::new(Notify::new()));

        started_rx.await.unwrap();
        drop(handle);

        tokio::time::timeout(std::time::Duration::from_secs(1), dropped_rx)
            .await
            .expect("dropping ForwardHandle must abort its detached task")
            .unwrap();
    }

    #[test]
    fn stopping_handle_outside_tokio_runtime_does_not_panic() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let task = runtime.spawn(std::future::pending::<()>());
        let handle = test_handle(task, Arc::new(Notify::new()));

        handle.stop();
    }

    #[tokio::test]
    async fn stopping_handle_allows_background_task_to_finish_gracefully() {
        let disconnect = Arc::new(Notify::new());
        let disconnect_task = disconnect.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = started_tx.send(());
            let _ = release_rx.await;
            disconnect_task.notified().await;
            let _ = finished_tx.send(());
        });
        let handle = test_handle(task, disconnect);

        started_rx.await.unwrap();
        handle.stop();
        release_tx.send(()).unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(1), finished_rx)
            .await
            .expect("stop must allow the task to observe the disconnect signal")
            .unwrap();
    }

    /// 起一对 loopback TCP socket：返回 (server_side, client_side)。
    /// 端口 0 让内核分配，避免冲突。
    async fn loopback_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let connect = tokio::spawn(async move { TcpStream::connect(addr).await.unwrap() });
        let (server, _) = listener.accept().await.unwrap();
        let client = connect.await.unwrap();
        (server, client)
    }

    /// SOCKS5 greeting + 吃回 [0x05, 0x00]。封 helper 让每个测试只关心 connect req。
    async fn negotiate_no_auth(client: &mut TcpStream) {
        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut reply = [0u8; 2];
        client.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, [0x05, 0x00]);
    }

    #[tokio::test]
    async fn socks5_ipv4() {
        let (mut server, mut client) = loopback_pair().await;
        let driver = tokio::spawn(async move {
            negotiate_no_auth(&mut client).await;
            // CONNECT 1.2.3.4:80
            client
                .write_all(&[0x05, 0x01, 0x00, 0x01, 1, 2, 3, 4, 0x00, 0x50])
                .await
                .unwrap();
            let mut reply = [0u8; 10];
            client.read_exact(&mut reply).await.unwrap();
            assert_eq!(&reply[..2], &[0x05, 0x00]);
        });
        let (host, port) = socks5_handshake(&mut server).await.unwrap();
        assert_eq!(host, "1.2.3.4");
        assert_eq!(port, 80);
        driver.await.unwrap();
    }

    #[tokio::test]
    async fn socks5_domain() {
        let (mut server, mut client) = loopback_pair().await;
        let domain = b"example.com";
        let driver = tokio::spawn(async move {
            negotiate_no_auth(&mut client).await;
            let mut req = vec![0x05, 0x01, 0x00, 0x03, domain.len() as u8];
            req.extend_from_slice(domain);
            req.extend_from_slice(&443u16.to_be_bytes());
            client.write_all(&req).await.unwrap();
            let mut reply = [0u8; 10];
            client.read_exact(&mut reply).await.unwrap();
            assert_eq!(&reply[..2], &[0x05, 0x00]);
        });
        let (host, port) = socks5_handshake(&mut server).await.unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
        driver.await.unwrap();
    }

    #[tokio::test]
    async fn socks5_ipv6() {
        let (mut server, mut client) = loopback_pair().await;
        // 2001:db8::1
        let addr_bytes: [u8; 16] = [
            0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x01,
        ];
        let driver = tokio::spawn(async move {
            negotiate_no_auth(&mut client).await;
            let mut req = vec![0x05, 0x01, 0x00, 0x04];
            req.extend_from_slice(&addr_bytes);
            req.extend_from_slice(&8080u16.to_be_bytes());
            client.write_all(&req).await.unwrap();
            let mut reply = [0u8; 10];
            client.read_exact(&mut reply).await.unwrap();
            assert_eq!(&reply[..2], &[0x05, 0x00]);
        });
        let (host, port) = socks5_handshake(&mut server).await.unwrap();
        // 实现里 IPv6 全 8 段拼接，不压缩
        assert_eq!(host, "2001:db8:0:0:0:0:0:1");
        assert_eq!(port, 8080);
        driver.await.unwrap();
    }

    #[tokio::test]
    async fn socks5_rejects_non_v5() {
        let (mut server, mut client) = loopback_pair().await;
        let driver = tokio::spawn(async move {
            client.write_all(&[0x04, 0x01, 0x00]).await.unwrap();
        });
        let err = socks5_handshake(&mut server).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        driver.await.unwrap();
    }

    #[tokio::test]
    async fn socks5_rejects_bind_command() {
        let (mut server, mut client) = loopback_pair().await;
        let driver = tokio::spawn(async move {
            negotiate_no_auth(&mut client).await;
            // CMD=0x02 (BIND) — 不支持
            client
                .write_all(&[0x05, 0x02, 0x00, 0x01, 1, 2, 3, 4, 0x00, 0x50])
                .await
                .unwrap();
            let mut reply = [0u8; 10];
            client.read_exact(&mut reply).await.unwrap();
            assert_eq!(reply[1], 0x07); // command not supported
        });
        let err = socks5_handshake(&mut server).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        driver.await.unwrap();
    }

    #[tokio::test]
    async fn socks5_rejects_unknown_atyp() {
        let (mut server, mut client) = loopback_pair().await;
        let driver = tokio::spawn(async move {
            negotiate_no_auth(&mut client).await;
            // atyp=0xff 不存在 — 实现走 default 分支直接 Err
            client.write_all(&[0x05, 0x01, 0x00, 0xff]).await.unwrap();
        });
        let err = socks5_handshake(&mut server).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        driver.await.unwrap();
    }

    #[tokio::test]
    async fn counted_copy_streams_and_counts() {
        // tokio 没给 std::io::Cursor 实现 AsyncRead/Write，用官方 duplex。
        let (a_read, mut a_write) = tokio::io::duplex(64);
        let (mut b_read, b_write) = tokio::io::duplex(64);
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();

        let copier = tokio::spawn(async move {
            let mut a = a_read;
            let mut b = b_write;
            counted_copy(&mut a, &mut b, &counter_clone).await
        });

        let payload = b"the quick brown fox jumps over the lazy dog";
        a_write.write_all(payload).await.unwrap();
        drop(a_write); // EOF → 退出 loop

        let mut received = Vec::new();
        b_read.read_to_end(&mut received).await.unwrap();
        copier.await.unwrap().unwrap();

        assert_eq!(received, payload);
        assert_eq!(counter.load(Ordering::Relaxed), payload.len() as u64);
    }

    #[tokio::test]
    async fn counted_copy_zero_bytes() {
        let (a_read, a_write) = tokio::io::duplex(64);
        let (_b_read, b_write) = tokio::io::duplex(64);
        let counter = AtomicU64::new(0);
        drop(a_write); // 立刻 EOF
        let mut a = a_read;
        let mut b = b_write;
        counted_copy(&mut a, &mut b, &counter).await.unwrap();
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }
}
