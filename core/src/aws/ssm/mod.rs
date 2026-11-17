//! Native Session Manager shell transport. AWS APIs create the session; the
//! signed WebSocket carries the Session Manager protocol, not a local PTY.

mod codec;
mod crypto;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::aws::{
    api,
    auth::{AwsAuth, AwsCredentials},
};
use crate::error::{locked, AppError, AppResult};
use crate::models::ConnectorSpec;
use crate::resource::{Cleanup, CleanupFuture, CleanupHandle, OpenedResource, PendingOperation};
use codec::*;
use crypto::Encryption;

// This is the negotiated AWS protocol feature level, not the RSSH app version.
// AWS recommends 1.3.0.0 for clients built from its published implementation.
const CLIENT_VERSION: &str = "1.3.0.0";
const INPUT_CHUNK: usize = 16 * 1024;
const MAX_WRITE: usize = 256 * 1024;
const MAX_QUEUED_BYTES: usize = 2 * 1024 * 1024;
const MAX_OUT_OF_ORDER: usize = 64;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
const OPEN_TIMEOUT: Duration = Duration::from_secs(90);
const PING_INTERVAL: Duration = Duration::from_secs(15);
const SILENCE_TIMEOUT: Duration = Duration::from_secs(60);
const RETRANSMIT_AFTER: Duration = Duration::from_secs(2);
const MAX_RESENDS: u32 = 8;
const MAX_RECONNECTS: u32 = 3;

pub enum SsmOut {
    Data(Vec<u8>),
    Error(AppError),
    Close,
}

pub type SsmSink = Arc<dyn Fn(&str, SsmOut) + Send + Sync>;
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn error(code: &'static str) -> AppError {
    AppError::other(code, json!({}))
}

fn protocol_error(reason: &'static str) -> AppError {
    AppError::other("aws_ssm_protocol_error", json!({ "reason": reason }))
}

fn connect_error<T>(_: T) -> AppError {
    // WebSocket errors may include the signed URL or session token.
    error("aws_ssm_connect_failed")
}

#[derive(Clone)]
struct SessionInfo {
    id: String,
    stream_url: String,
    token: String,
}

impl SessionInfo {
    fn from_response(value: Value, existing_id: Option<&str>) -> AppResult<Self> {
        let id = value["SessionId"]
            .as_str()
            .or(existing_id)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| protocol_error("missing session id"))?;
        if existing_id.is_some_and(|expected| expected != id) {
            return Err(protocol_error("resume changed session id"));
        }
        Ok(Self {
            id: id.to_owned(),
            // Preserve a returned ID even if the other fields are malformed:
            // cleanup must still terminate that newly created AWS session.
            stream_url: value["StreamUrl"].as_str().unwrap_or_default().to_owned(),
            token: value["TokenValue"].as_str().unwrap_or_default().to_owned(),
        })
    }
}

fn required_string(value: &Value, field: &str) -> AppResult<String> {
    value[field]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| protocol_error("missing response field"))
}

struct DataKey {
    plaintext: Vec<u8>,
    ciphertext: String,
}

/// The API boundary is injectable so protocol, cancellation, reconnect and
/// remote cleanup can be exercised against an actual local WebSocket server.
#[async_trait::async_trait]
trait Service: Send + Sync {
    async fn start(&self, target: &str) -> AppResult<SessionInfo>;
    async fn resume(&self, id: &str) -> AppResult<SessionInfo>;
    async fn terminate(&self, id: &str) -> AppResult<()>;
    async fn data_key(
        &self,
        key: &str,
        session: &str,
        target: &str,
        challenge: Option<&str>,
    ) -> AppResult<DataKey>;
    async fn connect(&self, info: &SessionInfo) -> AppResult<Socket>;
}

struct AwsService {
    auth: Arc<AwsAuth>,
    source_id: String,
    region: String,
    credentials: CredentialLease,
}

/// In-memory, zeroizing temporary credentials retained only for cleanup after
/// logout/deletion. Normal requests always consult the live auth provider.
/// A source cannot silently switch the principal owning an existing session.
struct CredentialLease {
    account_id: String,
    cached: Mutex<Option<AwsCredentials>>,
}

