//! Local MCP adapter over connected terminals and shared AI conversations.
//!
//! Only the advertised tools cross this boundary. The native
//! and browser frontends retain ownership of approval and terminal execution.

use std::sync::Arc;

use rssh_core::agent::{self, AgentSession, EndpointInfo};
#[cfg(any(android, ios, ohos))]
use rssh_core::error::AppError;
use rssh_core::error::AppResult;
use rssh_core::state::{AppState, SessionOwner};

#[derive(Clone, serde::Serialize)]
pub struct Status {
    pub endpoint: Option<EndpointInfo>,
    pub sessions: Vec<AgentSession>,
    pub requests: Vec<agent::SessionRequest>,
}

pub fn status(state: &AppState, owner: &SessionOwner) -> AppResult<Status> {
    Ok(Status {
        endpoint: state.agent_bridge.endpoint(owner)?,
        sessions: agent::list_sessions(state, owner)?,
        requests: agent::pending_sessions(state, owner)?,
    })
}

pub async fn start(state: Arc<AppState>, owner: SessionOwner) -> AppResult<Status> {
    #[cfg(any(windows, macos, linux))]
    {
        http::start(state.clone(), owner.clone()).await?;
        status(&state, &owner)
    }
    #[cfg(any(android, ios, ohos))]
    {
        let _ = (state, owner);
        Err(AppError::other("mcp_unavailable", serde_json::json!({})))
    }
}

pub fn stop(state: &AppState, owner: &SessionOwner) -> AppResult<()> {
    state.agent_bridge.stop_endpoint(owner)
}

