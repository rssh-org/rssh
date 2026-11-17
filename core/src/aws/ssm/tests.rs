use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::TcpListener;
use tokio::sync::Notify;

struct MockService {
    url: String,
    starts: AtomicUsize,
    resumes: AtomicUsize,
    terminates: AtomicUsize,
    fail_terminate: AtomicUsize,
    start_entered: Notify,
    start_release: Option<Notify>,
    terminate_entered: Notify,
    terminate_release: Option<Notify>,
    credential_observer: Option<(Arc<dyn crate::secret::SecretStore>, String)>,
    data_key_challenges: Mutex<Vec<Option<String>>>,
}

impl MockService {
    fn new(url: String) -> Self {
        Self {
            url,
            starts: AtomicUsize::new(0),
            resumes: AtomicUsize::new(0),
            terminates: AtomicUsize::new(0),
            fail_terminate: AtomicUsize::new(0),
            start_entered: Notify::new(),
            start_release: None,
            terminate_entered: Notify::new(),
            terminate_release: None,
            credential_observer: None,
            data_key_challenges: Mutex::new(Vec::new()),
        }
    }
    fn info(&self, token: &str) -> SessionInfo {
        SessionInfo {
            id: "aws-session".into(),
            stream_url: self.url.clone(),
            token: token.into(),
        }
    }
}

#[async_trait::async_trait]
impl Service for MockService {
    async fn start(&self, _: &str) -> AppResult<SessionInfo> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.start_entered.notify_one();
        if let Some(release) = &self.start_release {
            release.notified().await;
        }
        Ok(self.info("start-token"))
    }
    async fn resume(&self, id: &str) -> AppResult<SessionInfo> {
        assert_eq!(id, "aws-session");
        self.resumes.fetch_add(1, Ordering::SeqCst);
        Ok(self.info("resume-token"))
    }
    async fn terminate(&self, id: &str) -> AppResult<()> {
        assert_eq!(id, "aws-session");
        self.terminates.fetch_add(1, Ordering::SeqCst);
        if let Some((store, key)) = &self.credential_observer {
            assert!(
                store.get(key)?.is_some(),
                "source grant was deleted before TerminateSession"
            );
        }
        self.terminate_entered.notify_one();
        if let Some(release) = &self.terminate_release {
            release.notified().await;
        }
        if let Some((store, key)) = &self.credential_observer {
            assert!(
                store.get(key)?.is_some(),
                "source grant was deleted while TerminateSession was in flight"
            );
        }
        if self
            .fail_terminate
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(error("aws_ssm_connect_failed"));
        }
        Ok(())
    }
    async fn data_key(
        &self,
        key: &str,
        session: &str,
        target: &str,
        challenge: Option<&str>,
    ) -> AppResult<DataKey> {
        assert_eq!(key, "kms-test");
        assert_eq!(session, "aws-session");
        assert_eq!(target, "i-0123456789abcdef0");
        self.data_key_challenges
            .lock()
            .unwrap()
            .push(challenge.map(str::to_owned));
        Ok(DataKey {
            plaintext: (0..64).collect(),
            ciphertext: STANDARD.encode(b"encrypted-key"),
        })
    }
    async fn connect(&self, info: &SessionInfo) -> AppResult<Socket> {
        tokio_tungstenite::connect_async(&info.stream_url)
            .await
            .map(|(socket, _)| socket)
            .map_err(connect_error)
    }
}

fn connector() -> ConnectorSpec {
    ConnectorSpec::AwsNative {
        source_id: uuid::Uuid::new_v4().to_string(),
        region: "us-east-1".into(),
        instance_id: "i-0123456789abcdef0".into(),
        account_id: "123456789012".into(),
    }
}

fn event_sink() -> (SsmSink, mpsc::UnboundedReceiver<SsmOut>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        Arc::new(move |_, event| {
            let _ = tx.send(event);
        }),
        rx,
    )
}

async fn accept(listener: &TcpListener, token: &str) -> Socket {
    let (tcp, _) = listener.accept().await.unwrap();
    let mut socket = tokio_tungstenite::accept_async(MaybeTlsStream::Plain(tcp))
        .await
        .unwrap();
    let handshake = socket.next().await.unwrap().unwrap().into_text().unwrap();
    let handshake: Value = serde_json::from_str(&handshake).unwrap();
    assert_eq!(handshake["TokenValue"], token);
    assert_eq!(handshake["ClientVersion"], CLIENT_VERSION);
    socket
}

async fn read_frame(socket: &mut Socket) -> Frame {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            Message::Binary(bytes) => return Frame::decode(&bytes).unwrap(),
            Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.unwrap(),
            other => panic!("unexpected websocket message: {other:?}"),
        }
    }
}