impl CredentialLease {
    fn remember(&self, credentials: AwsCredentials) -> AppResult<AwsCredentials> {
        if credentials.account_id != self.account_id {
            return Err(error("aws_ssm_account_mismatch"));
        }
        let mut cached = locked(&self.cached)?;
        if credentials.arn.is_empty()
            || cached
                .as_ref()
                .is_some_and(|previous| previous.arn != credentials.arn)
        {
            return Err(error("aws_ssm_identity_changed"));
        }
        *cached = Some(credentials.clone());
        Ok(credentials)
    }

    fn cleanup_fallback(&self, refreshed: AppResult<AwsCredentials>) -> AppResult<AwsCredentials> {
        match refreshed {
            Ok(credentials) => Ok(credentials),
            Err(error) => locked(&self.cached)?.clone().ok_or(error),
        }
    }
}

impl AwsService {
    async fn credentials(&self) -> AppResult<AwsCredentials> {
        let credentials = self.auth.credentials(&self.source_id).await?;
        self.credentials.remember(credentials)
    }

    async fn request(&self, target: &str, body: Value) -> AppResult<Value> {
        let credentials = self.credentials().await?;
        api::json_request(&credentials, &self.region, "ssm", target, &body).await
    }
}

fn data_key_request(key: &str, session: &str, target: &str, challenge: Option<&str>) -> Value {
    let mut context = json!({ "aws:ssm:SessionId": session, "aws:ssm:TargetId": target });
    if let Some(challenge) = challenge {
        // This is the agent's opaque encryption-context challenge, distinct
        // from the encrypted proof exchanged after KMS key negotiation.
        context["aws:ssm:RandomChallenge"] = challenge.into();
    }
    json!({ "KeyId": key, "NumberOfBytes": 64, "EncryptionContext": context })
}

#[async_trait::async_trait]
impl Service for AwsService {
    async fn start(&self, target: &str) -> AppResult<SessionInfo> {
        // Omit DocumentName to honor this account's Session Manager preferences,
        // including RunAs, logging and required KMS encryption.
        let response = self
            .request("AmazonSSM.StartSession", json!({"Target": target}))
            .await?;
        SessionInfo::from_response(response, None)
    }

    async fn resume(&self, id: &str) -> AppResult<SessionInfo> {
        let response = self
            .request("AmazonSSM.ResumeSession", json!({"SessionId": id}))
            .await?;
        SessionInfo::from_response(response, Some(id))
    }

    async fn terminate(&self, id: &str) -> AppResult<()> {
        // Logout must disable reconnect/KMS/normal requests, while the original
        // still-valid session principal remains available to release resources.
        let credentials = self
            .credentials
            .cleanup_fallback(self.credentials().await)?;
        api::terminate_session(&credentials, &self.region, id).await
    }

    async fn data_key(
        &self,
        key: &str,
        session: &str,
        target: &str,
        challenge: Option<&str>,
    ) -> AppResult<DataKey> {
        let credentials = self.credentials().await?;
        let mut response = api::json_request(
            &credentials,
            &self.region,
            "kms",
            "TrentService.GenerateDataKey",
            &data_key_request(key, session, target, challenge),
        )
        .await?;
        let encoded = zeroize::Zeroizing::new(required_string(&response, "Plaintext")?);
        // Avoid retaining a second plaintext-key copy in the JSON allocation.
        if let Some(Value::String(value)) =
            response.as_object_mut().and_then(|v| v.remove("Plaintext"))
        {
            drop(zeroize::Zeroizing::new(value));
        }
        let plaintext = STANDARD
            .decode(encoded.as_bytes())
            .map_err(|_| protocol_error("invalid KMS plaintext encoding"))?;
        let ciphertext = required_string(&response, "CiphertextBlob")?;
        Ok(DataKey {
            plaintext,
            ciphertext,
        })
    }

    async fn connect(&self, info: &SessionInfo) -> AppResult<Socket> {
        validate_stream_url(&info.stream_url, &self.region, &info.id)?;
        let credentials = self.credentials().await?;
        let mut request = info
            .stream_url
            .as_str()
            .into_client_request()
            .map_err(connect_error)?;
        let url = url::Url::parse(&info.stream_url).map_err(connect_error)?;
        let presigned = url.query_pairs().any(|(key, _)| key == "X-Amz-Signature");
        if !presigned {
            for (name, value) in api::signed_headers(
                &credentials,
                &self.region,
                "ssmmessages",
                "GET",
                &info.stream_url,
                &[],
                &[],
            )? {
                let name: tokio_tungstenite::tungstenite::http::HeaderName =
                    name.parse().map_err(connect_error)?;
                let value = value.parse().map_err(connect_error)?;
                request.headers_mut().insert(name, value);
            }
        }
        let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(MAX_PAYLOAD + 1024))
            .max_frame_size(Some(MAX_PAYLOAD + 1024));
        let (socket, _) =
            tokio_tungstenite::connect_async_with_config(request, Some(config), false)
                .await
                .map_err(connect_error)?;
        Ok(socket)
    }
}

