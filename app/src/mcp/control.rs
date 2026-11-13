//! Local-only control for CLI clients of an existing MCP host.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use rssh_core::emitter::Sink;
use rssh_core::error::{locked, AppError, AppResult};
use rssh_core::mcp_control::{self, ControlDescriptor, CONTROL_VERSION};
use rssh_core::state::{AppState, SessionOwner};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::watch;

struct Target {
    owner: SessionOwner,
    label: String,
    sink: Sink,
    closed: watch::Sender<bool>,
}

pub struct Control {
    state: Arc<AppState>,
    instance_id: String,
    authority: String,
    stopped: AtomicBool,
    targets: Mutex<HashMap<String, Arc<Target>>>,
}

/// The host owns this guard, not the server task, so host shutdown cancels serving.
pub struct ControlService {
    pub control: Arc<Control>,
    shutdown: watch::Sender<bool>,
}

impl ControlService {
    pub async fn start(state: Arc<AppState>, host: &str) -> AppResult<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let instance_id = uuid::Uuid::new_v4().to_string();
        let control = Arc::new(Control {
            state,
            instance_id: instance_id.clone(),
            authority: address.to_string(),
            stopped: AtomicBool::new(false),
            targets: Mutex::new(HashMap::new()),
        });
        let descriptor = ControlDescriptor {
            version: CONTROL_VERSION,
            instance_id,
            pid: std::process::id(),
            port: address.port(),
            host: host.to_owned(),
        };
        mcp_control::publish_descriptor(&control.state.data_dir, &descriptor)?;
        let router = Router::new()
            .route("/v1/targets", get(list_targets))
            .route("/v1/mcp/start", post(start_endpoint))
            .route("/v1/mcp/stop", post(stop_endpoint))
            .route("/v1/mcp/status", post(endpoint_status))
            .fallback(|| async {
                failure(StatusCode::NOT_FOUND, "not_found", "Unknown control route")
            })
            .with_state(control.clone())
            .layer(middleware::from_fn_with_state(
                control.clone(),
                validate_request,
            ));
        let (shutdown, mut stopped) = watch::channel(false);
        let serving = control.clone();
        tokio::spawn(async move {
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    if !*stopped.borrow() {
                        let _ = stopped.changed().await;
                    }
                })
                .await;
            if let Err(error) = result {
                log::warn!("MCP control server stopped: {error}");
            }
            serving.cleanup();
        });
        Ok(Self { control, shutdown })
    }

    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
        self.control.cleanup();
    }
}

impl Drop for ControlService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Control {
    /// Register actual host windows/connections, including those without terminals.
    pub fn register(&self, owner: SessionOwner, label: String, sink: Sink) -> AppResult<()> {
        let id = super::scope_id(&owner);
        let mut targets = locked(&self.targets)?;
        if self.stopped.load(Ordering::Acquire) {
            return Err(AppError::other("mcp_control_stopped", json!({})));
        }
        targets.entry(id).or_insert_with(|| {
            Arc::new(Target {
                owner,
                label,
                sink,
                closed: watch::channel(false).0,
            })
        });
        Ok(())
    }

    pub fn unregister(&self, owner: &SessionOwner) {
        match self.targets.lock() {
            Ok(mut targets) => {
                if let Some(target) = targets.remove(&super::scope_id(owner)) {
                    target.closed.send_replace(true);
                }
                // Revoke before a new window can register the same owner label.
                if let Err(error) = super::stop(&self.state, owner) {
                    log::warn!("MCP endpoint cleanup failed: {error}");
                }
            }
            Err(error) => log::warn!("MCP control target cleanup failed: {error}"),
        }
    }