async fn output_frame(socket: &mut Socket, sequence: i64, payload_type: u32, payload: Vec<u8>) {
    let mut frame = Frame::input(sequence, payload_type, payload);
    frame.message_type = "output_stream_data".into();
    socket
        .send(Message::Binary(frame.encode().unwrap().into()))
        .await
        .unwrap();
}

async fn ack_input(socket: &mut Socket, frame: &Frame) {
    socket
        .send(Message::Binary(
            frame.acknowledgement().encode().unwrap().into(),
        ))
        .await
        .unwrap();
}

async fn handshake(socket: &mut Socket, kms: bool) -> Option<Encryption> {
    handshake_with_challenge(socket, kms, None).await
}

async fn handshake_with_challenge(
    socket: &mut Socket,
    kms: bool,
    challenge: Option<&str>,
) -> Option<Encryption> {
    let mut actions = vec![
        json!({"ActionType":"SessionType","ActionParameters":{"SessionType":"Standard_Stream","Properties":{}}}),
    ];
    if kms {
        let mut parameters = json!({"KMSKeyId":"kms-test"});
        if let Some(challenge) = challenge {
            parameters["Challenge"] = challenge.into();
        }
        actions.push(json!({"ActionType":"KMSEncryption","ActionParameters":parameters}));
    }
    output_frame(
        socket,
        0,
        HANDSHAKE_REQUEST,
        serde_json::to_vec(&json!({"AgentVersion":"3.3.0.0","RequestedClientActions":actions}))
            .unwrap(),
    )
    .await;
    assert_eq!(read_frame(socket).await.message_type, "acknowledge");
    let response = read_frame(socket).await;
    assert_eq!(response.payload_type, HANDSHAKE_RESPONSE);
    assert_eq!(response.sequence, 0);
    let payload: Value = serde_json::from_slice(&response.payload).unwrap();
    assert_eq!(payload["Errors"], json!([]));
    ack_input(socket, &response).await;
    let encryption = if kms {
        assert_eq!(
            payload["ProcessedClientActions"][1]["ActionResult"]["ChallengeAcknowledgement"],
            challenge.is_some()
        );
        assert_eq!(
            payload["ProcessedClientActions"][1]["ActionResult"]["KMSCipherTextKey"],
            STANDARD.encode(b"encrypted-key")
        );
        let key: Vec<u8> = (0..64).collect();
        let agent = Encryption::new([&key[32..], &key[..32]].concat()).unwrap();
        let challenge = agent.encrypt(b"agent verifies the encryption key").unwrap();
        output_frame(
            socket,
            1,
            CHALLENGE_REQUEST,
            serde_json::to_vec(&json!({"Challenge":STANDARD.encode(challenge)})).unwrap(),
        )
        .await;
        assert_eq!(read_frame(socket).await.message_type, "acknowledge");
        let response = read_frame(socket).await;
        assert_eq!(response.payload_type, CHALLENGE_RESPONSE);
        let payload: Value = serde_json::from_slice(&response.payload).unwrap();
        assert_eq!(
            agent
                .decrypt(
                    &STANDARD
                        .decode(payload["Challenge"].as_str().unwrap())
                        .unwrap()
                )
                .unwrap(),
            b"agent verifies the encryption key"
        );
        ack_input(socket, &response).await;
        Some(agent)
    } else {
        None
    };
    output_frame(
        socket,
        if kms { 2 } else { 1 },
        HANDSHAKE_COMPLETE,
        b"{}".to_vec(),
    )
    .await;
    assert_eq!(read_frame(socket).await.message_type, "acknowledge");
    let size = read_frame(socket).await;
    assert_eq!(size.payload_type, SIZE);
    assert_eq!(
        serde_json::from_slice::<Value>(&size.payload).unwrap(),
        json!({"cols":80,"rows":24})
    );
    ack_input(socket, &size).await;
    encryption
}

async fn opened(service: Arc<MockService>, sink: SsmSink) -> SsmHandle {
    let (operation, _pending) = PendingOperation::new();
    let resource = open_with_service(
        service,
        uuid::Uuid::new_v4().to_string(),
        connector(),
        80,
        24,
        sink,
        operation,
    )
    .await
    .unwrap();
    let mut handle = None;
    resource
        .activate(|_, opened| {
            handle = Some(opened);
            Ok(())
        })
        .await
        .unwrap();
    handle.unwrap()
}

