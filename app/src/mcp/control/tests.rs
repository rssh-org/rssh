use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Fixture {
    service: ControlService,
    endpoint: String,
    client: reqwest::Client,
    owner: SessionOwner,
    events: Arc<Mutex<Vec<String>>>,
    _directory: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_secret_store(None).await
    }

    async fn with_secret_store(
        secret_store: Option<Arc<dyn rssh_core::secret::SecretStore>>,
    ) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut state = AppState::open_with_services(
            directory.path().into(),
            rssh_core::state::NativeServices {
                keyring: None,
                serial: rssh_core::terminal::serial::native_backend().unwrap(),
            },
        )
        .unwrap();
        if let Some(secret_store) = secret_store {
            state.secret_store = secret_store;
        }
        let service = ControlService::start(Arc::new(state), "desktop")
            .await
            .unwrap();
        let endpoint = mcp_control::read_descriptors(directory.path())
            .unwrap()
            .descriptors
            .remove(0)
            .endpoint;
        let events = Arc::new(Mutex::new(Vec::new()));
        let emitted = events.clone();
        let owner = SessionOwner::Window("main".into());
        service
            .control
            .register(
                owner.clone(),
                "Main window".into(),
                Arc::new(move |event, _| {
                    emitted.lock().unwrap().push(event.into());
                    true
                }),
            )
            .unwrap();
        Self {
            service,
            endpoint,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            owner,
            events,
            _directory: directory,
        }
    }

    async fn call(&self, request: ControlRequest) -> Value {
        mcp_control::request(&self.endpoint, &request)
            .await
            .unwrap()
    }

    async fn raw(&self, value: &Value) -> Value {
        let mut stream = native_transport().connect(&self.endpoint).await.unwrap();
        mcp_control::write_json(&mut stream, value, RESPONSE_LIMIT)
            .await
            .unwrap();
        mcp_control::read_json(&mut stream, RESPONSE_LIMIT)
            .await
            .unwrap()
    }

    async fn start(&self, owner: &SessionOwner) -> Value {
        let response = self
            .call(ControlRequest::Start {
                owner_id: super::super::scope_id(owner),
            })
            .await;
        assert!(response.get("error").is_none(), "{response}");
        response
    }
}

#[tokio::test]
async fn discovers_empty_windows_and_bootstraps_the_existing_mcp_endpoint() {
    let fixture = Fixture::new().await;
    let control = &fixture.service.control;
    let descriptor = mcp_control::read_descriptors(&control.state.data_dir)
        .unwrap()
        .descriptors
        .remove(0);
    assert_eq!(descriptor.instance_id, control.instance_id);
    assert_eq!(descriptor.endpoint, fixture.endpoint);
    let targets = fixture.call(ControlRequest::Targets).await;
    assert_eq!(
        targets["targets"],
        json!([{"owner_id":"window-main", "label":"Main window"}])
    );
    let status = fixture
        .call(ControlRequest::Status {
            owner_id: "window-main".into(),
        })
        .await;
    assert_eq!(status["running"], false);
    assert!(status["connection"].is_null());
    let connection = fixture.start(&fixture.owner).await;
    assert_eq!(fixture.start(&fixture.owner).await, connection);
    assert_eq!(connection["transport"], "streamable-http");
    let endpoint = control
        .state
        .agent_bridge
        .endpoint(&fixture.owner)
        .unwrap()
        .unwrap();
    assert_eq!(connection["url"], endpoint.url);
    assert_eq!(
        connection["headers"]["Authorization"],
        format!("Bearer {}", endpoint.token)
    );
    assert_eq!(
        fixture.events.lock().unwrap().as_slice(),
        ["mcp:changed:window-main", "mcp:changed:window-main"]
    );
    // Only CLI control changed transport: MCP itself retains HTTP and its token.
    for token in [None, Some("wrong")] {
        let request = fixture
            .client
            .post(&endpoint.url)
            .header("Accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{}}));
        let request = match token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        assert_eq!(
            request.send().await.unwrap().status(),
            reqwest::StatusCode::UNAUTHORIZED
        );
    }
    let initialized = fixture.client.post(&endpoint.url).bearer_auth(endpoint.token)
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
            "protocolVersion":"2025-11-25", "capabilities":{}, "clientInfo":{"name":"control-test", "version":"1"}
        }})).send().await.unwrap();
    assert_eq!(initialized.status(), reqwest::StatusCode::OK);
    assert_eq!(
        fixture
            .call(ControlRequest::Status {
                owner_id: "window-main".into()
            })
            .await["connection"],
        connection
    );
}

