use super::*;

struct Fixture {
    service: ControlService,
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
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap(),
            owner,
            events,
            _directory: directory,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.service.control.authority)
    }

    fn post(&self, path: &str, owner: &SessionOwner) -> reqwest::RequestBuilder {
        self.client
            .post(self.url(path))
            .json(&json!({"owner_id": super::super::scope_id(owner)}))
    }

    async fn start(&self, owner: &SessionOwner) -> Value {
        let response = self.post("/v1/mcp/start", owner).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.json().await.unwrap()
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
    let targets: Value = fixture
        .client
        .get(fixture.url("/v1/targets"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        targets["targets"],
        json!([{"owner_id":"window-main", "label":"Main window"}])
    );
    let status: Value = fixture
        .post("/v1/mcp/status", &fixture.owner)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["running"], false);
    assert!(status["connection"].is_null());

    let connection = fixture.start(&fixture.owner).await;
    let again = fixture.start(&fixture.owner).await;
    assert_eq!(connection, again);
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
            StatusCode::UNAUTHORIZED
        );
    }
    let initialized = fixture.client.post(&endpoint.url).bearer_auth(endpoint.token)
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
            "protocolVersion":"2025-11-25", "capabilities":{}, "clientInfo":{"name":"control-test", "version":"1"}
        }})).send().await.unwrap();
    assert_eq!(initialized.status(), StatusCode::OK);
    let status: Value = fixture
        .post("/v1/mcp/status", &fixture.owner)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["connection"], connection);
}

#[tokio::test]
async fn rejects_browser_origins_foreign_hosts_and_invalid_requests() {
    let fixture = Fixture::new().await;
    let own_origin = format!("http://{}", fixture.service.control.authority);
    for origin in ["https://untrusted.example", "null", own_origin.as_str()] {
        for path in ["/v1/mcp/start", "/v1/mcp/status", "/v1/mcp/stop"] {
            let response = fixture
                .post(path, &fixture.owner)
                .header("Origin", origin)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        let response = fixture
            .client
            .get(fixture.url("/v1/targets"))
            .header("Origin", origin)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!response
            .headers()
            .contains_key("Access-Control-Allow-Origin"));
    }
    for host in ["untrusted.example", "localhost", "127.0.0.1"] {
        let response = fixture
            .post("/v1/mcp/start", &fixture.owner)
            .header("Host", host)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    for content_type in [
        None,
        Some("text/plain"),
        Some("application/x-www-form-urlencoded"),
    ] {
        let request = fixture
            .client
            .post(fixture.url("/v1/mcp/start"))
            .body(r#"{"owner_id":"window-main"}"#);
        let request = match content_type {
            Some(content_type) => request.header("Content-Type", content_type),
            None => request,
        };
        assert_eq!(
            request.send().await.unwrap().status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }
    let preflight = fixture
        .client
        .request(reqwest::Method::OPTIONS, fixture.url("/v1/mcp/start"))
        .header("Origin", "https://untrusted.example")
        .header("Access-Control-Request-Method", "POST")
        .header("Access-Control-Request-Headers", "Content-Type")
        .send()
        .await
        .unwrap();
    assert_eq!(preflight.status(), StatusCode::FORBIDDEN);
    assert!(!preflight
        .headers()
        .contains_key("Access-Control-Allow-Origin"));
    let too_large = fixture
        .post("/v1/mcp/start", &fixture.owner)
        .body(" ".repeat(8193))
        .send()
        .await
        .unwrap();
    assert_eq!(too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let invalid = fixture
        .post("/v1/mcp/start", &fixture.owner)
        .json(&json!({"owner_id":"window-main", "command":"other"}))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let unknown = fixture
        .post("/v1/mcp/start", &SessionOwner::Window("missing".into()))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    assert!(fixture
        .service
        .control
        .state
        .agent_bridge
        .endpoint(&fixture.owner)
        .unwrap()
        .is_none());
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
    let rejected = fixture
        .client
        .post(fixture.url("/v1/mcp/stop"))
        .header("Origin", "https://untrusted.example")
        .json(&json!({"owner_id":"window-main"}))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    assert!(control
        .state
        .agent_bridge
        .endpoint(&fixture.owner)
        .unwrap()
        .is_some());

    for _ in 0..2 {
        let response = fixture
            .post("/v1/mcp/stop", &fixture.owner)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let stopped: Value = response.json().await.unwrap();
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
    // Listener shutdown can close the connection or reject it; the old token
    // must never authenticate successfully after the stop response.
    if let Ok(response) = fixture.client
        .post(connection["url"].as_str().unwrap())
        .header("Authorization", connection["headers"]["Authorization"].as_str().unwrap())
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
            "protocolVersion":"2025-11-25", "capabilities":{}, "clientInfo":{"name":"stopped-test", "version":"1"}
        }}))
        .send().await
    {
        assert!(!response.status().is_success());
    }
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
    let discovered = fixture
        .client
        .get(fixture.url("/v1/targets"))
        .send()
        .await
        .unwrap();
    assert_eq!(discovered.status(), StatusCode::OK);
    let connection = fixture.start(&fixture.owner).await;
    let status = fixture
        .post("/v1/mcp/status", &fixture.owner)
        .send()
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(
        status.json::<Value>().await.unwrap()["connection"],
        connection
    );
    let stopped = fixture
        .post("/v1/mcp/stop", &fixture.owner)
        .send()
        .await
        .unwrap();
    assert_eq!(stopped.status(), StatusCode::OK);
    assert_eq!(stopped.json::<Value>().await.unwrap()["running"], false);
    fixture.service.shutdown();
    assert_eq!(store.0.load(Ordering::Relaxed), 0);
    assert!(
        mcp_control::read_descriptors(&fixture.service.control.state.data_dir)
            .unwrap()
            .descriptors
            .is_empty()
    );
}