async fn next_data(events: &mut mpsc::UnboundedReceiver<SsmOut>) -> Vec<u8> {
    loop {
        match tokio::time::timeout(Duration::from_secs(8), events.recv())
            .await
            .unwrap()
            .unwrap()
        {
            SsmOut::Data(bytes) => return bytes,
            SsmOut::Error(error) => panic!("unexpected transport error: {error}"),
            SsmOut::Close => panic!("transport closed before data"),
        }
    }
}

#[tokio::test]
async fn websocket_shell_orders_output_preserves_bytes_and_terminates_once() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let service = Arc::new(MockService::new(format!(
        "ws://{}",
        listener.local_addr().unwrap()
    )));
    let (duplicate_seen, duplicate_acknowledged) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener, "start-token").await;
        handshake(&mut socket, false).await;
        let input = read_frame(&mut socket).await;
        assert_eq!(input.payload_type, OUTPUT);
        assert_eq!(input.payload, &[0, 0x1b, b'[', b'A', 0xff]);
        ack_input(&mut socket, &input).await;
        output_frame(&mut socket, 3, OUTPUT, b"second".to_vec()).await;
        assert_eq!(read_frame(&mut socket).await.message_type, "acknowledge");
        output_frame(&mut socket, 2, OUTPUT, b"first".to_vec()).await;
        assert_eq!(read_frame(&mut socket).await.message_type, "acknowledge");
        // Duplicate acknowledged output must not be displayed twice.
        output_frame(&mut socket, 2, OUTPUT, b"first".to_vec()).await;
        assert_eq!(read_frame(&mut socket).await.message_type, "acknowledge");
        duplicate_seen.send(()).unwrap();
        while let Some(message) = socket.next().await {
            if matches!(message, Ok(Message::Close(_))) {
                break;
            }
        }
    });
    let (sink, mut events) = event_sink();
    let handle = opened(service.clone(), sink).await;
    handle.write(&[0, 0x1b, b'[', b'A', 0xff]).unwrap();
    assert_eq!(next_data(&mut events).await, b"first");
    assert_eq!(next_data(&mut events).await, b"second");
    duplicate_acknowledged.await.unwrap();
    handle.control.close().await.unwrap();
    handle.control.close().await.unwrap();
    assert_eq!(service.starts.load(Ordering::SeqCst), 1);
    assert_eq!(service.terminates.load(Ordering::SeqCst), 1);
    assert!(handle.write(b"closed").is_err());
    server.await.unwrap();
}

#[tokio::test]
async fn retransmission_reuses_frame_identity_and_ack_cannot_remove_another_frame() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(MaybeTlsStream::Plain(tcp))
            .await
            .unwrap();
        let first = read_frame(&mut socket).await;
        let resent = read_frame(&mut socket).await;
        assert_eq!(first.id, resent.id);
        assert_eq!(first.sequence, resent.sequence);
        assert_eq!(first.payload, resent.payload);
    });
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let mut protocol = Protocol::default();
    protocol.queue(OUTPUT, b"one".to_vec()).unwrap();
    protocol.flush(&mut socket).await.unwrap();
    let pending = protocol.outgoing.get_mut(&0).unwrap();
    pending.last_sent = Some(Instant::now() - RETRANSMIT_AFTER - Duration::from_secs(1));
    let original_id = pending.id;
    protocol.retransmit(&mut socket).await.unwrap();
    let wrong = Frame::input(0, OUTPUT, Vec::new()).acknowledgement();
    assert!(protocol.acknowledge(&wrong).is_err());
    assert!(protocol.outgoing.contains_key(&0));
    let mut correct = Frame::input(0, OUTPUT, Vec::new());
    correct.id = original_id;
    protocol.acknowledge(&correct.acknowledgement()).unwrap();
    assert!(protocol.outgoing.is_empty());
    server.await.unwrap();
}