fn validate_stream_url(raw: &str, region: &str, id: &str) -> AppResult<()> {
    let url = url::Url::parse(raw).map_err(connect_error)?;
    let suffix = if region.starts_with("cn-") {
        "amazonaws.com.cn"
    } else {
        "amazonaws.com"
    };
    let expected_host = format!("ssmmessages.{region}.{suffix}");
    if url.scheme() != "wss"
        || url.host_str() != Some(expected_host.as_str())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|p| p != 443)
        || url.fragment().is_some()
        || url.path() != format!("/v1/data-channel/{id}")
    {
        return Err(protocol_error("invalid data-channel endpoint"));
    }
    Ok(())
}

enum Input {
    Write(Vec<u8>),
    Resize(u16, u16),
}

struct Control {
    cancel: watch::Sender<bool>,
    done: watch::Receiver<bool>,
    remote_id: Mutex<Option<String>>,
    terminated: tokio::sync::Mutex<bool>,
    service: Arc<dyn Service>,
}

impl Cleanup for Control {
    fn close(&self) -> CleanupFuture<'_> {
        Box::pin(async move {
            self.cancel.send_replace(true);
            let mut done = self.done.clone();
            while !*done.borrow_and_update() {
                if done.changed().await.is_err() {
                    break;
                }
            }
            let mut terminated = self.terminated.lock().await;
            if *terminated {
                return Ok(());
            }
            let remote_id = locked(&self.remote_id)?.clone();
            if let Some(id) = remote_id {
                self.service.terminate(&id).await?;
            }
            *terminated = true;
            Ok(())
        })
    }
}

#[derive(Clone)]
pub struct SsmHandle {
    control: Arc<Control>,
    input: mpsc::Sender<Input>,
    output: broadcast::Sender<Vec<u8>>,
    connector: Arc<ConnectorSpec>,
    target_key: Arc<str>,
}

impl SsmHandle {
    pub fn write(&self, data: &[u8]) -> AppResult<()> {
        if data.is_empty() {
            return Ok(());
        }
        if data.len() > MAX_WRITE {
            return Err(error("aws_ssm_buffer_full"));
        }
        self.send(Input::Write(data.to_vec()))
    }

    pub fn write_line(&self, text: &str) -> AppResult<()> {
        let mut data = text.as_bytes().to_vec();
        data.push(b'\r');
        self.write(&data)
    }

    pub fn resize(&self, cols: u16, rows: u16) -> AppResult<()> {
        self.send(Input::Resize(cols.max(1), rows.max(1)))
    }

