//! Local IPC control for CLI clients of an existing MCP host.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rssh_core::emitter::Sink;
use rssh_core::error::{locked, AppError, AppResult};
use rssh_core::mcp_control::{
    self, native_transport, BoxStream, ControlDescriptor, ControlRequest, CONTROL_VERSION,
    REQUEST_LIMIT, RESPONSE_LIMIT,
};
use rssh_core::state::{AppState, SessionOwner};
use serde_json::{json, Value};
use tokio::sync::watch;
use tokio::task::JoinSet;

struct Target {
    owner: SessionOwner,
    label: String,
    sink: Sink,
    closed: watch::Sender<bool>,
}

pub struct Control {
    state: Arc<AppState>,
    instance_id: String,
    stopped: AtomicBool,
    targets: Mutex<HashMap<String, Arc<Target>>>,
}

/// The host owns this guard, not the listener task, so closing the host stops IPC.
pub struct ControlService {
    pub control: Arc<Control>,
    shutdown: watch::Sender<bool>,
}

impl ControlService {
    pub async fn start(state: Arc<AppState>, host: &str) -> AppResult<Self> {
        let instance_id = uuid::Uuid::new_v4().to_string();
        let endpoint = native_transport().endpoint(&instance_id)?;
        let mut listener = native_transport().bind(&endpoint).await?;
        let control = Arc::new(Control {
            state,
            instance_id: instance_id.clone(),
            stopped: AtomicBool::new(false),
            targets: Mutex::new(HashMap::new()),
        });
        let descriptor = ControlDescriptor {
            version: CONTROL_VERSION,
            instance_id,
            pid: std::process::id(),
            endpoint,
            host: host.to_owned(),
        };
        mcp_control::publish_descriptor(&control.state.data_dir, &descriptor)?;
        let (shutdown, mut stopped) = watch::channel(false);
        let serving = control.clone();
        tokio::spawn(async move {
            let mut requests = JoinSet::new();
            let mut accept_failures = 0;
            loop {
                if *stopped.borrow() {
                    break;
                }
                tokio::select! {
                    biased;
                    _ = stopped.changed() => break,
                    Some(result) = requests.join_next(), if !requests.is_empty() => {
                        if let Err(error) = result {
                            log::warn!("MCP control request task failed: {error}");
                        }
                    }
                    accepted = listener.accept(), if requests.len() < 64 => {
                        match accepted {
                            Ok(stream) => {
                                accept_failures = 0;
                                let control = serving.clone();
                                requests.spawn(async move {
                                    match tokio::time::timeout(
                                        Duration::from_secs(5),
                                        serve_connection(control, stream),
                                    ).await {
                                        Ok(Ok(())) => {}
                                        Ok(Err(error)) => log::debug!("MCP control connection ended: {error}"),
                                        Err(_) => log::debug!("MCP control connection timed out"),
                                    }
                                });
                            }
                            Err(error) => {
                                log::warn!("MCP control accept failed: {error}");
                                accept_failures += 1;
                                // Transient disconnects or resource pressure can
                                // recover; a broken listener must not stay published.
                                if matches!(error.kind(),
                                    std::io::ErrorKind::PermissionDenied |
                                    std::io::ErrorKind::InvalidInput |
                                    std::io::ErrorKind::Unsupported |
                                    std::io::ErrorKind::NotFound
                                ) || accept_failures >= 100 {
                                    break;
                                }
                                tokio::select! {
                                    _ = stopped.changed() => break,
                                    _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                                }
                            }
                        }
                    }
                }
            }
            // Drop the listening endpoint and all pending reads before the host
            // state can disappear. Domain shutdown revokes any committed MCP.
            drop(listener);
            requests.abort_all();
            while requests.join_next().await.is_some() {}
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

async fn serve_connection(control: Arc<Control>, mut stream: BoxStream) -> std::io::Result<()> {
    let response = match mcp_control::read_json::<ControlRequest>(&mut stream, REQUEST_LIMIT).await
    {
        Ok(request) => dispatch(&control, request)
            .await
            .unwrap_or_else(app_failure),
        Err(_) => failure(
            "invalid_request",
            "Expected a bounded JSON MCP control request",
        ),
    };
    mcp_control::write_json(&mut stream, &response, RESPONSE_LIMIT).await
}

async fn dispatch(control: &Control, request: ControlRequest) -> AppResult<Value> {
    if control.stopped.load(Ordering::Acquire) {
        return Err(AppError::other("mcp_control_stopped", json!({})));
    }
    match request {
        ControlRequest::Targets => {
            let targets = locked(&control.targets)?;
            let mut targets: Vec<_> = targets
                .iter()
                .map(|(id, target)| json!({"owner_id":id, "label":target.label}))
                .collect();
            targets.sort_by(|a, b| a["owner_id"].as_str().cmp(&b["owner_id"].as_str()));
            Ok(json!({"instance_id":control.instance_id, "targets":targets}))
        }
        ControlRequest::Start { owner_id } => {
            let target = control.target(&owner_id)?;
            let endpoint = control.start(target).await?;
            Ok(control.connection(&owner_id, endpoint))
        }
        ControlRequest::Status { owner_id } => {
            let target = control.target(&owner_id)?;
            let endpoint = control.state.agent_bridge.endpoint(&target.owner)?;
            Ok(json!({
                "running":endpoint.is_some(),
                "connection":endpoint.map(|endpoint| control.connection(&owner_id, endpoint)),
                "instance_id":control.instance_id,
                "owner_id":owner_id,
            }))
        }
        ControlRequest::Stop { owner_id } => {
            let target = control.target(&owner_id)?;
            control.stop(target).await?;
            Ok(json!({
                "running":false, "connection":null,
                "instance_id":control.instance_id, "owner_id":owner_id,
            }))
        }
    }
}

fn failure(code: &str, message: &str) -> Value {
    json!({"error":{"code":code,"message":message}})
}

fn app_failure(error: AppError) -> Value {
    let message = if error.code() == "mcp_owner_unavailable" {
        "The selected RSSH window or connection is no longer available".to_owned()
    } else {
        log::warn!("MCP control request failed: {error}");
        format!("MCP control request failed ({})", error.code())
    };
    failure(error.code(), &message)
}

#[cfg(test)]
mod tests;