#[tokio::test]
async fn lifecycle_reconcile_terminates_ssm_and_enforces_write_owner() {
    use crate::commands::{aws_ssm, lifecycle};
    use crate::state::{SessionKind, SessionOwner, SessionPhase};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let service = Arc::new(MockService::new(format!(
        "ws://{}",
        listener.local_addr().unwrap()
    )));
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener, "start-token").await;
        handshake(&mut socket, false).await;
        while let Some(message) = socket.next().await {
            if matches!(message, Ok(Message::Close(_))) {
                break;
            }
        }
    });
    let (host, _events) = crate::ssh::test_support::headless_host();
    let state = host.state();
    let owner = SessionOwner::Headless(uuid::Uuid::new_v4());
    let other = SessionOwner::Headless(uuid::Uuid::new_v4());
    let id = uuid::Uuid::new_v4().to_string();
    let reservation =
        lifecycle::reserve_resource(state, &id, SessionKind::Ssm, owner.clone()).unwrap();
    reservation.bind_discovery_source("test-source").unwrap();
    assert_eq!(
        state.lifecycle_sessions.lock().unwrap()[&id]
            .discovery_source_id
            .as_deref(),
        Some("test-source")
    );
    assert!(reservation.bind_discovery_source("another-source").is_err());
    let operation = reservation.pending_operation().unwrap();
    let (sink, _events) = event_sink();
    let opened = open_with_service(
        service.clone(),
        id.clone(),
        connector(),
        80,
        24,
        sink,
        operation,
    )
    .await
    .unwrap();
    opened
        .activate(|id, handle| {
            reservation.activate_returned(id, lifecycle::ReadySession::Ssm(handle))
        })
        .await
        .unwrap();
    assert_eq!(
        aws_ssm::aws_ssm_write(other.clone(), state, id.clone(), b"bad".to_vec())
            .unwrap_err()
            .code(),
        "session_owner_mismatch"
    );
    assert_eq!(
        aws_ssm::aws_ssm_resize(other.clone(), state, id.clone(), 90, 30)
            .unwrap_err()
            .code(),
        "session_owner_mismatch"
    );
    assert_eq!(
        lifecycle::reconcile_owner(state, &other, vec![])
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        lifecycle::reconcile_owner(state, &owner, vec![])
            .await
            .unwrap(),
        1
    );
    assert_eq!(service.terminates.load(Ordering::SeqCst), 1);
    assert!(!state.ssm_sessions.lock().unwrap().contains_key(&id));
    assert_eq!(
        state.lifecycle_sessions.lock().unwrap()[&id].phase(),
        SessionPhase::Closed
    );
    server.await.unwrap();
}

#[tokio::test]
async fn kms_handshake_challenge_and_bidirectional_encryption() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let service = Arc::new(MockService::new(format!(
        "ws://{}",
        listener.local_addr().unwrap()
    )));
    let (exit_seen, exit_acknowledged) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener, "start-token").await;
        let agent = handshake(&mut socket, true).await.unwrap();
        let input = read_frame(&mut socket).await;
        assert_eq!(agent.decrypt(&input.payload).unwrap(), b"whoami\r");
        ack_input(&mut socket, &input).await;
        output_frame(
            &mut socket,
            3,
            OUTPUT,
            agent.encrypt(b"ssm-user\r\n").unwrap(),
        )
        .await;
        assert_eq!(read_frame(&mut socket).await.message_type, "acknowledge");
        output_frame(
            &mut socket,
            4,
            STDERR,
            agent.encrypt(b"stderr output\r\n").unwrap(),
        )
        .await;
        assert_eq!(read_frame(&mut socket).await.message_type, "acknowledge");
        output_frame(&mut socket, 5, EXIT_CODE, agent.encrypt(b"0").unwrap()).await;
        assert_eq!(read_frame(&mut socket).await.message_type, "acknowledge");
        exit_seen.send(()).unwrap();
        while let Some(message) = socket.next().await {
            if matches!(message, Ok(Message::Close(_))) {
                break;
            }
        }
    });
    let (sink, mut events) = event_sink();
    let handle = opened(service.clone(), sink).await;
    handle.write_line("whoami").unwrap();
    assert_eq!(next_data(&mut events).await, b"ssm-user\r\n");
    assert_eq!(next_data(&mut events).await, b"stderr output\r\n");
    exit_acknowledged.await.unwrap();
    handle.control.close().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn encrypted_exit_code_must_authenticate_before_it_is_accepted() {
    let key: Vec<u8> = (0..64).collect();
    let agent = Encryption::new([&key[32..], &key[..32]].concat()).unwrap();
    let mut protocol = Protocol {
        encryption: Some(Encryption::new(key).unwrap()),
        handshake_complete: true,
        ..Protocol::default()
    };
    let service = MockService::new(String::new());
    let (sink, _) = event_sink();
    let (output, _) = broadcast::channel(1);
    let valid = Frame::input(0, EXIT_CODE, agent.encrypt(b"0").unwrap());
    let mut tampered = valid.clone();
    *tampered.payload.last_mut().unwrap() ^= 1;
    assert!(process_payload(
        &mut protocol,
        tampered,
        &service,
        "aws-session",
        "i-test",
        "id",
        &sink,
        &output
    )
    .await
    .is_err());
    process_payload(
        &mut protocol,
        valid,
        &service,
        "aws-session",
        "i-test",
        "id",
        &sink,
        &output,
    )
    .await
    .unwrap();
}