    fn send(&self, input: Input) -> AppResult<()> {
        if *self.control.cancel.borrow() || *self.control.done.borrow() {
            return Err(error("aws_ssm_closed"));
        }
        self.input.try_send(input).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => error("aws_ssm_buffer_full"),
            mpsc::error::TrySendError::Closed(_) => error("aws_ssm_closed"),
        })
    }

    pub fn connector(&self) -> &ConnectorSpec {
        &self.connector
    }
    pub fn target_key(&self) -> &str {
        &self.target_key
    }
    pub fn subscribe_output(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output.subscribe()
    }
    pub fn cleanup(&self) -> CleanupHandle {
        CleanupHandle::new(self.control.clone())
    }
    pub fn cancel(&self) {
        self.control.cancel.send_replace(true);
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn open_resource(
    auth: Arc<AwsAuth>,
    session_id: String,
    source_id: String,
    region: String,
    instance_id: String,
    account_id: String,
    cols: u16,
    rows: u16,
    sink: SsmSink,
    operation: PendingOperation,
) -> AppResult<OpenedResource<SsmHandle>> {
    let service = Arc::new(AwsService {
        auth,
        source_id: source_id.clone(),
        region: region.clone(),
        credentials: CredentialLease {
            account_id: account_id.clone(),
            cached: Mutex::new(None),
        },
    });
    let connector = ConnectorSpec::AwsNative {
        source_id,
        region,
        instance_id,
        account_id,
    };
    open_with_service(service, session_id, connector, cols, rows, sink, operation).await
}

async fn open_with_service(
    service: Arc<dyn Service>,
    id: String,
    connector: ConnectorSpec,
    cols: u16,
    rows: u16,
    sink: SsmSink,
    operation: PendingOperation,
) -> AppResult<OpenedResource<SsmHandle>> {
    let (target, target_key) = match &connector {
        ConnectorSpec::AwsNative {
            instance_id,
            account_id,
            region,
            ..
        } => (
            instance_id.clone(),
            format!("aws:{account_id}:{region}:{instance_id}"),
        ),
        _ => {
            return operation
                .finish_open(Err(protocol_error("invalid connector")))
                .await
        }
    };
    let (input, commands) = mpsc::channel(32);
    let (output, _) = broadcast::channel(128);
    let (cancel, cancelled) = watch::channel(false);
    let (finished, done) = watch::channel(false);
    let control = Arc::new(Control {
        cancel,
        done,
        remote_id: Mutex::new(None),
        terminated: tokio::sync::Mutex::new(false),
        service,
    });
    // This precedes spawning or StartSession. Dropping the opening future
    // leaves the registry owning the actor and its eventual AWS session ID.
    operation.retain_cleanup(CleanupHandle::new(control.clone()))?;
    let handle = SsmHandle {
        control: control.clone(),
        input,
        output: output.clone(),
        connector: Arc::new(connector),
        target_key: target_key.into(),
    };
    let (ready, readiness) = oneshot::channel();
    let actor_id = id.clone();
    tokio::spawn(async move {
        let mut exit = ActorExit {
            finished,
            sink: sink.clone(),
            id: actor_id.clone(),
            emit_close: false,
        };
        let mut ready = Some(ready);
        let result = run_actor(
            &control, &actor_id, &target, cols, rows, commands, cancelled, output, &sink,
            &mut ready,
        )
        .await;
        // An unsuccessful opener reports its original error through readiness.
        // A close event here would cancel the registry's PendingOperation first
        // and turn a useful auth/protocol failure into "open cancelled".
        exit.emit_close = ready.is_none();
        if let Some(ready) = ready.take() {
            let _ = ready.send(result.clone().and_then(|_| Err(error("aws_ssm_closed"))));
        }
        if let Err(error) = result {
            if !*control.cancel.borrow() {
                sink(&actor_id, SsmOut::Error(error));
            }
        }
    });
    let result = tokio::select! {
        biased;
        _ = operation.cancelled() => Err(error("aws_ssm_open_cancelled")),
        result = tokio::time::timeout(OPEN_TIMEOUT, readiness) => match result {
            Ok(Ok(Ok(()))) => Ok((id, handle)),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) => Err(error("aws_ssm_closed")),
            Err(_) => Err(error("aws_ssm_open_timeout")),
        }
    };
    operation.finish_open(result).await
}

struct ActorExit {
    finished: watch::Sender<bool>,
    sink: SsmSink,
    id: String,
    emit_close: bool,
}

impl Drop for ActorExit {
    fn drop(&mut self) {
        self.finished.send_replace(true);
        if self.emit_close {
            (self.sink)(&self.id, SsmOut::Close);
        }
    }
}

struct Unacknowledged {
    id: uuid::Uuid,
    bytes: Vec<u8>,
    last_sent: Option<Instant>,
    retries: u32,
}

#[derive(Default)]
struct Protocol {
    sequence: i64,
    expected: i64,
    outgoing: BTreeMap<i64, Unacknowledged>,
    incoming: BTreeMap<i64, Frame>,
    encryption: Option<Encryption>,
    handshake_started: bool,
    handshake_complete: bool,
    session_type_set: bool,
    paused: bool,
    remote_ended: bool,
}

impl Protocol {
    fn queued_bytes(&self) -> usize {
        self.outgoing.values().map(|m| m.bytes.len()).sum()
    }