    fn cleanup(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        match self.targets.lock() {
            Ok(mut targets) => {
                for target in targets.drain().map(|(_, target)| target) {
                    target.closed.send_replace(true);
                    if let Err(error) = super::stop(&self.state, &target.owner) {
                        log::warn!("MCP endpoint shutdown failed: {error}");
                    }
                }
            }
            Err(error) => log::warn!("MCP target shutdown failed: {error}"),
        }
        if let Err(error) = mcp_control::remove_descriptor(&self.state.data_dir, &self.instance_id)
        {
            log::warn!("MCP control discovery cleanup failed: {error}");
        }
    }

    fn target(&self, owner_id: &str) -> AppResult<Arc<Target>> {
        locked(&self.targets)?
            .get(owner_id)
            .cloned()
            .ok_or_else(|| AppError::not_found("mcp_owner_unavailable", json!({})))
    }

    fn current(&self, target: &Arc<Target>) -> AppResult<bool> {
        Ok(locked(&self.targets)?
            .get(&super::scope_id(&target.owner))
            .is_some_and(|current| Arc::ptr_eq(current, target))
            && !*target.closed.borrow())
    }

    async fn start(&self, target: Arc<Target>) -> AppResult<rssh_core::agent::EndpointInfo> {
        let mut closed = target.closed.subscribe();
        if *closed.borrow() || !self.current(&target)? {
            return Err(AppError::not_found("mcp_owner_unavailable", json!({})));
        }
        let result = tokio::select! {
            biased;
            _ = closed.changed() => {
                return Err(AppError::not_found("mcp_owner_unavailable", json!({})));
            }
            result = super::start(self.state.clone(), target.owner.clone()) => result?,
        };
        let endpoint = result
            .endpoint
            .ok_or_else(|| AppError::other("agent_endpoint_stopped", json!({})))?;
        if !self.current(&target)? {
            self.state
                .agent_bridge
                .stop_endpoint_if_token(&target.owner, &endpoint.token)?;
            return Err(AppError::not_found("mcp_owner_unavailable", json!({})));
        }
        // Existing native/WS event adapters deliver this only to the owner.
        if !(target.sink)(
            &format!("mcp:changed:{}", super::scope_id(&target.owner)),
            json!({}),
        ) {
            self.state
                .agent_bridge
                .stop_endpoint_if_token(&target.owner, &endpoint.token)?;
            return Err(AppError::not_found("mcp_owner_unavailable", json!({})));
        }
        Ok(endpoint)
    }

    async fn stop(&self, target: Arc<Target>) -> AppResult<()> {
        // A start already waiting on listener creation must settle before stop
        // revokes it. Keep the owner-generation check and revocation atomic.
        let _starting = self.state.agent_bridge.mcp_start_lock.lock().await;
        {
            let targets = locked(&self.targets)?;
            if !targets
                .get(&super::scope_id(&target.owner))
                .is_some_and(|current| Arc::ptr_eq(current, &target))
                || *target.closed.borrow()
            {
                return Err(AppError::not_found("mcp_owner_unavailable", json!({})));
            }
            // Reuse bridge revocation. Actors persist their final history and
            // emit session_ended; the frontend retains the final timeline drain.
            super::stop(&self.state, &target.owner)?;
        }
        if !(target.sink)(
            &format!("mcp:changed:{}", super::scope_id(&target.owner)),
            json!({}),
        ) {
            return Err(AppError::not_found("mcp_owner_unavailable", json!({})));
        }
        Ok(())
    }

    fn connection(&self, owner_id: &str, endpoint: rssh_core::agent::EndpointInfo) -> Value {
        json!({
            "url": endpoint.url,
            "headers": { "Authorization": format!("Bearer {}", endpoint.token) },
            "transport": "streamable-http",
            "instance_id": self.instance_id,
            "owner_id": owner_id,
        })
    }
}

async fn validate_request(
    State(control): State<Arc<Control>>,
    request: Request,
    next: Next,
) -> Response {
    if control.stopped.load(Ordering::Acquire) {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "control_stopped",
            "Control server is shutting down",
        );
    }
    let host_valid = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        == Some(control.authority.as_str());
    if !host_valid || request.headers().contains_key(header::ORIGIN) {
        return failure(
            StatusCode::FORBIDDEN,
            "invalid_origin",
            "Invalid control request origin",
        );
    }
    next.run(request).await
}