#[test]
fn kms_data_key_context_matches_agent_decryption_context() {
    let legacy = data_key_request("kms-test", "aws-session", "i-test", None);
    assert_eq!(
        legacy,
        json!({
            "KeyId":"kms-test", "NumberOfBytes":64,
            "EncryptionContext":{
                "aws:ssm:SessionId":"aws-session", "aws:ssm:TargetId":"i-test",
            },
        })
    );
    let challenged = data_key_request("kms-test", "aws-session", "i-test", Some("opaque+/=="));
    assert_eq!(
        challenged["EncryptionContext"],
        json!({
            "aws:ssm:SessionId":"aws-session", "aws:ssm:TargetId":"i-test",
            "aws:ssm:RandomChallenge":"opaque+/==",
        })
    );
}

#[tokio::test]
async fn kms_handshake_acknowledges_and_passes_agent_context_challenge() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let service = Arc::new(MockService::new(format!(
        "ws://{}",
        listener.local_addr().unwrap()
    )));
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener, "start-token").await;
        // New agents require this acknowledgement when
        // Kms.RequireKMSChallengeResponse is enabled. The helper also verifies
        // the subsequent encrypted challenge, which is a separate exchange.
        handshake_with_challenge(&mut socket, true, Some("agent-context-challenge+/=="))
            .await
            .unwrap();
        while let Some(message) = socket.next().await {
            if matches!(message, Ok(Message::Close(_))) {
                break;
            }
        }
    });
    let (sink, _) = event_sink();
    let handle = opened(service.clone(), sink).await;
    assert_eq!(
        *service.data_key_challenges.lock().unwrap(),
        [Some("agent-context-challenge+/==".into())]
    );
    handle.control.close().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn output_burst_does_not_starve_terminal_input() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let service = Arc::new(MockService::new(format!(
        "ws://{}",
        listener.local_addr().unwrap()
    )));
    let (begin, begin_burst) = oneshot::channel();
    let (input_seen, input_received) = oneshot::channel();
    let (stop, stop_server) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener, "start-token").await;
        handshake(&mut socket, false).await;
        begin_burst.await.unwrap();
        // Buffer a whole burst before flushing, so output remains ready while
        // the first output callback enqueues Ctrl-C on the client.
        for sequence in 2..514 {
            let mut frame = Frame::input(sequence, OUTPUT, vec![b'x']);
            frame.message_type = "output_stream_data".into();
            socket
                .feed(Message::Binary(frame.encode().unwrap().into()))
                .await
                .unwrap();
        }
        socket.flush().await.unwrap();
        let mut acknowledgements = 0;
        loop {
            let frame = read_frame(&mut socket).await;
            if frame.message_type == "acknowledge" {
                acknowledgements += 1;
                assert!(acknowledgements < 64, "output starved queued Ctrl-C");
            } else {
                assert_eq!(frame.payload_type, OUTPUT);
                assert_eq!(frame.payload, b"\x03");
                ack_input(&mut socket, &frame).await;
                input_seen.send(()).unwrap();
                break;
            }
        }
        stop_server.await.unwrap();
    });
    let handle_slot = Arc::new(Mutex::new(None::<SsmHandle>));
    let output_count = Arc::new(AtomicUsize::new(0));
    let sink_slot = handle_slot.clone();
    let sink: SsmSink = Arc::new(move |_, event| {
        if matches!(event, SsmOut::Data(_)) && output_count.fetch_add(1, Ordering::SeqCst) == 0 {
            sink_slot
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .write(b"\x03")
                .unwrap();
        }
    });
    let handle = opened(service, sink).await;
    *handle_slot.lock().unwrap() = Some(handle.clone());
    begin.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), input_received)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle.control.close())
        .await
        .expect("client cleanup timed out")
        .unwrap();
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server close timed out")
        .unwrap();
}