    fn queue(&mut self, payload_type: u32, mut payload: Vec<u8>) -> AppResult<()> {
        if payload_type == OUTPUT {
            if let Some(encryption) = &self.encryption {
                payload = encryption.encrypt(&payload)?;
            }
        }
        let frame = Frame::input(self.sequence, payload_type, payload);
        let bytes = frame.encode()?;
        if self.queued_bytes() + bytes.len() > MAX_QUEUED_BYTES {
            return Err(error("aws_ssm_buffer_full"));
        }
        self.outgoing.insert(
            self.sequence,
            Unacknowledged {
                id: frame.id,
                bytes,
                last_sent: None,
                retries: 0,
            },
        );
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| protocol_error("input sequence exhausted"))?;
        Ok(())
    }

    fn acknowledge(&mut self, frame: &Frame) -> AppResult<()> {
        let ack: Value = serde_json::from_slice(&frame.payload)
            .map_err(|_| protocol_error("invalid acknowledgement"))?;
        let sequence = ack["AcknowledgedMessageSequenceNumber"]
            .as_i64()
            .ok_or_else(|| protocol_error("invalid acknowledgement sequence"))?;
        if ack["AcknowledgedMessageType"].as_str() != Some("input_stream_data")
            || ack["IsSequentialMessage"].as_bool() != Some(true)
        {
            return Err(protocol_error("invalid acknowledgement type"));
        }
        if let Some(pending) = self.outgoing.get(&sequence) {
            if ack["AcknowledgedMessageId"].as_str() != Some(pending.id.to_string().as_str()) {
                return Err(protocol_error("acknowledgement id mismatch"));
            }
            self.outgoing.remove(&sequence);
        }
        Ok(())
    }

    async fn flush(&mut self, socket: &mut Socket) -> AppResult<()> {
        if self.paused {
            return Ok(());
        }
        for pending in self.outgoing.values_mut() {
            if pending.last_sent.is_none() {
                send(socket, Message::Binary(pending.bytes.clone().into())).await?;
                pending.last_sent = Some(Instant::now());
            }
        }
        Ok(())
    }

    async fn retransmit(&mut self, socket: &mut Socket) -> AppResult<()> {
        if self.paused {
            return Ok(());
        }
        if let Some(pending) = self.outgoing.values_mut().next() {
            if pending
                .last_sent
                .is_some_and(|sent| sent.elapsed() >= RETRANSMIT_AFTER)
            {
                if pending.retries >= MAX_RESENDS {
                    return Err(error("aws_ssm_connect_failed"));
                }
                send(socket, Message::Binary(pending.bytes.clone().into())).await?;
                pending.last_sent = Some(Instant::now());
                pending.retries += 1;
            }
        }
        Ok(())
    }
}

async fn send(socket: &mut Socket, message: Message) -> AppResult<()> {
    tokio::time::timeout(CONNECT_TIMEOUT, socket.send(message))
        .await
        .map_err(connect_error)?
        .map_err(connect_error)
}

async fn connect(service: &dyn Service, info: &SessionInfo, client_id: &str) -> AppResult<Socket> {
    if info.stream_url.is_empty() || info.token.is_empty() {
        return Err(protocol_error("missing data-channel credentials"));
    }
    let mut socket = tokio::time::timeout(CONNECT_TIMEOUT, service.connect(info))
        .await
        .map_err(connect_error)??;
    send(
        &mut socket,
        Message::Text(
            json!({
                "MessageSchemaVersion": "1.0",
                "RequestId": uuid::Uuid::new_v4().to_string(),
                "TokenValue": info.token,
                "ClientId": client_id,
                "ClientVersion": CLIENT_VERSION,
            })
            .to_string()
            .into(),
        ),
    )
    .await?;
    Ok(socket)
}