#[tokio::test]
async fn rejects_unsupported_methods_extra_fields_and_oversized_frames() {
    let fixture = Fixture::new().await;
    for request in [
        json!({"method":"arbitrary_command", "params":{}}),
        json!({"method":"start", "params":{"owner_id":"window-main", "command":"other"}}),
        json!({"method":"start", "params":{"owner_id":42}}),
        json!({"method":"targets", "extra":true}),
    ] {
        let response = fixture.raw(&request).await;
        assert_eq!(response["error"]["code"], "invalid_request", "{request}");
    }
    let mut stream = native_transport().connect(&fixture.endpoint).await.unwrap();
    stream.write_all(&3_u32.to_be_bytes()).await.unwrap();
    stream.write_all(b"bad").await.unwrap();
    let response: Value = mcp_control::read_json(&mut stream, RESPONSE_LIMIT)
        .await
        .unwrap();
    assert_eq!(response["error"]["code"], "invalid_request");
    let mut oversized = native_transport().connect(&fixture.endpoint).await.unwrap();
    oversized
        .write_u32((REQUEST_LIMIT + 1) as u32)
        .await
        .unwrap();
    // The listener rejects an oversized length without waiting for its body.
    let response: Value = mcp_control::read_json(&mut oversized, RESPONSE_LIMIT)
        .await
        .unwrap();
    assert_eq!(response["error"]["code"], "invalid_request");
    assert_eq!(
        fixture
            .call(ControlRequest::Start {
                owner_id: "missing".into()
            })
            .await["error"]["code"],
        "mcp_owner_unavailable"
    );
    assert!(fixture
        .service
        .control
        .state
        .agent_bridge
        .endpoint(&fixture.owner)
        .unwrap()
        .is_none());
    // Malformed clients must not stop the listener or consume its next request.
    assert_eq!(
        fixture.call(ControlRequest::Targets).await["targets"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn shutdown_closes_idle_clients_and_the_local_listener() {
    let fixture = Fixture::new().await;
    let mut idle = native_transport().connect(&fixture.endpoint).await.unwrap();
    // Also prove another connection can be served while one client sends nothing.
    assert!(fixture
        .call(ControlRequest::Targets)
        .await
        .get("targets")
        .is_some());
    fixture.service.shutdown();
    let mut byte = [0];
    let closed = tokio::time::timeout(Duration::from_secs(1), idle.read(&mut byte))
        .await
        .unwrap();
    assert!(matches!(closed, Ok(0)) || closed.is_err());
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if native_transport().connect(&fixture.endpoint).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn owner_closure_cancels_waiting_start_and_does_not_revoke_a_new_generation() {
    let fixture = Fixture::new().await;
    let control = fixture.service.control.clone();
    let target = control.target("window-main").unwrap();
    let lock = control.state.agent_bridge.mcp_start_lock.lock().await;
    let old = tokio::spawn({
        let control = control.clone();
        async move { control.start(target).await }
    });
    tokio::task::yield_now().await;
    control.unregister(&fixture.owner);
    control
        .register(
            fixture.owner.clone(),
            "Replacement".into(),
            Arc::new(|_, _| true),
        )
        .unwrap();
    drop(lock);
    assert_eq!(
        old.await.unwrap().unwrap_err().code(),
        "mcp_owner_unavailable"
    );
    let endpoint = fixture.start(&fixture.owner).await;
    let other = SessionOwner::Headless(uuid::Uuid::new_v4());
    control
        .register(
            other.clone(),
            "Other frontend".into(),
            Arc::new(|_, _| true),
        )
        .unwrap();
    let other_endpoint = fixture.start(&other).await;
    assert_ne!(endpoint["url"], other_endpoint["url"]);
    control.unregister(&fixture.owner);
    assert!(control
        .state
        .agent_bridge
        .endpoint(&fixture.owner)
        .unwrap()
        .is_none());
    assert!(control
        .state
        .agent_bridge
        .endpoint(&other)
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn shutdown_revokes_waiting_requests_endpoints_and_discovery() {
    let fixture = Fixture::new().await;
    let control = fixture.service.control.clone();
    fixture.start(&fixture.owner).await;
    let lock = control.state.agent_bridge.mcp_start_lock.lock().await;
    let target = control.target("window-main").unwrap();
    let pending = tokio::spawn({
        let control = control.clone();
        async move { control.start(target).await }
    });
    tokio::task::yield_now().await;
    fixture.service.shutdown();
    drop(lock);
    assert_eq!(
        pending.await.unwrap().unwrap_err().code(),
        "mcp_owner_unavailable"
    );
    assert!(control
        .state
        .agent_bridge
        .endpoint(&fixture.owner)
        .unwrap()
        .is_none());
    assert!(mcp_control::read_descriptors(&control.state.data_dir)
        .unwrap()
        .descriptors
        .is_empty());
    assert!(control
        .register(
            fixture.owner.clone(),
            "Closed".into(),
            Arc::new(|_, _| true)
        )
        .is_err());
    assert!(control.targets.lock().unwrap().is_empty());
}

#[tokio::test]
async fn stop_revokes_only_the_selected_endpoint_and_keeps_bootstrap_available() {
    let fixture = Fixture::new().await;
    let control = &fixture.service.control;
    let connection = fixture.start(&fixture.owner).await;
    let other = SessionOwner::Window("other".into());
    control
        .register(other.clone(), "Other".into(), Arc::new(|_, _| true))
        .unwrap();
    let other_connection = fixture.start(&other).await;
    for _ in 0..2 {
        let stopped = fixture
            .call(ControlRequest::Stop {
                owner_id: "window-main".into(),
            })
            .await;
        assert_eq!(
            stopped,
            json!({
                "running":false, "connection":null,
                "instance_id":control.instance_id, "owner_id":"window-main"
            })
        );
    }
    assert!(control
        .state
        .agent_bridge
        .endpoint(&fixture.owner)
        .unwrap()
        .is_none());
    assert_eq!(
        control
            .state
            .agent_bridge
            .endpoint(&other)
            .unwrap()
            .unwrap()
            .url,
        other_connection["url"]
    );
    assert_eq!(
        fixture.events.lock().unwrap().last().unwrap(),
        "mcp:changed:window-main"
    );
    // The stopped MCP listener must not accept the old MCP bearer token.
    if let Ok(response) = fixture.client
        .post(connection["url"].as_str().unwrap())
        .header("Authorization", connection["headers"]["Authorization"].as_str().unwrap())
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
            "protocolVersion":"2025-11-25", "capabilities":{}, "clientInfo":{"name":"stopped-test", "version":"1"}
        }})).send().await
    { assert!(!response.status().is_success()); }
    let restarted = fixture.start(&fixture.owner).await;
    assert_ne!(
        restarted["headers"]["Authorization"],
        connection["headers"]["Authorization"]
    );
    assert_eq!(
        mcp_control::read_descriptors(&control.state.data_dir)
            .unwrap()
            .descriptors
            .len(),
        1
    );
}

#[tokio::test]
async fn stop_waits_for_an_earlier_start_and_rejects_a_replaced_owner() {
    let fixture = Fixture::new().await;
    let control = fixture.service.control.clone();
    let target = control.target("window-main").unwrap();
    let lock = control.state.agent_bridge.mcp_start_lock.lock().await;
    let starting = tokio::spawn({
        let control = control.clone();
        let target = target.clone();
        async move { control.start(target).await }
    });
    tokio::task::yield_now().await;
    let stopping = tokio::spawn({
        let control = control.clone();
        let target = target.clone();
        async move { control.stop(target).await }
    });
    tokio::task::yield_now().await;
    drop(lock);
    starting.await.unwrap().unwrap();
    stopping.await.unwrap().unwrap();
    assert!(control
        .state
        .agent_bridge
        .endpoint(&fixture.owner)
        .unwrap()
        .is_none());

    control.unregister(&fixture.owner);
    control
        .register(
            fixture.owner.clone(),
            "Replacement".into(),
            Arc::new(|_, _| true),
        )
        .unwrap();
    let restarted = fixture.start(&fixture.owner).await;
    assert_eq!(
        control.stop(target).await.unwrap_err().code(),
        "mcp_owner_unavailable"
    );
    assert_eq!(
        control
            .state
            .agent_bridge
            .endpoint(&fixture.owner)
            .unwrap()
            .unwrap()
            .url,
        restarted["url"]
    );
}

#[derive(Default)]
struct UnavailableSecretStore(std::sync::atomic::AtomicUsize);

impl UnavailableSecretStore {
    fn unavailable<T>(&self) -> AppResult<T> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Err(AppError::other("keyring_failed", json!({})))
    }
}

impl rssh_core::secret::SecretStore for UnavailableSecretStore {
    fn get(&self, _: &str) -> AppResult<Option<String>> {
        self.unavailable()
    }
    fn set(&self, _: &str, _: &str) -> AppResult<()> {
        self.unavailable()
    }
    fn delete(&self, _: &str) -> AppResult<()> {
        self.unavailable()
    }
    fn backend_name(&self) -> &'static str {
        "unavailable"
    }
}

#[tokio::test]
async fn control_lifecycle_does_not_access_an_unavailable_secret_store() {
    let store = Arc::new(UnavailableSecretStore::default());
    let fixture = Fixture::with_secret_store(Some(store.clone())).await;
    assert!(fixture
        .call(ControlRequest::Targets)
        .await
        .get("targets")
        .is_some());
    let connection = fixture.start(&fixture.owner).await;
    assert_eq!(
        fixture
            .call(ControlRequest::Status {
                owner_id: "window-main".into()
            })
            .await["connection"],
        connection
    );
    assert_eq!(
        fixture
            .call(ControlRequest::Stop {
                owner_id: "window-main".into()
            })
            .await["running"],
        false
    );
    fixture.service.shutdown();
    assert_eq!(store.0.load(Ordering::Relaxed), 0);
    assert!(
        mcp_control::read_descriptors(&fixture.service.control.state.data_dir)
            .unwrap()
            .descriptors
            .is_empty()
    );
}
