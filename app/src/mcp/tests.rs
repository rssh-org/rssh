use super::*;
use serde_json::{json, Value};

#[tokio::test(start_paused = true)]
async fn session_survives_maximum_approval_and_execution_wait() {
    use futures_util::StreamExt;
    use rmcp::model::{
        ClientJsonRpcMessage, ClientRequest, PingRequest, RequestId, ServerJsonRpcMessage,
        ServerResult,
    };
    use rmcp::transport::{streamable_http_server::session::SessionManager, Transport};
    use std::time::Duration;

    let manager = Arc::new(super::http::session_manager());
    let (id, mut transport) = manager.create_session().await.unwrap();
    let ping = |id| {
        ClientJsonRpcMessage::request(
            ClientRequest::PingRequest(PingRequest::default()),
            RequestId::Number(id),
        )
    };
    let initialize = tokio::spawn({
        let manager = manager.clone();
        let id = id.clone();
        async move { manager.initialize_session(&id, ping(1)).await }
    });
    assert!(transport.receive().await.is_some());
    transport
        .send(ServerJsonRpcMessage::response(
            ServerResult::empty(()),
            RequestId::Number(1),
        ))
        .await
        .unwrap();
    initialize.await.unwrap().unwrap();

    let stream = manager.create_stream(&id, ping(2)).await.unwrap();
    assert!(transport.receive().await.is_some());
    // No MCP traffic is generated during approval or terminal execution:
    // 300s approval + 300s execution + 30s result-delivery grace.
    tokio::time::advance(Duration::from_secs(300 + 300 + 30)).await;
    tokio::time::sleep(Duration::from_millis(1)).await;
    transport
        .send(ServerJsonRpcMessage::response(
            ServerResult::empty(()),
            RequestId::Number(2),
        ))
        .await
        .expect("the SDK session must outlive the command's maximum wait");
    tokio::pin!(stream);
    let response = tokio::time::timeout(Duration::from_secs(1), async {
        while let Some(event) = stream.next().await {
            if let Some(message) = event.message {
                return Some(message);
            }
        }
        None
    })
    .await
    .unwrap()
    .expect("the waiting client must receive its response");
    assert!(matches!(
        response.as_ref(),
        ServerJsonRpcMessage::Response(_)
    ));
    manager.close_session(&id).await.unwrap();
}

struct Fixture {
    state: Arc<AppState>,
    owner: SessionOwner,
    endpoint: EndpointInfo,
    client: reqwest::Client,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(
            AppState::open_with_services(
                dir.path().into(),
                rssh_core::state::NativeServices {
                    keyring: None,
                    serial: rssh_core::terminal::serial::native_backend().unwrap(),
                },
            )
            .unwrap(),
        );
        let owner = SessionOwner::Headless(uuid::Uuid::new_v4());
        let endpoint = start(state.clone(), owner.clone())
            .await
            .unwrap()
            .endpoint
            .unwrap();
        Self {
            state,
            owner,
            endpoint,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap(),
            _dir: dir,
        }
    }

    fn request(&self, body: Value, session: Option<&str>) -> reqwest::RequestBuilder {
        let request = self
            .client
            .post(&self.endpoint.url)
            .bearer_auth(&self.endpoint.token)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-11-25")
            .json(&body);
        match session {
            Some(session) => request.header("MCP-Session-Id", session),
            None => request,
        }
    }

    async fn initialize(&self) -> String {
        let response = self
            .request(
                json!({"jsonrpc":"2.0", "id":1, "method":"initialize",
            "params":{"protocolVersion":"2025-11-25", "capabilities":{},
                "clientInfo":{"name":"rssh-test", "version":"1"}}}),
                None,
            )
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let session = response
            .headers()
            .get("mcp-session-id")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let result = read_result(response).await;
        assert_eq!(result["result"]["protocolVersion"], "2025-11-25");
        let response = self
            .request(
                json!({"jsonrpc":"2.0", "method":"notifications/initialized"}),
                Some(&session),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
        session
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        rssh_core::commands::lifecycle::close_owner(&self.state, &self.owner);
    }
}