#[cfg(any(windows, macos, linux))]
mod http {
    use super::*;
    use axum::extract::{Request, State};
    use axum::http::{header, StatusCode};
    use axum::middleware::{self, Next};
    use axum::response::{IntoResponse, Response};
    use axum::Router;
    use rmcp::model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, Implementation,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
        ToolAnnotations,
    };
    use rmcp::service::RequestContext;
    use rmcp::transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    };
    use rmcp::{RoleServer, ServerHandler};
    use serde_json::{json, Value};

    #[derive(Clone)]
    struct TerminalTools {
        state: Arc<AppState>,
        owner: SessionOwner,
        endpoint_token: Arc<str>,
    }

    fn session_tool_schema(name: &str) -> serde_json::Map<String, Value> {
        // Keep arguments aligned with the built-in AI tool; MCP only adds
        // the accepted terminal's identity.
        let mut schema = rssh_core::ai::tools::all_tools()
            .into_iter()
            .find(|tool| tool.name == name)
            .expect("built-in AI tool schema")
            .input_schema;
        schema["properties"]["session_id"] = json!({
            "type":"string", "description":"Exact session_id returned by list_sessions."
        });
        schema["required"]
            .as_array_mut()
            .unwrap()
            .push(json!("session_id"));
        schema["additionalProperties"] = json!(false);
        schema.as_object().unwrap().clone()
    }

    fn tool_definitions() -> Vec<Tool> {
        let list = Tool::new(
            "list_sessions",
            "List every connected terminal in this RSSH window. Call open_session to request control before run_command or download_file. An empty tab_id means the terminal frontend is still registering; retry discovery before opening it. Shell metadata reflects RSSH's existing detection/configuration and can be stale after a nested shell. Use the reported shell syntax and inspect any further environment details with run_command.",
            json!({"type":"object", "properties":{}, "additionalProperties":false})
                .as_object().unwrap().clone(),
        ).annotate(ToolAnnotations::new().read_only(true));
        let open = Tool::new(
            "open_session",
            "Ask the user to let this local agent control an existing terminal through an RSSH AI conversation. Waits for acceptance and returns the conversation identity. Reuses an already accepted conversation for this terminal. No model provider is required.",
            json!({"type":"object", "properties":{
                "session_id":{"type":"string", "description":"Exact session_id returned by list_sessions."},
                "title":{"type":"string", "minLength":1, "maxLength":512, "description":"Short title describing the debugging task."}
            }, "required":["session_id","title"], "additionalProperties":false}).as_object().unwrap().clone(),
        );
        let run = Tool::new(
            "run_command",
            "Run one command in a terminal accepted through open_session. Uses the RSSH AI conversation's command validation, approval cards, danger-mode settings, audit and output redaction. Returns output from the existing interactive shell. Match list_sessions shell syntax; RSSH adds its own completion marker. Do not add markers or change shells. A timeout ends output collection and does not guarantee the command stopped. The terminal frontend must remain open. Do not automatically repeat a command after a timeout or lost response.",
            session_tool_schema("run_command"),
        );
        let download = Tool::new(
            "download_file",
            "Download a remote file using the existing SSH connection's SFTP subsystem after open_session is accepted. Reuses RSSH AI download approval, danger-mode auto_download_file settings, audit and history. max_mb must be 1..100; inspect the remote file size first and transfer larger files manually. Local shell, serial and Telnet targets cannot provide SFTP. Returns status, bytes and a local_path on the RSSH host under its data directory; it does not return file contents. In Headless/JetBrains the RSSH host may differ from the agent's machine. The terminal frontend must remain open.",
            session_tool_schema("download_file"),
        );
        vec![list, open, run, download]
    }

    impl ServerHandler for TerminalTools {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
                .with_server_info(Implementation::new("rssh", env!("CARGO_PKG_VERSION")))
                .with_instructions("Use list_sessions to select a connected terminal, open_session(session_id,title) to request a conversation, then run_command to diagnose it or download_file to fetch an SSH file onto the RSSH host. Local repository access remains the coding agent's responsibility. Tools follow the conversation's AI approval settings; download_file returns a host-local path, not file contents.")
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            Ok(ListToolsResult::with_all_items(tool_definitions()))
        }

        fn get_tool(&self, name: &str) -> Option<Tool> {
            tool_definitions()
                .into_iter()
                .find(|tool| tool.name == name)
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            let result = match request.name.as_ref() {
                "list_sessions" => {
                    agent::list_mcp_sessions(&self.state, &self.owner, &self.endpoint_token)
                        .map(|sessions| json!({"sessions":sessions}))
                }
                "open_session" => {
                    let input = serde_json::from_value::<agent::OpenSessionInput>(Value::Object(
                        request.arguments.unwrap_or_default(),
                    ))
                    .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?;
                    tokio::select! {
                        _ = context.ct.cancelled() => {
                            return Ok(CallToolResult::structured_error(json!({"code":"request_cancelled"})).into());
                        }
                        result = agent::open_mcp_session(&self.state, &self.owner, &self.endpoint_token, input) => {
                            result.map(|result| serde_json::to_value(result).expect("session result serializes"))
                        }
                    }
                }
                "run_command" => {
                    let input = serde_json::from_value::<agent::RunCommandInput>(Value::Object(
                        request.arguments.unwrap_or_default(),
                    ))
                    .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?;
                    tokio::select! {
                        _ = context.ct.cancelled() => {
                            return Ok(CallToolResult::structured_error(json!({"code":"request_cancelled"})).into());
                        }
                        result = agent::run_mcp_command(&self.state, &self.owner, &self.endpoint_token, input) => {
                            result.map(|result| serde_json::to_value(result).expect("command result serializes"))
                        }
                    }
                }
                "download_file" => {
                    let input = serde_json::from_value::<agent::DownloadFileInput>(Value::Object(
                        request.arguments.unwrap_or_default(),
                    ))
                    .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?;
                    tokio::select! {
                        _ = context.ct.cancelled() => {
                            return Ok(CallToolResult::structured_error(json!({"code":"request_cancelled"})).into());
                        }
                        result = agent::download_mcp_file(&self.state, &self.owner, &self.endpoint_token, input) => {
                            result.map(|result| serde_json::to_value(result).expect("download result serializes"))
                        }
                    }
                }
                _ => return Err(ErrorData::invalid_params("Unknown tool", None)),
            };
            Ok(match result {
                Ok(result) => {
                    let failed = result.get("timed_out").and_then(Value::as_bool) == Some(true)
                        || result.get("ok").and_then(Value::as_bool) == Some(false)
                        || result.get("early_terminated").and_then(Value::as_bool) == Some(true)
                        || result.get("rejected").is_some_and(|value| !value.is_null())
                        || result
                            .get("exit_code")
                            .and_then(Value::as_i64)
                            .is_some_and(|code| code != 0);
                    if failed {
                        CallToolResult::structured_error(result)
                    } else {
                        CallToolResult::structured(result)
                    }
                }
                Err(error) => {
                    CallToolResult::structured_error(json!({"code":error.code(), "error":error}))
                }
            }
            .into())
        }
    }

    #[derive(Clone)]
    struct Authentication {
        state: Arc<AppState>,
        owner: SessionOwner,
        token: Arc<str>,
    }

    async fn authenticate(
        State(auth): State<Authentication>,
        request: Request,
        next: Next,
    ) -> Result<Response, Response> {
        let active = auth
            .state
            .agent_bridge
            .endpoint(&auth.owner)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())?
            .is_some_and(|endpoint| endpoint.token == auth.token.as_ref());
        let authorized = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|value| value == auth.token.as_ref());
        if !active || !authorized {
            return Err((
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Bearer realm=\"RSSH\"")],
            )
                .into_response());
        }
        Ok(next.run(request).await)
    }

    pub(super) fn session_manager() -> LocalSessionManager {
        let mut manager = LocalSessionManager::default();
        // rmcp counts a waiting handler as idle. Leave room for user approval
        // and the AI command runner's maximum 300s execution window.
        manager.session_config.keep_alive = Some(std::time::Duration::from_secs(15 * 60));
        manager
    }

    pub(super) async fn start(state: Arc<AppState>, owner: SessionOwner) -> AppResult<()> {
        if state.agent_bridge.endpoint(&owner)?.is_some() {
            return Ok(());
        }
        let reservation = state.agent_bridge.reserve_endpoint(&owner)?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let authority = listener.local_addr()?.to_string();
        let origin = format!("http://{authority}");
        let token = uuid::Uuid::new_v4().simple().to_string();
        let config = StreamableHttpServerConfig::default()
            .with_allowed_hosts([authority])
            .with_allowed_origins([origin.clone()])
            .enforce_origin_validation();
        let cancellation = config.cancellation_token.clone();
        let tools = TerminalTools {
            state: state.clone(),
            owner: owner.clone(),
            endpoint_token: Arc::from(token.as_str()),
        };
        let service = StreamableHttpService::new(
            move || Ok(tools.clone()),
            Arc::new(session_manager()),
            config,
        );
        let router =
            Router::new()
                .nest_service("/mcp", service)
                .layer(middleware::from_fn_with_state(
                    Authentication {
                        state: state.clone(),
                        owner: owner.clone(),
                        token: Arc::from(token.as_str()),
                    },
                    authenticate,
                ));
        let (shutdown, mut stopped) = tokio::sync::watch::channel(false);
        let endpoint_token = token.clone();
        reservation.commit(
            EndpointInfo {
                url: format!("{origin}/mcp"),
                token,
            },
            shutdown,
        )?;
        tokio::spawn(async move {
            let server = axum::serve(listener, router).with_graceful_shutdown(async move {
                if !*stopped.borrow() {
                    let _ = stopped.changed().await;
                }
                cancellation.cancel();
            });
            if let Err(error) = server.await {
                log::warn!("MCP server stopped: {error}");
                if let Err(error) = state
                    .agent_bridge
                    .stop_endpoint_if_token(&owner, &endpoint_token)
                {
                    log::warn!("MCP cleanup failed: {error}");
                }
            }
        });
        Ok(())
    }
}

#[cfg(all(test, any(windows, macos, linux)))]
mod tests;
