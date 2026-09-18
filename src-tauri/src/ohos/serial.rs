//! Public HarmonyOS serial service; no access to private device nodes or DDK.
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use napi_derive_ohos::napi;
use napi_ohos::bindgen_prelude::Unknown;
use openharmony_ability::{
    impl_bridge_napi_type, AsyncBridge, BridgeCallOptions, BridgeContextRequirement,
    BridgeMainThreadEvent, BridgeNapiType, BridgePlugin, BridgeRuntime, PluginLifecycleEvent,
};

use crate::error::{locked, AppError, AppResult};
use crate::terminal::serial::{SerialCapabilities, SerialConfig, SerialOut, SerialSink};

#[derive(Clone)]
struct OutputTarget {
    session_id: String,
    sink: SerialSink,
}

// Keys identify one native open attempt, not a reusable frontend session ID.
// The lifecycle registry remains the only owner of live SerialHandles.
static OUTPUTS: LazyLock<Mutex<HashMap<String, OutputTarget>>> = LazyLock::new(Mutex::default);

pub struct SerialPlugin;
impl BridgePlugin for SerialPlugin {
    type Mode = AsyncBridge;
    const ID: &'static str = "rssh.serial";
    const REQUIRED_CONTEXTS: &'static [BridgeContextRequirement] =
        &[BridgeContextRequirement::Ability];

    fn on_main_thread_event<'env>(
        &self,
        event: BridgeMainThreadEvent<'env>,
    ) -> napi_ohos::Result<Unknown<'env>> {
        if event.name() != "serial-output" {
            return Err(napi_ohos::Error::from_reason("Unknown serial event"));
        }
        let output = event.decode::<OutputEvent>()?;
        let target = {
            let mut outputs = OUTPUTS
                .lock()
                .map_err(|_| napi_ohos::Error::from_reason("Serial output lock poisoned"))?;
            if output.closed {
                outputs.remove(&output.id)
            } else {
                outputs.get(&output.id).cloned()
            }
        };
        if let Some(target) = target {
            let value = if output.closed {
                SerialOut::Close
            } else {
                SerialOut::Data(output.data)
            };
            (target.sink)(&target.session_id, value);
        }
        event.respond(EmptyResponse {})
    }

    fn on_lifecycle(&self, event: &PluginLifecycleEvent) -> napi_ohos::Result<()> {
        if matches!(
            event,
            PluginLifecycleEvent::AbilityDestroyed | PluginLifecycleEvent::AbilityCreated { .. }
        ) {
            let targets = {
                let mut outputs = OUTPUTS
                    .lock()
                    .map_err(|_| napi_ohos::Error::from_reason("Serial output lock poisoned"))?;
                outputs
                    .drain()
                    .map(|(_, target)| target)
                    .collect::<Vec<_>>()
            };
            for target in targets {
                (target.sink)(&target.session_id, SerialOut::Close);
            }
        }
        Ok(())
    }
}

#[napi(object)]
pub struct EmptyRequest {}
impl_bridge_napi_type!(EmptyRequest, "rssh.serial.EmptyRequest");
#[napi(object)]
pub struct EmptyResponse {}
impl_bridge_napi_type!(EmptyResponse, "rssh.serial.EmptyResponse");
#[napi(object)]
pub struct PortsResponse {
    pub ports: Vec<String>,
}
impl_bridge_napi_type!(PortsResponse, "rssh.serial.PortsResponse");
#[napi(object)]
pub struct CapabilitiesResponse {
    pub flow_control: bool,
    pub xany: bool,
    pub signals: bool,
    pub baud_rates: Vec<u32>,
}
impl_bridge_napi_type!(CapabilitiesResponse, "rssh.serial.CapabilitiesResponse");
#[napi(object)]
pub struct OpenRequest {
    pub id: String,
    pub port: String,
    pub baud_rate: u32,
    pub data_bits: u8,
    pub parity: String,
    pub stop_bits: u8,
    pub flow_control: String,
    pub xany: bool,
}
impl_bridge_napi_type!(OpenRequest, "rssh.serial.OpenRequest");
#[napi(object)]
pub struct PortRequest {
    pub id: String,
}
impl_bridge_napi_type!(PortRequest, "rssh.serial.PortRequest");
#[napi(object)]
pub struct WriteRequest {
    pub id: String,
    pub data: Vec<u8>,
}
impl_bridge_napi_type!(WriteRequest, "rssh.serial.WriteRequest");
#[napi(object)]
pub struct LevelRequest {
    pub id: String,
    pub level: bool,
}
impl_bridge_napi_type!(LevelRequest, "rssh.serial.LevelRequest");
#[napi(object)]
pub struct OutputEvent {
    pub id: String,
    pub data: Vec<u8>,
    pub closed: bool,
}
impl_bridge_napi_type!(OutputEvent, "rssh.serial.OutputEvent");