#[tokio::test]
async fn disconnect_resumes_existing_shell_with_fresh_token_and_sequences() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let service = Arc::new(MockService::new(format!(
        "ws://{}",
        listener.local_addr().unwrap()
    )));
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener, "start-token").await;
        handshake(&mut socket, false).await;
        let input = read_frame(&mut socket).await;
        // Deliberately lose the ACK and socket. The exact encrypted/binary
        // envelope, including UUID and sequence, must be replayed on resume.
        drop(socket);
        let mut resumed = accept(&listener, "resume-token").await;
        let replay = read_frame(&mut resumed).await;
        assert_eq!(replay.sequence, input.sequence);
        assert_eq!(replay.id, input.id);
        assert_eq!(replay.payload, input.payload);
        ack_input(&mut resumed, &replay).await;
        output_frame(&mut resumed, 2, OUTPUT, b"resumed".to_vec()).await;
        assert_eq!(read_frame(&mut resumed).await.message_type, "acknowledge");
        while let Some(message) = resumed.next().await {
            if matches!(message, Ok(Message::Close(_))) {
                break;
            }
        }
    });
    let (sink, mut events) = event_sink();
    let handle = opened(service.clone(), sink).await;
    handle.write(b"echo resumed\r").unwrap();
    assert_eq!(next_data(&mut events).await, b"resumed");
    assert_eq!(service.starts.load(Ordering::SeqCst), 1);
    assert_eq!(service.resumes.load(Ordering::SeqCst), 1);
    handle.control.close().await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn cancelled_opener_retains_late_start_response_until_termination() {
    let service = Arc::new(MockService {
        start_release: Some(Notify::new()),
        ..MockService::new("ws://127.0.0.1:1".into())
    });
    let (operation, pending) = PendingOperation::new();
    let (sink, _) = event_sink();
    let task_service = service.clone();
    let opener = tokio::spawn(async move {
        open_with_service(
            task_service,
            uuid::Uuid::new_v4().to_string(),
            connector(),
            80,
            24,
            sink,
            operation,
        )
        .await
    });
    service.start_entered.notified().await;
    let cleanup = crate::resource::ResourceCleanup::pending(pending);
    opener.abort();
    assert!(matches!(opener.await, Err(error) if error.is_cancelled()));
    let cleanup_task = tokio::spawn(async move { cleanup.finish().await });
    tokio::task::yield_now().await;
    assert_eq!(service.terminates.load(Ordering::SeqCst), 0);
    service.start_release.as_ref().unwrap().notify_one();
    tokio::time::timeout(Duration::from_secs(3), cleanup_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(service.starts.load(Ordering::SeqCst), 1);
    assert_eq!(service.terminates.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failed_termination_is_retained_for_retry() {
    let service = Arc::new(MockService::new("ws://127.0.0.1:1".into()));
    service.fail_terminate.store(1, Ordering::SeqCst);
    let (operation, pending) = PendingOperation::new();
    let (sink, _) = event_sink();
    assert!(open_with_service(
        service.clone(),
        uuid::Uuid::new_v4().to_string(),
        connector(),
        80,
        24,
        sink,
        operation
    )
    .await
    .is_err());
    let cleanup = crate::resource::ResourceCleanup::pending(pending);
    assert!(cleanup.finish().await.is_err());
    assert!(!cleanup.is_finished());
    cleanup.finish().await.unwrap();
    assert!(cleanup.is_finished());
    assert_eq!(service.terminates.load(Ordering::SeqCst), 2);
}

#[test]
fn stream_endpoint_is_bound_to_region_session_and_tls() {
    for region in ["us-east-1", "us-gov-west-1", "cn-north-1"] {
        let suffix = if region.starts_with("cn-") {
            "amazonaws.com.cn"
        } else {
            "amazonaws.com"
        };
        let url = format!(
            "wss://ssmmessages.{region}.{suffix}/v1/data-channel/session?role=publish_subscribe"
        );
        validate_stream_url(&url, region, "session").unwrap();
        assert!(validate_stream_url(&url, region, "other").is_err());
    }
    for url in [
        "ws://ssmmessages.us-east-1.amazonaws.com/v1/data-channel/session",
        "wss://evil.example/v1/data-channel/session",
        "wss://ssmmessages.us-east-1.amazonaws.com.evil.example/v1/data-channel/session",
    ] {
        assert!(validate_stream_url(url, "us-east-1", "session").is_err());
    }
}

#[test]
fn cleanup_retains_original_principal_after_logout_and_identity_change() {
    fn credentials(arn: &str, key: &str) -> AwsCredentials {
        AwsCredentials {
            access_key_id: key.into(),
            secret_access_key: "secret".into(),
            session_token: "token".into(),
            account_id: "123456789012".into(),
            arn: arn.into(),
            expires_at: u64::MAX,
        }
    }
    let lease = CredentialLease {
        account_id: "123456789012".into(),
        cached: Mutex::new(None),
    };
    assert!(lease
        .cleanup_fallback(Err(error("aws_login_required")))
        .is_err());
    lease
        .remember(credentials(
            "arn:aws:iam::123456789012:user/original",
            "old",
        ))
        .unwrap_or_else(|e| panic!("{e}"));
    // A legitimate refresh updates the cleanup credential's expiration/key.
    lease
        .remember(credentials(
            "arn:aws:iam::123456789012:user/original",
            "renewed",
        ))
        .unwrap_or_else(|e| panic!("{e}"));
    let relogin = lease.remember(credentials(
        "arn:aws:iam::123456789012:user/other",
        "different",
    ));
    assert!(matches!(&relogin, Err(error) if error.code() == "aws_ssm_identity_changed"));
    let cleanup = lease
        .cleanup_fallback(relogin)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(cleanup.access_key_id, "renewed");
    assert_eq!(cleanup.arn, "arn:aws:iam::123456789012:user/original");
    let after_logout = lease
        .cleanup_fallback(Err(error("aws_login_required")))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(after_logout.access_key_id, "renewed");
}

fn isolated_state(dir: &std::path::Path) -> Arc<crate::state::AppState> {
    Arc::new(
        crate::state::AppState::open_with_services(
            dir.to_owned(),
            crate::state::NativeServices {
                keyring: None,
                #[cfg(any(windows, macos, linux, ohos))]
                serial: crate::terminal::serial::native_backend().unwrap(),
            },
        )
        .unwrap(),
    )
}

async fn wait_registered(state: &crate::state::AppState, id: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if state.lifecycle_sessions.lock().unwrap().contains_key(id) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("session reservation did not appear");
}

fn spawn_native_open(
    state: Arc<crate::state::AppState>,
    owner: crate::state::SessionOwner,
    source: String,
    id: String,
) -> tokio::task::JoinHandle<AppResult<String>> {
    tokio::spawn(async move {
        let host = crate::emitter::Host::new(state.clone(), Arc::new(|_, _| true));
        crate::commands::aws_ssm::aws_ssm_open(
            host,
            owner,
            &state,
            source,
            "us-east-1".into(),
            "i-0123456789abcdef0".into(),
            "123456789012".into(),
            80,
            24,
            Some(id),
        )
        .await
    })
}

#[tokio::test]
async fn source_closer_cancels_pending_open_waiting_for_source_read_gate() {
    use crate::commands::{aws_signin, lifecycle};
    use crate::state::{SessionKind, SessionOwner, SessionPhase};

    let dir = tempfile::tempdir().unwrap();
    let state = isolated_state(dir.path());
    let source = uuid::Uuid::new_v4().to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let owner = SessionOwner::Headless(uuid::Uuid::new_v4());
    let source_write = state.aws_auth.source_gate.clone().write_owned().await;
    let opener = spawn_native_open(state.clone(), owner.clone(), source.clone(), id.clone());
    wait_registered(&state, &id).await;
    assert_eq!(
        state.lifecycle_sessions.lock().unwrap()[&id].phase(),
        SessionPhase::Pending
    );
    let other_id = uuid::Uuid::new_v4().to_string();
    let other = lifecycle::reserve_resource(&state, &other_id, SessionKind::Ssm, owner).unwrap();
    other.bind_discovery_source("unrelated-source").unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        aws_signin::close_source_sessions(&state, &source),
    )
    .await
    .unwrap()
    .unwrap();
    let error = opener.await.unwrap().unwrap_err();
    assert_eq!(error.code(), "aws_ssm_open_cancelled");
    assert_eq!(
        state.lifecycle_sessions.lock().unwrap()[&id].phase(),
        SessionPhase::Closed
    );
    assert_eq!(
        state.lifecycle_sessions.lock().unwrap()[&other_id].phase(),
        SessionPhase::Pending
    );
    assert!(state.ssm_sessions.lock().unwrap().is_empty());
    drop(source_write);
    drop(other);
}

async fn assert_source_mutation_serializes_open_and_cleanup(delete_source: bool) {
    use crate::commands::{aws_signin, discovery, lifecycle};
    use crate::models::{DynamicDiscoveryConfig, DynamicDiscoverySource};
    use crate::state::{SessionKind, SessionOwner, SessionPhase};

    let dir = tempfile::tempdir().unwrap();
    let state = isolated_state(dir.path());
    let source = uuid::Uuid::new_v4().to_string();
    discovery::save_dynamic_discovery_sources_async(
        state.clone(),
        vec![DynamicDiscoverySource {
            id: source.clone(),
            name: "SSM fixture".into(),
            enabled: true,
            config: DynamicDiscoveryConfig::AwsNative {
                region: "us-east-1".into(),
                only_ssm_available: false,
            },
        }],
    )
    .await
    .unwrap();
    let credential_key = crate::aws::auth::credential_key(&source);
    // This marker can never authorize a real AWS request. It lets the test
    // observe exactly when the real logout/source-delete command removes data.
    state
        .secret_store
        .set(&credential_key, "fixture grant")
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let service = Arc::new(MockService {
        start_release: Some(Notify::new()),
        terminate_release: Some(Notify::new()),
        credential_observer: Some((state.secret_store.clone(), credential_key.clone())),
        ..MockService::new(format!("ws://{}", listener.local_addr().unwrap()))
    });
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener, "start-token").await;
        handshake(&mut socket, false).await;
        while let Some(message) = socket.next().await {
            if matches!(message, Ok(Message::Close(_))) {
                break;
            }
        }
    });
    let owner = SessionOwner::Headless(uuid::Uuid::new_v4());
    let id = uuid::Uuid::new_v4().to_string();
    let opening_state = state.clone();
    let opening_source = source.clone();
    let opening_id = id.clone();
    let opening_service = service.clone();
    let opening_owner = owner.clone();
    let opener = tokio::spawn(async move {
        let reservation = lifecycle::reserve_resource(
            &opening_state,
            &opening_id,
            SessionKind::Ssm,
            opening_owner,
        )
        .unwrap();
        reservation.bind_discovery_source(&opening_source).unwrap();
        let operation = reservation.pending_operation().unwrap();
        let _source_read = opening_state.aws_auth.source_gate.read().await;
        let (sink, _events) = event_sink();
        let opened = open_with_service(
            opening_service,
            opening_id,
            connector(),
            80,
            24,
            sink,
            operation,
        )
        .await
        .unwrap();
        opened
            .activate(|id, handle| {
                reservation.activate_returned(id, lifecycle::ReadySession::Ssm(handle))
            })
            .await
            .unwrap();
    });
    service.start_entered.notified().await;
    let mutation_state = state.clone();
    let mutation_source = source.clone();
    let mutation = tokio::spawn(async move {
        if delete_source {
            discovery::save_dynamic_discovery_sources_async(mutation_state, vec![]).await
        } else {
            aws_signin::aws_signin_logout(mutation_state, mutation_source).await
        }
    });
    // Tokio's fair RwLock rejects a new reader once the writer is queued.
    tokio::time::timeout(Duration::from_secs(3), async {
        while state.aws_auth.source_gate.try_read().is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !mutation.is_finished(),
        "logout/delete must wait for the opener's read lease"
    );
    assert_eq!(service.terminates.load(Ordering::SeqCst), 0);
    let waiting_id = uuid::Uuid::new_v4().to_string();
    let waiting = spawn_native_open(
        state.clone(),
        owner.clone(),
        source.clone(),
        waiting_id.clone(),
    );
    wait_registered(&state, &waiting_id).await;
    service.start_release.as_ref().unwrap().notify_one();
    opener.await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), service.terminate_entered.notified())
        .await
        .unwrap();
    assert!(state.secret_store.get(&credential_key).unwrap().is_some());
    assert!(
        !mutation.is_finished(),
        "grant deletion must wait for remote termination"
    );
    assert!(!discovery::list_dynamic_discovery_sources(&state)
        .unwrap()
        .is_empty());
    // Register one more opener *after* close_source_sessions took its snapshot.
    // It must stay behind the writer, then fail auth after deletion, never open
    // a new remote session in the gap between cleanup and credential removal.
    let late_id = uuid::Uuid::new_v4().to_string();
    let late = spawn_native_open(state.clone(), owner, source, late_id.clone());
    wait_registered(&state, &late_id).await;
    assert!(!late.is_finished());
    assert_eq!(
        state.lifecycle_sessions.lock().unwrap()[&late_id].phase(),
        SessionPhase::Pending
    );
    service.terminate_release.as_ref().unwrap().notify_one();
    tokio::time::timeout(Duration::from_secs(3), mutation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(state.secret_store.get(&credential_key).unwrap().is_none());
    assert_eq!(
        waiting.await.unwrap().unwrap_err().code(),
        "aws_ssm_open_cancelled"
    );
    assert_eq!(
        late.await.unwrap().unwrap_err().code(),
        "aws_login_required"
    );
    assert_eq!(service.starts.load(Ordering::SeqCst), 1);
    assert_eq!(service.terminates.load(Ordering::SeqCst), 1);
    assert!(state.ssm_sessions.lock().unwrap().is_empty());
    assert_eq!(
        state.lifecycle_sessions.lock().unwrap()[&id].phase(),
        SessionPhase::Closed
    );
    assert_eq!(
        discovery::list_dynamic_discovery_sources(&state)
            .unwrap()
            .is_empty(),
        delete_source
    );
    server.await.unwrap();
}

#[tokio::test]
async fn source_logout_waits_for_open_then_terminates_before_grant_deletion() {
    assert_source_mutation_serializes_open_and_cleanup(false).await;
}

#[tokio::test]
async fn source_deletion_waits_for_open_then_terminates_before_grant_deletion() {
    assert_source_mutation_serializes_open_and_cleanup(true).await;
}
