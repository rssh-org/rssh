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
async fn closing_an_owner_cancels_gui_start_waiting_for_the_start_lock() {
    let fixture = Fixture::new().await;
    stop(&fixture.state, &fixture.owner).unwrap();
    let lock = fixture.state.agent_bridge.mcp_start_lock.lock().await;
    let starting = start(fixture.state.clone(), fixture.owner.clone());
    tokio::pin!(starting);
    assert!(futures_util::poll!(&mut starting).is_pending());

    rssh_core::commands::lifecycle::close_owner(&fixture.state, &fixture.owner);
    drop(lock);

    assert!(starting.await.is_err());
    assert!(fixture
        .state
        .agent_bridge
        .endpoint(&fixture.owner)
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn concurrent_gui_and_cli_starts_reuse_the_same_endpoint() {
    let fixture = Fixture::new().await;
    stop(&fixture.state, &fixture.owner).unwrap();
    let (first, second, third) = tokio::join!(
        start(fixture.state.clone(), fixture.owner.clone()),
        start(fixture.state.clone(), fixture.owner.clone()),
        start(fixture.state.clone(), fixture.owner.clone()),
    );
    let first = first.unwrap().endpoint.unwrap();
    for result in [second, third] {
        let endpoint = result.unwrap().endpoint.unwrap();
        assert_eq!(endpoint.url, first.url);
        assert_eq!(endpoint.token, first.token);
    }
    assert_ne!(first.token, fixture.endpoint.token);
}

#[tokio::test]
async fn streamable_http_initializes_and_exposes_supported_tools() {
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
    let mut names: Vec<_> = result["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "analyze_locally",
            "download_file",
            "list_sessions",
            "load_skill",
            "match_file",
            "open_session",
            "patch_file",
            "run_command"
        ]
    );
    let tool = |name| {
        result["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap()
    };
    assert!(tool("run_command")["inputSchema"]["properties"]
        .get("request_id")
        .is_none());
    for name in [
        "run_command",
        "download_file",
        "load_skill",
        "match_file",
        "patch_file",
    ] {
        let canonical = rssh_core::ai::tools::all_tools()
            .into_iter()
            .find(|tool| tool.name == name)
            .unwrap();
        let exposed = tool(name);
        assert_eq!(exposed["description"], canonical.description);
        let mut expected_schema = canonical.input_schema;
        expected_schema["properties"]["session_id"] = json!({
            "type":"string", "description":"Exact session_id returned by list_sessions and accepted through open_session."
        });
        expected_schema["required"]
            .as_array_mut()
            .unwrap()
            .push(json!("session_id"));
        expected_schema["additionalProperties"] = json!(false);
        assert_eq!(exposed["inputSchema"], expected_schema);
    }
    let analyze = tool("analyze_locally");
    let mut expected_schema = rssh_core::ai::tools::all_tools()
        .into_iter()
        .find(|tool| tool.name == "analyze_locally")
        .unwrap()
        .input_schema;
    expected_schema["additionalProperties"] = json!(false);
    assert_eq!(analyze["inputSchema"], expected_schema);
    assert!(analyze["description"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("agent"));
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
async fn skill_and_file_tools_require_session_acceptance_and_strict_arguments() {
    let fixture = Fixture::new().await;
    let session = fixture.initialize().await;
    let inputs = [
        ("load_skill", json!({"id":"user-test"})),
        ("match_file", json!({"path":"/tmp/test", "find":"old"})),
        (
            "patch_file",
            json!({"path":"/tmp/test", "find":"old", "replace":"new", "expected_count":1}),
        ),
    ];
    for (name, mut arguments) in inputs {
        let call = |arguments| {
            fixture.request(
                json!({"jsonrpc":"2.0", "id":2, "method":"tools/call",
                    "params":{"name":name, "arguments":arguments}}),
                Some(&session),
            )
        };
        let missing = read_result(call(arguments.clone()).send().await.unwrap()).await;
        assert_eq!(missing["error"]["code"], -32602, "{name}: {missing}");
        arguments["session_id"] = json!("not-shared");
        let denied = read_result(call(arguments.clone()).send().await.unwrap()).await;
        assert_eq!(denied["result"]["isError"], true, "{name}: {denied}");
        assert_eq!(
            denied["result"]["structuredContent"]["code"], "agent_session_unavailable",
            "{name}: {denied}"
        );
        arguments["unexpected"] = json!(true);
        let unknown = read_result(call(arguments).send().await.unwrap()).await;
        assert_eq!(unknown["error"]["code"], -32602, "{name}: {unknown}");
    }
}

#[tokio::test]
async fn analyze_locally_directs_the_calling_agent_to_analyze_without_rssh() {
    let fixture = Fixture::new().await;
    let session = fixture.initialize().await;
    let response = fixture
        .request(
            json!({"jsonrpc":"2.0", "id":2, "method":"tools/call",
            "params":{"name":"analyze_locally", "arguments":{
                "local_path":"/tmp/profile.pprof", "task":"Analyze this profile"
            }}}),
            Some(&session),
        )
        .send()
        .await
        .unwrap();
    let result = read_result(response).await;
    assert_eq!(result["result"]["isError"], true);
    assert_eq!(
        result["result"]["structuredContent"]["code"],
        "mcp_analyze_locally_in_agent"
    );
    let message = result["result"]["structuredContent"]["message"]
        .as_str()
        .unwrap();
    assert!(message.to_lowercase().contains("agent"), "{message}");
    assert!(message.contains("RSSH"), "{message}");
    assert!(fixture.state.ai_sessions.lock().unwrap().is_empty());
    assert!(agent::pending_sessions(&fixture.state, &fixture.owner)
        .unwrap()
        .is_empty());
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
    let shell = if cfg!(windows) { "cmd.exe" } else { "/bin/sh" };
    rssh_core::db::settings::set(&fixture.state.db, "local_shell", shell).unwrap();
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
    .unwrap_or_else(|_| panic!("timed out waiting for {prefix}"))
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
    let shell = cfg!(windows).then_some(rssh_core::ai::shell::ShellKind::Cmd);
    let info = agent::accept_session(&fixture.state, host, &fixture.owner, request_id, shell)
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

// Observe the same per-session wire stream as TerminalPane.
async fn wait_for_display(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<(String, Value)>,
    terminal_id: &str,
    expected: &str,
) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let event = format!("pty:data:{terminal_id}");
    let mut bytes = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            let (name, payload) = events.recv().await.unwrap();
            if name != event {
                continue;
            }
            bytes.extend(STANDARD.decode(payload.as_str().unwrap()).unwrap());
            if String::from_utf8_lossy(&bytes).contains(expected) {
                break;
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "missing {expected:?} in {:?}",
            String::from_utf8_lossy(&bytes)
        )
    });
    String::from_utf8_lossy(&bytes).into_owned()
}

async fn approve(
    fixture: &Fixture,
    info: &rssh_core::ai::commands::AiSessionInfo,
    terminal_id: &str,
    card: &Value,
) -> rssh_core::ai::session::AiTerminalMutation {
    rssh_core::ai::commands::ai_terminal_command(
        fixture.owner.clone(),
        &fixture.state,
        info.tab_id.clone(),
        info.instance_id.clone(),
        card["id"].as_str().unwrap().into(),
        terminal_id.into(),
        rssh_core::ai::session::TerminalCommandAction::Approve {
            bracketed_paste: false,
        },
    )
    .await
    .unwrap()
    .expect("command completion is replayable")
}

#[tokio::test]
async fn command_call_uses_the_ai_conversation_after_session_acceptance() {
    let fixture = Fixture::new().await;
    let (id, host, mut events) = terminal(&fixture, "tab-test");
    let session = fixture.initialize().await;
    let info = open_terminal(&fixture, &session, &id, host, &mut events).await;
    assert_eq!(info.source, rssh_core::ai::session::SessionSource::Mcp);
    // Prepare state in the existing interactive shell. The command reads a
    // file through a relative path and a variable that only this shell knows.
    let dir = fixture._dir.path();
    std::fs::write(dir.join("source.txt"), "stream-visible\r\n").unwrap();
    let path = dir.display().to_string();
    std::fs::write(dir.join("checkpoint.txt"), "shell-checkpoint\r\n").unwrap();
    let (setup, command, checkpoint) = if cfg!(windows) {
        (
            format!(r#"cd /d "{path}" && set "RSSH_FILE=source.txt" && type checkpoint.txt"#),
            "type %RSSH_FILE% & set /p reply= & echo done",
            "type checkpoint.txt",
        )
    } else {
        (
            format!(
                "cd '{}' && export RSSH_FILE=source.txt && cat checkpoint.txt",
                path.replace('\'', "'\\''")
            ),
            "cat \"$RSSH_FILE\"; read -r reply; echo done",
            "cat checkpoint.txt",
        )
    };
    let write = |line: &str| {
        rssh_core::commands::pty::pty_write(
            &fixture.state,
            id.clone(),
            format!("{line}\r").into_bytes(),
        )
        .unwrap();
    };
    write(&setup);
    wait_for_display(&mut events, &id, "shell-checkpoint").await;
    let request = fixture.request(json!({"jsonrpc":"2.0", "id":3, "method":"tools/call", "params":{"name":"run_command", "arguments":{"session_id":id,"cmd":command,"explain":"Inspect terminal","side_effect":"read-only"}}}), Some(&session));
    let result = tokio::spawn(async move { read_result(request.send().await.unwrap()).await });
    let proposal = next_event(&mut events, "ai:command_proposed:").await;
    assert!(!result.is_finished());

    // Lose the first reply after its write. A second approval joins the
    // committed execution even while the shell is still waiting for input.
    let mut approval = Box::pin(approve(&fixture, &info, &id, &proposal));
    let display = tokio::select! {
        _ = &mut approval => panic!("shell is still waiting for input"),
        display = wait_for_display(&mut events, &id, "stream-visible") => display,
    };
    assert!(display.contains("RSSH_FILE"), "{display:?}"); // the command echo
    assert!(!result.is_finished());
    drop(approval);
    let mut retry = Box::pin(approve(&fixture, &info, &id, &proposal));
    tokio::select! {
        _ = &mut retry => panic!("a duplicate approval must await the original result"),
        _ = tokio::task::yield_now() => {}
    }
    write("ack");
    let completion = retry.await;
    let replay = approve(&fixture, &info, &id, &proposal).await;
    assert_eq!(completion.payload, replay.payload);
    write(checkpoint);
    let after = wait_for_display(&mut events, &id, "shell-checkpoint").await;
    assert_eq!(display.matches("stream-visible").count(), 1);
    assert!(
        !after.contains("stream-visible"),
        "replayed command: {after}"
    );
    let response = result.await.unwrap();
    let output = response["result"]["structuredContent"]["output"]
        .as_str()
        .unwrap();
    assert!(
        output.contains("stream-visible") && output.ends_with("done"),
        "{output:?}"
    );
    assert!(
        !output.contains("RSSH_FILE") && !output.contains("__rssh_done_"),
        "{output:?}"
    );
    assert_eq!(response["result"]["structuredContent"]["exit_code"], 0);
    assert_eq!(response["result"]["isError"], false);
}

#[tokio::test]
async fn download_requires_acceptance_and_copies_a_local_target_file() {
    let fixture = Fixture::new().await;
    let (id, host, mut events) = terminal(&fixture, "tab-download");
    let session = fixture.initialize().await;
    let source = fixture._dir.path().join("profile.pprof");
    std::fs::write(&source, b"local artifact").unwrap();
    let call = |request_id, max_mb| {
        fixture.request(
            json!({"jsonrpc":"2.0", "id":request_id, "method":"tools/call",
            "params":{"name":"download_file", "arguments":{
                "session_id":id, "remote_path":source, "max_mb":max_mb
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

    let info = open_terminal(&fixture, &session, &id, host, &mut events).await;
    let request = call(4, 1);
    let pending = tokio::spawn(async move { read_result(request.send().await.unwrap()).await });
    let proposal = next_event(&mut events, "ai:download_proposed:").await;
    let destination =
        std::path::Path::new(proposal["dest_dir"].as_str().unwrap()).join("profile.pprof");
    assert!(!destination.exists());
    rssh_core::ai::commands::ai_command_result_impl(
        &fixture.state,
        "tab-download",
        &fixture.owner,
        proposal["id"].as_str().unwrap().into(),
        0,
        String::new(),
        false,
        false,
        Some(&info.instance_id),
    )
    .await
    .unwrap();
    let local = pending.await.unwrap();
    assert_eq!(local["result"]["isError"], false);
    assert_eq!(local["result"]["structuredContent"]["session_id"], id);
    assert_eq!(local["result"]["structuredContent"]["ok"], true);
    assert_eq!(
        local["result"]["structuredContent"]["local_path"],
        destination.to_str().unwrap()
    );
    assert_eq!(local["result"]["structuredContent"]["bytes"], 14);
    assert_eq!(std::fs::read(&destination).unwrap(), b"local artifact");
    assert_eq!(std::fs::read(&source).unwrap(), b"local artifact");

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
async fn load_skill_uses_the_catalog_and_content_snapshot_from_session_acceptance() {
    let fixture = Fixture::new().await;
    let mut skill = rssh_core::ai::skills::SkillRecord {
        id: "user-mcp-test".into(),
        name: "Inspect service".into(),
        description: "A saved inspection workflow".into(),
        content: "Original workflow content".into(),
        builtin: false,
    };
    rssh_core::ai::skills::save_user(&fixture.state.db, &skill).unwrap();
    let (id, host, mut events) = terminal(&fixture, "tab-skill");
    let session = fixture.initialize().await;
    open_terminal(&fixture, &session, &id, host, &mut events).await;

    skill.name = "Changed name".into();
    skill.description = "Changed description".into();
    skill.content = "Changed workflow content".into();
    rssh_core::ai::skills::save_user(&fixture.state.db, &skill).unwrap();
    let reopened = fixture
        .request(
            json!({"jsonrpc":"2.0", "id":3, "method":"tools/call",
            "params":{"name":"open_session", "arguments":{
                "session_id":id, "title":"Reuse accepted session"
            }}}),
            Some(&session),
        )
        .send()
        .await
        .unwrap();
    let reopened = read_result(reopened).await;
    assert_eq!(
        reopened["result"]["structuredContent"]["available_skills"],
        json!([{
            "id":"user-mcp-test", "name":"Inspect service",
            "description":"A saved inspection workflow"
        }])
    );
    let call = |request_id, skill_id| {
        fixture.request(
            json!({"jsonrpc":"2.0", "id":request_id, "method":"tools/call",
            "params":{"name":"load_skill", "arguments":{
                "session_id":id, "id":skill_id
            }}}),
            Some(&session),
        )
    };
    let loaded = read_result(call(4, "user-mcp-test").send().await.unwrap()).await;
    assert_eq!(loaded["result"]["isError"], false, "{loaded}");
    assert_eq!(loaded["result"]["structuredContent"]["session_id"], id);
    assert_eq!(loaded["result"]["structuredContent"]["ok"], true);
    let content = loaded["result"]["structuredContent"]["content"]
        .as_str()
        .unwrap();
    assert!(content.contains("Original workflow content"), "{content}");
    assert!(!content.contains("Changed"), "{content}");
    let event = next_event(&mut events, "ai:skill_loaded:").await;
    assert_eq!(event["id"], "user-mcp-test");

    let missing = read_result(call(5, "user-missing").send().await.unwrap()).await;
    assert_eq!(missing["result"]["isError"], true, "{missing}");
    assert_eq!(missing["result"]["structuredContent"]["ok"], false);
    assert!(missing["result"]["structuredContent"]["content"]
        .as_str()
        .unwrap()
        .contains("Unknown skill id"));
}

#[cfg(any(macos, linux))]
#[tokio::test]
async fn file_tools_use_the_existing_terminal_and_require_each_patch_approval() {
    let fixture = Fixture::new().await;
    let (id, host, mut events) = terminal(&fixture, "tab-files");
    let session = fixture.initialize().await;
    let info = open_terminal(&fixture, &session, &id, host, &mut events).await;
    let source = fixture._dir.path().join("source with spaces.txt");
    let original = "old value 10.0.0.7\nold value\n";
    std::fs::write(&source, original).unwrap();
    let request = fixture
        .request(
            json!({"jsonrpc":"2.0", "id":3, "method":"tools/call",
            "params":{"name":"match_file", "arguments":{
                "session_id":id, "path":source, "find":"old value", "before":0, "after":9
            }}}),
            Some(&session),
        )
        .timeout(std::time::Duration::from_secs(15));
    let matching = tokio::spawn(async move { read_result(request.send().await.unwrap()).await });
    let probe = next_event(&mut events, "ai:internal_command:").await;
    rssh_core::ai::commands::ai_terminal_command(
        fixture.owner.clone(),
        &fixture.state,
        info.tab_id.clone(),
        info.instance_id.clone(),
        probe["id"].as_str().unwrap().into(),
        id.clone(),
        rssh_core::ai::session::TerminalCommandAction::Approve {
            bracketed_paste: false,
        },
    )
    .await
    .unwrap();
    let card = next_event(&mut events, "ai:match_proposed:").await;
    assert!(!matching.is_finished());
    approve(&fixture, &info, &id, &card).await;
    let matched = matching.await.unwrap();
    assert_eq!(matched["result"]["isError"], false, "{matched}");
    let content: Value = serde_json::from_str(
        matched["result"]["structuredContent"]["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(content["count"], 2);
    assert!(content["matches"][0]["context"]
        .as_str()
        .unwrap()
        .contains("<REDACTED:ip-10>"));
    assert!(!matched.to_string().contains("10.0.0.7"));
    assert_eq!(std::fs::read_to_string(&source).unwrap(), original);

    let patch_call = |request_id, find, replace| {
        fixture
            .request(
                json!({"jsonrpc":"2.0", "id":request_id, "method":"tools/call",
                "params":{"name":"patch_file", "arguments":{
                    "session_id":id, "path":source, "find":find,
                    "replace":replace, "expected_count":2
                }}}),
                Some(&session),
            )
            .timeout(std::time::Duration::from_secs(15))
    };
    let request = patch_call(4, "old value", "new value");
    let patching = tokio::spawn(async move { read_result(request.send().await.unwrap()).await });
    for step in ["cp", "modify", "diff", "mv"] {
        let card = next_event(&mut events, "ai:patch_proposed:").await;
        assert_eq!(card["step"], step);
        assert!(!patching.is_finished());
        assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
        if step == "mv" {
            assert!(card["diff"].as_str().unwrap().contains("+new value"));
        }
        approve(&fixture, &info, &id, &card).await;
    }
    let patched = patching.await.unwrap();
    assert_eq!(patched["result"]["isError"], false, "{patched}");
    let content: Value = serde_json::from_str(
        patched["result"]["structuredContent"]["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(content["changed"], 2);
    let expected = "new value 10.0.0.7\nnew value\n";
    assert_eq!(std::fs::read_to_string(&source).unwrap(), expected);

    let request = patch_call(5, "new value", "rejected value");
    let patching = tokio::spawn(async move { read_result(request.send().await.unwrap()).await });
    let card = next_event(&mut events, "ai:patch_proposed:").await;
    rssh_core::ai::commands::ai_command_reject_impl(
        &fixture.state,
        &info.tab_id,
        &fixture.owner,
        card["id"].as_str().unwrap().into(),
        "Keep 10.0.0.7 unchanged".into(),
        Some(&info.instance_id),
    )
    .await
    .unwrap();
    let rejected = patching.await.unwrap();
    assert_eq!(rejected["result"]["isError"], true, "{rejected}");
    assert_eq!(rejected["result"]["structuredContent"]["ok"], false);
    assert!(!rejected.to_string().contains("10.0.0.7"));
    assert_eq!(std::fs::read_to_string(&source).unwrap(), expected);
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
    approve(&fixture, &info, &id, &next).await;
    let response = result.await.unwrap();
    assert_eq!(response["result"]["structuredContent"]["output"], "next");
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