fn operation_error(error: impl std::fmt::Display) -> AppError {
    AppError::pty(
        "serial_op_failed",
        serde_json::json!({"err": error.to_string()}),
    )
}

struct SerialSession {
    id: String,
    port: String,
    bridge: BridgeRuntime,
    operations: tokio::sync::Mutex<()>,
    capabilities: SerialCapabilities,
}

impl Drop for SerialSession {
    fn drop(&mut self) {
        let target = OUTPUTS
            .lock()
            .ok()
            .and_then(|mut outputs| outputs.remove(&self.id));
        if let Some(target) = target {
            (target.sink)(&target.session_id, SerialOut::Close);
        }
        let bridge = self.bridge.clone();
        let id = self.id.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = bridge
                .call_async::<SerialPlugin, PortRequest, EmptyResponse>(
                    "close",
                    PortRequest { id },
                    BridgeCallOptions::default(),
                )
                .await
            {
                // Ability teardown owns a second cleanup path in SerialPlugin.onDispose.
                log::warn!("failed to close HarmonyOS serial session: {error}");
            }
        });
    }
}

#[derive(Clone)]
pub struct SerialHandle(Arc<SerialSession>);
impl SerialHandle {
    async fn call<Request: BridgeNapiType>(&self, action: &str, request: Request) -> AppResult<()> {
        let _operation = self.0.operations.lock().await;
        self.0
            .bridge
            .call_async::<SerialPlugin, Request, EmptyResponse>(
                action,
                request,
                BridgeCallOptions::default(),
            )
            .await
            .map_err(operation_error)?;
        Ok(())
    }

    /// Explicit close must finish releasing the native port before the caller
    /// reconnects. Drop remains the idempotent fallback for window teardown.
    pub async fn close(&self) -> AppResult<()> {
        let _operation = self.0.operations.lock().await;
        self.0
            .bridge
            .call_async::<SerialPlugin, PortRequest, EmptyResponse>(
                "close",
                PortRequest {
                    id: self.0.id.clone(),
                },
                // A pending system authorization must resolve before its late
                // native handle can be released. Human decisions have no deadline.
                BridgeCallOptions::default().without_timeout(),
            )
            .await
            .map_err(operation_error)?;
        Ok(())
    }

    pub async fn write(&self, data: &[u8]) -> AppResult<()> {
        self.call(
            "write",
            WriteRequest {
                id: self.0.id.clone(),
                data: data.to_vec(),
            },
        )
        .await
    }
    pub async fn set_dtr(&self, level: bool) -> AppResult<()> {
        self.0.capabilities.require_signals()?;
        self.call(
            "set-dtr",
            LevelRequest {
                id: self.0.id.clone(),
                level,
            },
        )
        .await
    }
    pub async fn set_rts(&self, level: bool) -> AppResult<()> {
        self.0.capabilities.require_signals()?;
        self.call(
            "set-rts",
            LevelRequest {
                id: self.0.id.clone(),
                level,
            },
        )
        .await
    }
    pub async fn send_break(&self) -> AppResult<()> {
        self.0.capabilities.require_signals()?;
        self.call(
            "send-break",
            PortRequest {
                id: self.0.id.clone(),
            },
        )
        .await
    }
    pub fn port_name(&self) -> &str {
        &self.0.port
    }
}