async fn read_result(response: reqwest::Response) -> Value {
    let text = response.text().await.unwrap();
    if let Ok(value) = serde_json::from_str(&text) {
        return value;
    }
    text.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .find(|value| value.get("id").is_some())
        .unwrap_or_else(|| panic!("missing MCP response: {text}"))
}

#[tokio::test]
async fn streamable_http_initializes_and_exposes_only_terminal_tools() {
    let fixture = Fixture::new().await;
    let session = fixture.initialize().await;
    let response = fixture
        .request(
            json!({"jsonrpc":"2.0", "id":2, "method":"tools/list"}),
            Some(&session),
        )
        .send()
        .await
        .unwrap();
    let result = read_result(response).await;
    let names: Vec<_> = result["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "list_sessions",
            "open_session",
            "run_command",
            "download_file"
        ]
    );
    assert!(result["result"]["tools"][2]["inputSchema"]["properties"]
        .get("request_id")
        .is_none());
    let download_schema = &result["result"]["tools"][3]["inputSchema"];
    let mut expected_schema = rssh_core::ai::tools::all_tools()
        .into_iter()
        .find(|tool| tool.name == "download_file")
        .unwrap()
        .input_schema;
    expected_schema["properties"]["session_id"] = json!({
        "type":"string", "description":"Exact session_id returned by list_sessions."
    });
    expected_schema["required"]
        .as_array_mut()
        .unwrap()
        .push(json!("session_id"));
    expected_schema["additionalProperties"] = json!(false);
    assert_eq!(download_schema, &expected_schema);
    let response = fixture
        .request(
            json!({"jsonrpc":"2.0", "id":3, "method":"tools/call",
        "params":{"name":"list_sessions", "arguments":{}}}),
            Some(&session),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        read_result(response).await["result"]["structuredContent"]["sessions"],
        json!([])
    );
    let response = fixture
        .request(
            json!({"jsonrpc":"2.0", "id":4, "method":"tools/call",
        "params":{"name":"run_command", "arguments":{"session_id":"not-shared", "cmd":"echo hello",
            "explain":"Read a sample", "side_effect":"read-only"}}}),
            Some(&session),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(read_result(response).await["result"]["isError"], true);
}

#[tokio::test]
async fn rejects_unauthenticated_and_cross_origin_requests() {
    let fixture = Fixture::new().await;
    let body = json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{}});
    let response = fixture
        .client
        .post(&fixture.endpoint.url)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
    let response = fixture
        .request(body.clone(), None)
        .header("Origin", "https://untrusted.example")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    let response = fixture
        .request(body, None)
        .header("Host", "untrusted.example")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn restarting_rotates_endpoint_credentials() {
    let fixture = Fixture::new().await;
    let same = start(fixture.state.clone(), fixture.owner.clone())
        .await
        .unwrap()
        .endpoint
        .unwrap();
    assert_eq!(same.token, fixture.endpoint.token);
    stop(&fixture.state, &fixture.owner).unwrap();
    let replacement = start(fixture.state.clone(), fixture.owner.clone())
        .await
        .unwrap()
        .endpoint
        .unwrap();
    assert_ne!(replacement.token, fixture.endpoint.token);
    let response = fixture
        .client
        .post(&replacement.url)
        .bearer_auth(&fixture.endpoint.token)
        .json(&json!({"jsonrpc":"2.0", "id":1, "method":"tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
}

fn terminal(
    fixture: &Fixture,
    tab_id: &str,
) -> (
    String,
    rssh_core::emitter::Host,
    tokio::sync::mpsc::UnboundedReceiver<(String, Value)>,
) {
    let (events, received) = tokio::sync::mpsc::unbounded_channel();
    let sink: rssh_core::emitter::Sink =
        Arc::new(move |event, payload| events.send((event.to_owned(), payload)).is_ok());
    let host = rssh_core::emitter::Host::new(fixture.state.clone(), sink.clone());
    let id = rssh_core::commands::pty::pty_spawn(
        host.clone(),
        fixture.owner.clone(),
        &fixture.state,
        80,
        24,
        None,
    )
    .unwrap();
    agent::register_session(
        &fixture.state,
        &fixture.owner,
        sink,
        id.clone(),
        tab_id.into(),
        "Test terminal".into(),
    )
    .unwrap();
    (id, host, received)
}

async fn next_event(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<(String, Value)>,
    prefix: &str,
) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let (event, payload) = events.recv().await.unwrap();
            if event.starts_with(prefix) {
                return payload;
            }
        }
    })
    .await
    .unwrap()
}