async fn list_targets(State(control): State<Arc<Control>>) -> Response {
    match locked(&control.targets) {
        Ok(targets) => {
            let mut targets: Vec<_> = targets
                .iter()
                .map(|(id, target)| json!({"owner_id": id, "label": target.label}))
                .collect();
            targets.sort_by(|a, b| a["owner_id"].as_str().cmp(&b["owner_id"].as_str()));
            response(
                StatusCode::OK,
                json!({"instance_id": control.instance_id, "targets": targets}),
            )
        }
        Err(error) => app_failure(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerInput {
    owner_id: String,
}

async fn input(request: Request) -> Result<OwnerInput, Response> {
    let json_content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
    if !json_content_type {
        return Err(failure(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "invalid_request",
            "Expected Content-Type application/json",
        ));
    }

    let body = axum::body::to_bytes(request.into_body(), 8 * 1024)
        .await
        .map_err(|_| {
            failure(
                StatusCode::PAYLOAD_TOO_LARGE,
                "invalid_request",
                "Control request exceeds the size limit",
            )
        })?;
    serde_json::from_slice(&body).map_err(|_| {
        failure(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Expected a JSON object containing owner_id",
        )
    })
}

async fn start_endpoint(State(control): State<Arc<Control>>, request: Request) -> Response {
    let input = match input(request).await {
        Ok(input) => input,
        Err(error) => return error,
    };
    let target = match control.target(&input.owner_id) {
        Ok(target) => target,
        Err(error) => return app_failure(error),
    };
    match control.start(target).await {
        Ok(endpoint) => response(
            StatusCode::OK,
            control.connection(&input.owner_id, endpoint),
        ),
        Err(error) => app_failure(error),
    }
}

async fn endpoint_status(State(control): State<Arc<Control>>, request: Request) -> Response {
    let input = match input(request).await {
        Ok(input) => input,
        Err(error) => return error,
    };
    let target = match control.target(&input.owner_id) {
        Ok(target) => target,
        Err(error) => return app_failure(error),
    };
    match control.state.agent_bridge.endpoint(&target.owner) {
        Ok(endpoint) => response(
            StatusCode::OK,
            json!({
                "running": endpoint.is_some(),
                "connection": endpoint.map(|endpoint| control.connection(&input.owner_id, endpoint)),
                "instance_id": control.instance_id,
                "owner_id": input.owner_id,
            }),
        ),
        Err(error) => app_failure(error),
    }
}

async fn stop_endpoint(State(control): State<Arc<Control>>, request: Request) -> Response {
    let input = match input(request).await {
        Ok(input) => input,
        Err(error) => return error,
    };
    let target = match control.target(&input.owner_id) {
        Ok(target) => target,
        Err(error) => return app_failure(error),
    };
    match control.stop(target).await {
        Ok(()) => response(
            StatusCode::OK,
            json!({
                "running": false,
                "connection": null,
                "instance_id": control.instance_id,
                "owner_id": input.owner_id,
            }),
        ),
        Err(error) => app_failure(error),
    }
}

fn response(status: StatusCode, value: Value) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        value.to_string(),
    )
        .into_response()
}

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    response(status, json!({"error": {"code": code, "message": message}}))
}

fn app_failure(error: AppError) -> Response {
    let status = if error.code() == "mcp_owner_unavailable" {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    let message = if error.code() == "mcp_owner_unavailable" {
        "The selected RSSH window or connection is no longer available".to_owned()
    } else {
        log::warn!("MCP control request failed: {error}");
        format!("MCP control request failed ({})", error.code())
    };
    failure(status, error.code(), &message)
}

#[cfg(test)]
mod tests;