pub async fn capabilities() -> AppResult<SerialCapabilities> {
    let response = super::app()?
        .bridge()
        .map_err(operation_error)?
        .call_async::<SerialPlugin, EmptyRequest, CapabilitiesResponse>(
            "capabilities",
            EmptyRequest {},
            BridgeCallOptions::default(),
        )
        .await
        .map_err(operation_error)?;
    Ok(SerialCapabilities {
        flow_control: response.flow_control,
        xany: response.xany,
        signals: response.signals,
        baud_rates: response.baud_rates,
    })
}

pub async fn available_ports() -> AppResult<Vec<String>> {
    let response = super::app()?
        .bridge()
        .map_err(operation_error)?
        .call_async::<SerialPlugin, EmptyRequest, PortsResponse>(
            "list",
            EmptyRequest {},
            BridgeCallOptions::default(),
        )
        .await
        .map_err(operation_error)?;
    Ok(response.ports)
}

/// An open error and the independent result of releasing any partial native
/// resource. The lifecycle owner must acknowledge cleanup before retrying.
pub struct OpenFailure {
    pub error: AppError,
    pub cleanup: AppResult<()>,
}

impl From<AppError> for OpenFailure {
    fn from(error: AppError) -> Self {
        Self {
            error,
            cleanup: Ok(()),
        }
    }
}

pub async fn open(
    session_id: String,
    port: &str,
    config: SerialConfig,
    sink: SerialSink,
    cancellation: impl std::future::Future<Output = ()>,
) -> Result<(String, SerialHandle), OpenFailure> {
    tokio::pin!(cancellation);
    let cancelled = || {
        AppError::not_found(
            "session_reservation_lost",
            serde_json::json!({ "id": session_id }),
        )
    };
    let capabilities = tokio::select! {
        biased;
        _ = &mut cancellation => return Err(cancelled().into()),
        result = capabilities() => result?,
    };
    capabilities.validate(&config)?;
    let bridge = super::app()?.bridge().map_err(operation_error)?;
    let id = uuid::Uuid::new_v4().to_string();
    locked(&OUTPUTS)?.insert(
        id.clone(),
        OutputTarget {
            session_id: session_id.clone(),
            sink,
        },
    );
    // Create the guard before awaiting authorization: failure or cancellation
    // removes the pending output target and closes a late native open.
    let handle = SerialHandle(Arc::new(SerialSession {
        id: id.clone(),
        port: port.to_owned(),
        bridge: bridge.clone(),
        operations: tokio::sync::Mutex::new(()),
        capabilities,
    }));
    let opened = tokio::select! {
        biased;
        _ = &mut cancellation => Err(cancelled()),
        result = bridge.call_async::<SerialPlugin, OpenRequest, EmptyResponse>(
            "open",
            OpenRequest {
                id,
                port: port.to_owned(),
                baud_rate: config.baud_rate,
                data_bits: config.data_bits,
                parity: config.parity,
                stop_bits: config.stop_bits,
                flow_control: config.flow_control,
                xany: config.xany,
            },
            BridgeCallOptions::default().without_timeout(),
        ) => result.map_err(|error| {
            AppError::pty(
                "serial_open_failed",
                serde_json::json!({"port": port, "err": error.to_string()}),
            )
        }),
    };
    if let Err(error) = opened {
        // If open was dispatched, closing the same attempt cancels its
        // authorization and joins late cleanup; otherwise close is a no-op.
        let cleanup = handle.close().await;
        return Err(OpenFailure { error, cleanup });
    }
    Ok((session_id, handle))
}