#[allow(clippy::too_many_arguments)]
async fn run_actor(
    control: &Control,
    id: &str,
    target: &str,
    cols: u16,
    rows: u16,
    mut input: mpsc::Receiver<Input>,
    mut cancelled: watch::Receiver<bool>,
    output: broadcast::Sender<Vec<u8>>,
    sink: &SsmSink,
    ready: &mut Option<oneshot::Sender<AppResult<()>>>,
) -> AppResult<()> {
    // Deliberately do not cancel an in-flight StartSession request: its response
    // is needed to terminate a session created after the UI was cancelled.
    let info = control.service.start(target).await?;
    *locked(&control.remote_id)? = Some(info.id.clone());
    if *cancelled.borrow() {
        return Ok(());
    }
    let client_id = uuid::Uuid::new_v4().to_string();
    let mut socket = tokio::select! {
        _ = cancelled.changed() => return Ok(()),
        result = connect(control.service.as_ref(), &info, &client_id) => result?,
    };
    let mut protocol = Protocol::default();
    let mut retries = 0;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let opened = Instant::now();
    let mut received = Instant::now();
    let mut pinged = Instant::now();
    let mut initial_size_queued = false;
    loop {
        if *cancelled.borrow() {
            break;
        }
        let mut peer_responded = false;
        // Fair polling keeps a continuously ready output stream from starving
        // terminal input and retransmission timers. Cancellation is checked
        // before each iteration as well as while waiting below.
        let mut result: AppResult<bool> = tokio::select! {
            _ = cancelled.changed() => break,
            message = socket.next() => match message {
                Some(Ok(Message::Binary(bytes))) => {
                    received = Instant::now();
                    peer_responded = true;
                    match process_frame(&mut protocol, &mut socket, &bytes, control.service.as_ref(), &info.id, target, id, sink, &output).await {
                        Ok(closed) => Ok(closed),
                        Err(error) => Err(error),
                    }
                },
                Some(Ok(Message::Ping(bytes))) => { received = Instant::now(); peer_responded = true; send(&mut socket, Message::Pong(bytes)).await.map(|_| false) },
                Some(Ok(Message::Pong(_))) => { received = Instant::now(); peer_responded = true; Ok(false) },
                Some(Ok(Message::Text(bytes))) => {
                    // Some services send control frames using a text WebSocket
                    // opcode. The protocol remains the same binary envelope.
                    received = Instant::now();
                    peer_responded = true;
                    process_frame(&mut protocol, &mut socket, bytes.as_bytes(), control.service.as_ref(), &info.id, target, id, sink, &output).await
                },
                Some(Ok(Message::Frame(_))) => Ok(false),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => Err(error("aws_ssm_connect_failed")),
            },
            command = input.recv(), if protocol.handshake_complete && protocol.queued_bytes() < MAX_QUEUED_BYTES - MAX_WRITE - 8192 => {
                match command {
                    Some(Input::Write(bytes)) => {
                        for chunk in bytes.chunks(INPUT_CHUNK) { protocol.queue(OUTPUT, chunk.to_vec())?; }
                    },
                    Some(Input::Resize(cols, rows)) => protocol.queue(SIZE, serde_json::to_vec(&json!({"cols":cols,"rows":rows})).unwrap())?,
                    None => break,
                }
                protocol.flush(&mut socket).await.map(|_| false)
            },
            _ = tick.tick() => {
                if !protocol.handshake_complete && opened.elapsed() >= HANDSHAKE_TIMEOUT {
                    return Err(error("aws_ssm_open_timeout"));
                }
                if received.elapsed() >= SILENCE_TIMEOUT { Err(error("aws_ssm_connect_failed")) }
                else if pinged.elapsed() >= PING_INTERVAL {
                    pinged = Instant::now();
                    send(&mut socket, Message::Ping(b"keepalive".to_vec().into())).await.map(|_| false)
                } else { protocol.retransmit(&mut socket).await.map(|_| false) }
            },
        };
        if result.is_ok() && peer_responded {
            // Bound consecutive failed reconnects, not a mobile session's
            // lifetime number of network transitions.
            retries = 0;
        }
        if matches!(result, Ok(false)) {
            if protocol.handshake_complete && !initial_size_queued {
                protocol.queue(
                    SIZE,
                    serde_json::to_vec(&json!({"cols":cols.max(1),"rows":rows.max(1)})).unwrap(),
                )?;
                initial_size_queued = true;
            }
            // All writes, including handshake responses and initial resize,
            // pass through the same reconnect path as socket read failures.
            result = protocol.flush(&mut socket).await.map(|_| false);
            if result.is_ok() && protocol.handshake_complete {
                if let Some(ready) = ready.take() {
                    if ready.send(Ok(())).is_err() {
                        break;
                    }
                }
            }
        }
        match result {
            Ok(true) => {
                // Only a channel_closed naming this exact remote session proves
                // it ended. Legacy notifications without an ID still close the
                // stream, but cleanup must confirm termination through the API.
                if protocol.remote_ended {
                    *control.terminated.lock().await = true;
                }
                break;
            }
            Ok(false) => {}
            Err(error)
                if error.code() == "aws_ssm_connect_failed" && protocol.handshake_complete =>
            {
                // Sequence numbers, encrypted frames and keys remain unchanged.
                // Replay unacknowledged frames only after ResumeSession supplies
                // a fresh token; never silently start a second remote shell.
                loop {
                    if retries >= MAX_RECONNECTS {
                        return Err(error.clone());
                    }
                    retries += 1;
                    let attempt = async {
                        tokio::time::sleep(Duration::from_millis(500 * (1 << (retries - 1)))).await;
                        let resumed = control.service.resume(&info.id).await?;
                        connect(control.service.as_ref(), &resumed, &client_id).await
                    };
                    match tokio::select! { _ = cancelled.changed() => return Ok(()), result = attempt => result }
                    {
                        Ok(reconnected) => {
                            socket = reconnected;
                            break;
                        }
                        Err(_) if retries < MAX_RECONNECTS => continue,
                        Err(error) => return Err(error),
                    }
                }
                received = Instant::now();
                pinged = Instant::now();
                protocol.paused = false;
                for pending in protocol.outgoing.values_mut() {
                    pending.last_sent = None;
                    pending.retries = 0;
                }
            }
            Err(error) => return Err(error),
        }
    }
    // Lifecycle cleanup terminates after exit unless AWS already confirmed it.
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.close(None)).await;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_frame(
    protocol: &mut Protocol,
    socket: &mut Socket,
    bytes: &[u8],
    service: &dyn Service,
    remote_id: &str,
    target: &str,
    id: &str,
    sink: &SsmSink,
    output: &broadcast::Sender<Vec<u8>>,
) -> AppResult<bool> {
    let frame = Frame::decode(bytes)?;
    match frame.message_type.as_str() {
        "acknowledge" => {
            protocol.acknowledge(&frame)?;
            return Ok(false);
        }
        "start_publication" => {
            protocol.paused = false;
            return Ok(false);
        }
        "pause_publication" => {
            protocol.paused = true;
            return Ok(false);
        }
        "channel_closed" => {
            let closed: Value = serde_json::from_slice(&frame.payload)
                .map_err(|_| protocol_error("invalid channel close"))?;
            if closed["SessionId"]
                .as_str()
                .is_some_and(|other| other != remote_id)
            {
                return Err(protocol_error("channel close session mismatch"));
            }
            protocol.remote_ended = closed["SessionId"].as_str() == Some(remote_id);
            return Ok(true);
        }
        "output_stream_data" => {}
        _ => return Err(protocol_error("unsupported message type")),
    }
    if frame.sequence < 0 {
        return Err(protocol_error("invalid output sequence"));
    }
    if frame.sequence > protocol.expected {
        if protocol.incoming.len() >= MAX_OUT_OF_ORDER
            || protocol
                .incoming
                .values()
                .map(|f| f.payload.len())
                .sum::<usize>()
                + frame.payload.len()
                > MAX_QUEUED_BYTES
        {
            return Err(protocol_error("output reorder buffer exceeded"));
        }
        // ACK only after storing: acknowledging an unstored message loses data.
        let ack = frame.acknowledgement().encode()?;
        protocol.incoming.entry(frame.sequence).or_insert(frame);
        send(socket, Message::Binary(ack.into())).await?;
        return Ok(false);
    }
    send(
        socket,
        Message::Binary(frame.acknowledgement().encode()?.into()),
    )
    .await?;
    if frame.sequence < protocol.expected {
        return Ok(false);
    }
    let mut next = Some(frame);
    while let Some(frame) = next {
        process_payload(
            protocol, frame, service, remote_id, target, id, sink, output,
        )
        .await?;
        protocol.expected = protocol
            .expected
            .checked_add(1)
            .ok_or_else(|| protocol_error("output sequence exhausted"))?;
        next = protocol.incoming.remove(&protocol.expected);
    }
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
async fn process_payload(
    protocol: &mut Protocol,
    mut frame: Frame,
    service: &dyn Service,
    remote_id: &str,
    target: &str,
    id: &str,
    sink: &SsmSink,
    output: &broadcast::Sender<Vec<u8>>,
) -> AppResult<()> {
    // The agent encrypts all three stream-result types. Authenticate exit codes
    // too, even though this shell UI waits for channel_closed to finish.
    if matches!(frame.payload_type, OUTPUT | STDERR | EXIT_CODE) {
        if let Some(encryption) = &protocol.encryption {
            frame.payload = encryption.decrypt(&frame.payload)?;
        }
    }
    match frame.payload_type {
        HANDSHAKE_REQUEST => {
            if protocol.handshake_started || protocol.handshake_complete {
                return Err(protocol_error("duplicate handshake"));
            }
            protocol.handshake_started = true;
            let request: Value = serde_json::from_slice(&frame.payload)
                .map_err(|_| protocol_error("invalid handshake"))?;
            let actions = request["RequestedClientActions"]
                .as_array()
                .ok_or_else(|| protocol_error("missing handshake actions"))?;
            let mut processed = Vec::new();
            for action in actions {
                let kind = action["ActionType"]
                    .as_str()
                    .ok_or_else(|| protocol_error("missing handshake action"))?;
                let parameters = &action["ActionParameters"];
                let result = match kind {
                    "SessionType" => match parameters["SessionType"].as_str() {
                        Some(
                            "Standard_Stream" | "InteractiveCommands" | "NonInteractiveCommands",
                        ) => {
                            protocol.session_type_set = true;
                            Value::Null
                        }
                        _ => return Err(protocol_error("unsupported session type")),
                    },
                    "KMSEncryption" => {
                        if protocol.encryption.is_some() {
                            return Err(protocol_error("duplicate encryption action"));
                        }
                        let key = required_string(parameters, "KMSKeyId")?;
                        let challenge = match parameters.get("Challenge") {
                            None | Some(Value::Null) => None,
                            Some(Value::String(value)) => {
                                (!value.is_empty()).then_some(value.as_str())
                            }
                            _ => return Err(protocol_error("invalid KMS context challenge")),
                        };
                        let data_key = service.data_key(&key, remote_id, target, challenge).await?;
                        protocol.encryption = Some(Encryption::new(data_key.plaintext)?);
                        json!({
                            "KMSCipherTextKey": data_key.ciphertext,
                            "KMSCipherTextHash": null,
                            "ChallengeAcknowledgement": challenge.is_some(),
                        })
                    }
                    _ => return Err(protocol_error("unsupported handshake action")),
                };
                processed.push(
                    json!({"ActionType":kind,"ActionStatus":1,"ActionResult":result,"Error":""}),
                );
            }
            protocol.queue(
                HANDSHAKE_RESPONSE,
                serde_json::to_vec(&json!({
                    "ClientVersion":CLIENT_VERSION,"ProcessedClientActions":processed,"Errors":[],
                }))
                .unwrap(),
            )?;
        }
        HANDSHAKE_COMPLETE => {
            if !protocol.handshake_started || !protocol.session_type_set {
                return Err(protocol_error("unexpected handshake completion"));
            }
            protocol.handshake_complete = true;
            let completion: Value = serde_json::from_slice(&frame.payload)
                .map_err(|_| protocol_error("invalid handshake completion"))?;
            if let Some(message) = completion["CustomerMessage"]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                let bytes = format!("{message}\r\n").into_bytes();
                let _ = output.send(bytes.clone());
                sink(id, SsmOut::Data(bytes));
            }
        }
        CHALLENGE_REQUEST => {
            let encryption = protocol
                .encryption
                .as_ref()
                .ok_or_else(|| protocol_error("unexpected encryption challenge"))?;
            let challenge: Value = serde_json::from_slice(&frame.payload)
                .map_err(|_| protocol_error("invalid encryption challenge"))?;
            let bytes = STANDARD
                .decode(required_string(&challenge, "Challenge")?)
                .map_err(|_| protocol_error("invalid challenge encoding"))?;
            let plaintext = zeroize::Zeroizing::new(encryption.decrypt(&bytes)?);
            let response = encryption.encrypt(&plaintext)?;
            protocol.queue(
                CHALLENGE_RESPONSE,
                serde_json::to_vec(&json!({"Challenge":STANDARD.encode(response)})).unwrap(),
            )?;
        }
        OUTPUT | STDERR | ERROR => {
            if !protocol.handshake_started && frame.payload_type == OUTPUT {
                // Old agents begin shell output without a handshake.
                protocol.handshake_complete = true;
                protocol.session_type_set = true;
            }
            if !protocol.handshake_complete {
                return Err(protocol_error("output preceded handshake completion"));
            }
            let data = frame.payload;
            let _ = output.send(data.clone());
            sink(id, SsmOut::Data(data));
        }
        EXIT_CODE => {} // channel_closed terminates the stream after final output.
        _ => return Err(protocol_error("unsupported payload type")),
    }
    Ok(())
}