async fn open_terminal(
    fixture: &Fixture,
    protocol_session: &str,
    terminal_id: &str,
    host: rssh_core::emitter::Host,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<(String, Value)>,
) -> rssh_core::ai::commands::AiSessionInfo {
    let request = fixture.request(json!({"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":{"name":"open_session", "arguments":{"session_id":terminal_id,"title":"Diagnose terminal"}}}), Some(protocol_session));
    let result = tokio::spawn(async move { read_result(request.send().await.unwrap()).await });
    let requested = next_event(events, "agent:session_requested:").await;
    let request_id = requested["id"].as_str().unwrap();
    let info = agent::accept_session(&fixture.state, host, &fixture.owner, request_id)
        .await
        .unwrap();
    assert!(
        !result.is_finished(),
        "open_session must await frontend subscriptions"
    );
    agent::session_ready(
        &fixture.state,
        &fixture.owner,
        request_id,
        &info.instance_id,
    )
    .unwrap();
    let response = result.await.unwrap();
    assert_eq!(
        response["result"]["structuredContent"]["conversation_id"],
        info.conversation_id
    );
    assert_eq!(response["result"]["isError"], false);
    info
}

#[tokio::test]
async fn command_call_uses_the_ai_conversation_after_session_acceptance() {
    let fixture = Fixture::new().await;
    let (id, host, mut events) = terminal(&fixture, "tab-test");
    let session = fixture.initialize().await;
    let info = open_terminal(&fixture, &session, &id, host, &mut events).await;
    assert_eq!(info.source, rssh_core::ai::session::SessionSource::Mcp);
    let request = fixture.request(json!({"jsonrpc":"2.0", "id":3, "method":"tools/call", "params":{"name":"run_command", "arguments":{"session_id":id,"cmd":"echo hello","explain":"Inspect terminal","side_effect":"read-only"}}}), Some(&session));
    let result = tokio::spawn(async move { read_result(request.send().await.unwrap()).await });
    let proposal = next_event(&mut events, "ai:command_proposed:").await;
    rssh_core::ai::commands::ai_command_result_impl(
        &fixture.state,
        "tab-test",
        &fixture.owner,
        proposal["id"].as_str().unwrap().into(),
        0,
        "hello\n".into(),
        false,
        false,
        Some(&info.instance_id),
    )
    .await
    .unwrap();
    let response = result.await.unwrap();
    assert_eq!(response["result"]["structuredContent"]["output"], "hello\n");
    assert_eq!(response["result"]["structuredContent"]["exit_code"], 0);
    assert_eq!(response["result"]["isError"], false);
}

