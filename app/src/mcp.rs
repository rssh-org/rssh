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

#[cfg(any(windows, macos, linux))]
pub mod control;

pub fn scope_id(owner: &SessionOwner) -> String {
    match owner {
        SessionOwner::Window(label) => format!("window-{label}"),
        SessionOwner::Headless(id) => format!("headless-{id}"),
    }
}

#[derive(Clone, serde::Serialize)]
pub struct Status {
    pub scope_id: String,
    pub endpoint: Option<EndpointInfo>,
    pub sessions: Vec<AgentSession>,
    pub requests: Vec<agent::SessionRequest>,
}

pub fn status(state: &AppState, owner: &SessionOwner) -> AppResult<Status> {
    Ok(Status {
        scope_id: scope_id(owner),
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
    use rssh_core::ai::{llm::ToolSchema, session::ExternalToolInput, tools};
    use serde::de::DeserializeOwned;
    use serde::Deserialize;
    use serde_json::{json, Value};

    const LOCAL_ANALYSIS_MESSAGE: &str =
        "Analyze the local file yourself using your own tools. RSSH does not run local analysis for an MCP agent.";

    #[derive(Clone)]
    struct TerminalTools {
        state: Arc<AppState>,
        owner: SessionOwner,
        endpoint_token: Arc<str>,
    }

    fn session_tool(canonical: ToolSchema) -> Tool {
        let mut schema = canonical.input_schema;
        schema["properties"]["session_id"] = json!({
            "type":"string", "description":"Exact session_id returned by list_sessions and accepted through open_session."
        });
        schema["required"]
            .as_array_mut()
            .unwrap()
            .push(json!("session_id"));
        schema["additionalProperties"] = json!(false);
        Tool::new(
            canonical.name,
            canonical.description,
            schema.as_object().unwrap().clone(),
        )
    }

    fn tool_definitions() -> Vec<Tool> {
        let list = Tool::new(
            "list_sessions",
            "List connected terminals in this RSSH window. Select the returned session_id and call open_session to request access. An empty tab_id means the terminal frontend is still registering; retry discovery before opening it. Shell metadata reflects the active conversation's manual selection, or defaults to POSIX without an active conversation. Serial and Telnet retain their transport kind. The selection does not change the terminal's actual shell.",
            json!({"type":"object", "properties":{}, "additionalProperties":false})
                .as_object().unwrap().clone(),
        ).annotate(ToolAnnotations::new().read_only(true));
        let open = Tool::new(
            "open_session",
            "Ask the user to let this local agent control an existing terminal through an RSSH AI conversation. Waits for acceptance and returns the conversation identity and available_skills catalog. Pass a catalog id to load_skill to read its content. Reuses an already accepted conversation for this terminal. No model provider is required.",
            json!({"type":"object", "properties":{
                "session_id":{"type":"string", "description":"Exact session_id returned by list_sessions."},
                "title":{"type":"string", "minLength":1, "maxLength":512, "description":"Short title describing the debugging task."}
            }, "required":["session_id","title"], "additionalProperties":false}).as_object().unwrap().clone(),
        );
        let mut definitions = vec![list, open];
        for canonical in tools::all_tools() {
            match canonical.name.as_str() {
                tools::TOOL_RUN_COMMAND
                | tools::TOOL_DOWNLOAD_FILE
                | tools::TOOL_LOAD_SKILL
                | tools::TOOL_MATCH_FILE
                | tools::TOOL_PATCH_FILE => {
                    definitions.push(session_tool(canonical));
                }
                tools::TOOL_ANALYZE_LOCALLY => {
                    let mut schema = canonical.input_schema;
                    schema["additionalProperties"] = json!(false);
                    definitions.push(Tool::new(
                        canonical.name,
                        LOCAL_ANALYSIS_MESSAGE,
                        schema.as_object().unwrap().clone(),
                    ));
                }
                _ => {}
            }
        }
        definitions
    }

    fn parse_args<T: DeserializeOwned>(args: Value) -> Result<T, ErrorData> {
        serde_json::from_value(args)
            .map_err(|error| ErrorData::invalid_params(error.to_string(), None))
    }

    #[derive(Deserialize)]
    struct SessionArgs<T> {
        session_id: String,
        #[serde(flatten)]
        input: T,
        #[serde(flatten)]
        extra: serde_json::Map<String, Value>,
    }

    impl<T: DeserializeOwned> SessionArgs<T> {
        fn parse(args: Value) -> Result<Self, ErrorData> {
            let parsed: Self = parse_args(args)?;
            if let Some(field) = parsed.extra.keys().next() {
                return Err(ErrorData::invalid_params(
                    format!("Unknown argument: {field}"),
                    None,
                ));
            }
            Ok(parsed)
        }
    }

    impl TerminalTools {
        async fn execute(
            &self,
            request: CallToolRequestParams,
        ) -> Result<CallToolResult, ErrorData> {
            let args = Value::Object(request.arguments.unwrap_or_default());
            let result = match request.name.as_ref() {
                "list_sessions" => {
                    agent::list_mcp_sessions(&self.state, &self.owner, &self.endpoint_token)
                        .map(|sessions| json!({"sessions":sessions}))
                }
                "open_session" => agent::open_mcp_session(
                    &self.state,
                    &self.owner,
                    &self.endpoint_token,
                    parse_args(args)?,
                )
                .await
                .map(|result| json!(result)),
                tools::TOOL_RUN_COMMAND => agent::run_mcp_command(
                    &self.state,
                    &self.owner,
                    &self.endpoint_token,
                    parse_args(args)?,
                )
                .await
                .map(|result| json!(result)),
                tools::TOOL_DOWNLOAD_FILE => agent::download_mcp_file(
                    &self.state,
                    &self.owner,
                    &self.endpoint_token,
                    parse_args(args)?,
                )
                .await
                .map(|result| json!(result)),
                tools::TOOL_LOAD_SKILL => {
                    self.call_actor_tool(SessionArgs::parse(args)?, ExternalToolInput::LoadSkill)
                        .await
                }
                tools::TOOL_MATCH_FILE => {
                    self.call_actor_tool(SessionArgs::parse(args)?, ExternalToolInput::MatchFile)
                        .await
                }
                tools::TOOL_PATCH_FILE => {
                    self.call_actor_tool(SessionArgs::parse(args)?, ExternalToolInput::PatchFile)
                        .await
                }
                tools::TOOL_ANALYZE_LOCALLY => Ok(json!({
                    "ok":false, "code":"mcp_analyze_locally_in_agent", "message":LOCAL_ANALYSIS_MESSAGE
                })),
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
            })
        }

        async fn call_actor_tool<T>(
            &self,
            args: SessionArgs<T>,
            tool: fn(T) -> ExternalToolInput,
        ) -> AppResult<Value> {
            let result = agent::call_mcp_tool(
                &self.state,
                &self.owner,
                &self.endpoint_token,
                &args.session_id,
                tool(args.input),
            )
            .await;
            result.map(|result| {
                let mut value = json!(result);
                value["session_id"] = json!(args.session_id);
                value
            })
        }
    }

    impl ServerHandler for TerminalTools {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
                .with_server_info(Implementation::new("rssh", env!("CARGO_PKG_VERSION")))
                .with_instructions("RSSH exposes the user's connected terminal sessions.")
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
            tokio::select! {
                _ = context.ct.cancelled() => Ok(CallToolResult::structured_error(json!({"code":"request_cancelled"})).into()),
                result = self.execute(request) => result.map(Into::into),
            }
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
        // Reserve before waiting so host teardown can revoke queued GUI starts
        // too. Concurrent starts join this generation and reuse its endpoint.
        let reservation = state.agent_bridge.reserve_endpoint(&owner)?;
        let _starting = state.agent_bridge.mcp_start_lock.lock().await;
        if reservation.endpoint()?.is_some() {
            return Ok(());
        }
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
        let state = state.clone();
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