#[tokio::test]
async fn download_requires_an_accepted_conversation_and_an_ssh_target() {
    let fixture = Fixture::new().await;
    let (id, host, mut events) = terminal(&fixture, "tab-download");
    let session = fixture.initialize().await;
    let call = |request_id, max_mb| {
        fixture.request(
            json!({"jsonrpc":"2.0", "id":request_id, "method":"tools/call",
            "params":{"name":"download_file", "arguments":{
                "session_id":id, "remote_path":"/tmp/profile.pprof", "max_mb":max_mb
            }}}),
            Some(&session),
        )
    };
    let denied = read_result(call(3, 1).send().await.unwrap()).await;
    assert_eq!(denied["result"]["isError"], true);
    assert_eq!(
        denied["result"]["structuredContent"]["code"],
        "agent_session_unavailable"
    );

    open_terminal(&fixture, &session, &id, host, &mut events).await;
    let local = read_result(call(4, 1).send().await.unwrap()).await;
    assert_eq!(local["result"]["isError"], true);
    assert_eq!(local["result"]["structuredContent"]["session_id"], id);
    assert_eq!(local["result"]["structuredContent"]["ok"], false);
    assert!(local["result"]["structuredContent"]["local_path"].is_null());
    assert!(local["result"]["structuredContent"]["bytes"].is_null());
    assert!(local["result"]["structuredContent"]["message"]
        .as_str()
        .unwrap()
        .contains("SFTP"));

    // Validation takes precedence over opening an SSH subsystem or displaying
    // an approval card, and must reject both sides of the advertised limit.
    for (request_id, max_mb) in [(5, 101), (6, 0)] {
        let response = read_result(call(request_id, max_mb).send().await.unwrap()).await;
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(response["result"]["structuredContent"]["ok"], false);
        assert!(response["result"]["structuredContent"]["message"]
            .as_str()
            .unwrap()
            .contains("100"));
    }
    let malformed = read_result(call(7, -1).send().await.unwrap()).await;
    assert_eq!(malformed["error"]["code"], -32602);
    while let Ok((event, _)) = events.try_recv() {
        assert!(!event.starts_with("ai:download_proposed:"));
    }
}

#[tokio::test]
async fn mcp_cancellation_rejects_the_shared_ai_command_card() {
    let fixture = Fixture::new().await;
    let (id, host, mut events) = terminal(&fixture, "tab-cancel");
    let session = fixture.initialize().await;
    let info = open_terminal(&fixture, &session, &id, host, &mut events).await;
    let request = fixture.request(json!({"jsonrpc":"2.0", "id":3, "method":"tools/call", "params":{"name":"run_command", "arguments":{"session_id":id,"cmd":"echo hello","explain":"Inspect terminal","side_effect":"read-only"}}}), Some(&session));
    let pending = tokio::spawn(async move { request.send().await });
    let proposal = next_event(&mut events, "ai:command_proposed:").await;
    let response = fixture.request(json!({"jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":3,"reason":"User stopped this request"}}), Some(&session)).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    let rejected = next_event(&mut events, "ai:command_rejected:").await;
    assert_eq!(rejected["id"], proposal["id"]);
    pending.abort();

    let request = fixture.request(json!({"jsonrpc":"2.0", "id":4, "method":"tools/call", "params":{"name":"run_command", "arguments":{"session_id":id,"cmd":"echo next","explain":"Continue after cancellation","side_effect":"read-only"}}}), Some(&session));
    let result = tokio::spawn(async move { read_result(request.send().await.unwrap()).await });
    let next = next_event(&mut events, "ai:command_proposed:").await;
    assert_ne!(next["id"], proposal["id"]);
    rssh_core::ai::commands::ai_command_result_impl(
        &fixture.state,
        "tab-cancel",
        &fixture.owner,
        next["id"].as_str().unwrap().into(),
        0,
        "next\n".into(),
        false,
        false,
        Some(&info.instance_id),
    )
    .await
    .unwrap();
    let response = result.await.unwrap();
    assert_eq!(response["result"]["structuredContent"]["output"], "next\n");
    assert_eq!(response["result"]["isError"], false);
}

#[tokio::test]
async fn mcp_cancellation_closes_the_session_request_before_admission() {
    let fixture = Fixture::new().await;
    let (id, _host, mut events) = terminal(&fixture, "tab-open-cancel");
    let session = fixture.initialize().await;
    let request = fixture.request(json!({"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":{"name":"open_session", "arguments":{"session_id":id,"title":"Inspect"}}}), Some(&session));
    let pending = tokio::spawn(async move { request.send().await });
    let requested = next_event(&mut events, "agent:session_requested:").await;
    fixture
        .request(
            json!({"jsonrpc":"2.0", "method":"notifications/cancelled", "params":{"requestId":2}}),
            Some(&session),
        )
        .send()
        .await
        .unwrap();
    let cancelled = next_event(&mut events, "agent:session_cancelled:").await;
    assert_eq!(cancelled["id"], requested["id"]);
    assert!(agent::pending_sessions(&fixture.state, &fixture.owner)
        .unwrap()
        .is_empty());
    pending.abort();
}
