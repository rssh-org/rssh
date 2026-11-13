//! Machine-readable bootstrap for the MCP service owned by a running frontend.

use clap::{Args, Subcommand};
use rssh_core::error::AppError;
use rssh_core::mcp_control::{self, ControlDescriptor, ControlRequest};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const NO_TARGET: &str = "Run an updated RSSH desktop GUI or a local headless/JetBrains frontend, then retry. The GUI/server and CLI must both support MCP control.";

#[derive(Subcommand)]
pub enum McpCmd {
    /// Enable MCP and print its Streamable HTTP connection JSON
    Start(Selection),
    /// Stop the selected frontend's MCP service and print its stopped status
    Stop(Selection),
    /// Print the selected MCP service's running state and connection information
    Status(Selection),
}

#[derive(Args, Default)]
pub struct Selection {
    /// Instance UUID from an ambiguous-target error's candidates
    #[arg(long)]
    instance: Option<uuid::Uuid>,
    /// Frontend owner ID from an ambiguous-target error's candidates
    #[arg(long)]
    owner: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Target {
    owner_id: String,
    label: String,
}

#[derive(Deserialize)]
struct Targets {
    instance_id: String,
    targets: Vec<Target>,
}

struct Instance {
    descriptor: ControlDescriptor,
    targets: Vec<Target>,
}

#[derive(Debug)]
struct Failure {
    code: String,
    message: String,
    candidates: Vec<Value>,
}

impl Failure {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            candidates: Vec::new(),
        }
    }

    fn json(&self) -> Value {
        let mut value = json!({"error": {"code": self.code, "message": self.message}});
        if !self.candidates.is_empty() {
            value["candidates"] = json!(self.candidates);
        }
        value
    }
}

impl From<AppError> for Failure {
    fn from(error: AppError) -> Self {
        Self::new(error.code(), crate::format_lib_error(&error))
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        let code = match error.kind() {
            std::io::ErrorKind::InvalidData => "mcp_control_response",
            _ => "mcp_control_unreachable",
        };
        Self::new(code, error.to_string())
    }
}

async fn request(
    descriptor: &ControlDescriptor,
    command: &ControlRequest,
) -> Result<Value, Failure> {
    let value = mcp_control::request(&descriptor.endpoint, command).await?;
    if let Some(error) = value.get("error") {
        return Err(Failure::new(
            error["code"].as_str().unwrap_or("mcp_control_response"),
            error["message"]
                .as_str()
                .unwrap_or("Local MCP control request failed"),
        ));
    }
    Ok(value)
}

async fn discover(
    descriptors: Vec<ControlDescriptor>,
) -> Result<(Vec<Instance>, Vec<Value>), Failure> {
    let mut live = Vec::new();
    let mut ignored = Vec::new();
    for descriptor in descriptors {
        let result = request(&descriptor, &ControlRequest::Targets)
            .await
            .and_then(|value| {
                let targets: Targets = serde_json::from_value(value)
                    .map_err(|_| Failure::new("mcp_control_response", "Invalid target listing"))?;
                if targets.instance_id != descriptor.instance_id
                    || targets
                        .targets
                        .iter()
                        .any(|target| target.owner_id.is_empty())
                {
                    return Err(Failure::new(
                        "mcp_control_response",
                        "Control instance or owner identity did not match",
                    ));
                }
                Ok(targets.targets)
            });
        match result {
            Ok(targets) => live.push(Instance {
                descriptor,
                targets,
            }),
            Err(error) => ignored.push(
                json!({"instance_id": descriptor.instance_id, "error": error.json()["error"]}),
            ),
        }
    }
    Ok((live, ignored))
}

fn candidate(instance: &Instance, target: &Target) -> Value {
    json!({"instance_id": instance.descriptor.instance_id, "host": instance.descriptor.host, "owner_id": target.owner_id, "label": target.label})
}

fn select<'a>(
    instances: &'a [Instance],
    selection: &Selection,
) -> Result<(&'a Instance, &'a Target), Failure> {
    let candidates: Vec<_> = instances
        .iter()
        .filter(|instance| {
            selection
                .instance
                .is_none_or(|id| id.to_string() == instance.descriptor.instance_id)
        })
        .flat_map(|instance| {
            instance
                .targets
                .iter()
                .map(move |target| (instance, target))
        })
        .filter(|(_, target)| {
            selection
                .owner
                .as_ref()
                .is_none_or(|owner| owner == &target.owner_id)
        })
        .collect();
    if candidates.len() == 1 {
        return Ok(candidates[0]);
    }
    let (code, message) = if candidates.is_empty() {
        (
            "mcp_target_not_found",
            format!("No matching live RSSH frontend. {NO_TARGET}"),
        )
    } else {
        ("mcp_target_ambiguous", "Multiple RSSH frontends match. Select --instance and --owner using the candidates below.".to_owned())
    };
    let mut error = Failure::new(code, message);
    error.candidates = if candidates.is_empty() {
        instances
            .iter()
            .flat_map(|instance| {
                instance
                    .targets
                    .iter()
                    .map(move |target| candidate(instance, target))
            })
            .collect()
    } else {
        candidates
            .iter()
            .map(|(instance, target)| candidate(instance, target))
            .collect()
    };
    Err(error)
}

fn validate_connection(value: &Value, instance: &Instance, target: &Target) -> Result<(), Failure> {
    let endpoint = value["url"].as_str().and_then(|s| url::Url::parse(s).ok());
    if value["instance_id"] != instance.descriptor.instance_id
        || value["owner_id"] != target.owner_id
        || value["transport"] != "streamable-http"
        || !value["headers"]["Authorization"]
            .as_str()
            .is_some_and(|header| {
                header.starts_with("Bearer ") && header.len() > 7 && !header.contains(['\r', '\n'])
            })
        || !endpoint.is_some_and(|url| {
            url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.port().is_some_and(|port| port > 0)
                && url.path() == "/mcp"
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        })
    {
        return Err(Failure::new(
            "mcp_control_response",
            "Invalid MCP connection information",
        ));
    }
    Ok(())
}

async fn execute(action: &McpCmd, instances: &[Instance]) -> Result<Value, Failure> {
    let selection = match action {
        McpCmd::Start(selection) | McpCmd::Status(selection) | McpCmd::Stop(selection) => selection,
    };
    let (instance, target) = select(instances, selection)?;
    let owner_id = target.owner_id.clone();
    let command = match action {
        McpCmd::Start(_) => ControlRequest::Start { owner_id },
        McpCmd::Stop(_) => ControlRequest::Stop { owner_id },
        McpCmd::Status(_) => ControlRequest::Status { owner_id },
    };
    let value = request(&instance.descriptor, &command).await?;
    if matches!(action, McpCmd::Start(_)) {
        validate_connection(&value, instance, target)?;
    } else {
        if value["instance_id"] != instance.descriptor.instance_id
            || value["owner_id"] != target.owner_id
            || !value["running"].is_boolean()
        {
            return Err(Failure::new(
                "mcp_control_response",
                "Invalid MCP status identity",
            ));
        }
        if matches!(action, McpCmd::Stop(_)) && value["running"] != false {
            return Err(Failure::new(
                "mcp_control_response",
                "MCP stop response reports the service is still running",
            ));
        }
        if value["running"] == true {
            validate_connection(&value["connection"], instance, target)?;
        } else if value.get("connection") != Some(&Value::Null) {
            return Err(Failure::new(
                "mcp_control_response",
                "Stopped MCP status included a connection",
            ));
        }
    }
    Ok(value)
}

pub fn run(action: &McpCmd) -> (Value, bool) {
    let mut warnings = Vec::new();
    let result = (|| -> Result<Value, Failure> {
        let data_dir = rssh_core::db::data_dir()?;
        let scan = mcp_control::read_descriptors(&data_dir)?;
        warnings.extend(scan.ignored.into_iter().map(|message| json!({"error": {"code": "mcp_control_descriptor_invalid", "message": message}})));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| Failure::new("mcp_control_runtime", error.to_string()))?;
        runtime.block_on(async {
            let (instances, ignored) = discover(scan.descriptors).await?;
            warnings.extend(ignored);
            execute(action, &instances).await
        })
    })();
    let (mut output, success) = match result {
        Ok(output) => (output, true),
        Err(error) => (error.json(), false),
    };
    if !warnings.is_empty() {
        output["warnings"] = json!(warnings);
    }
    (output, success)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};
    use mcp_control::{native_transport, read_json, write_json, REQUEST_LIMIT, RESPONSE_LIMIT};
    use tokio::io::AsyncWriteExt;

    fn instance() -> Instance {
        let instance_id = uuid::Uuid::new_v4().to_string();
        Instance {
            descriptor: ControlDescriptor {
                version: mcp_control::CONTROL_VERSION,
                endpoint: native_transport().endpoint(&instance_id).unwrap(),
                instance_id,
                pid: 1,
                host: "desktop".into(),
            },
            targets: vec![Target {
                owner_id: "window:main".into(),
                label: "Main".into(),
            }],
        }
    }

    fn connection(instance: &Instance) -> Value {
        json!({
            "url": "http://127.0.0.1:12345/mcp",
            "headers": {"Authorization": "Bearer endpoint-token"},
            "transport": "streamable-http",
            "instance_id": instance.descriptor.instance_id,
            "owner_id": "window:main",
        })
    }

    async fn serve(
        instance: &Instance,
        exchanges: Vec<(ControlRequest, Value)>,
    ) -> tokio::task::JoinHandle<()> {
        let mut listener = native_transport()
            .bind(&instance.descriptor.endpoint)
            .await
            .unwrap();
        tokio::spawn(async move {
            for (expected, response) in exchanges {
                let mut stream = listener.accept().await.unwrap();
                let request: Value = read_json(&mut *stream, REQUEST_LIMIT).await.unwrap();
                assert_eq!(request, serde_json::to_value(expected).unwrap());
                assert!(request.get("token").is_none());
                assert!(request.get("headers").is_none());
                write_json(&mut *stream, &response, RESPONSE_LIMIT)
                    .await
                    .unwrap();
            }
        })
    }

    #[test]
    fn public_mcp_commands_are_only_start_stop_and_status() {
        let command = crate::Cli::command();
        let mcp = command.find_subcommand("mcp").unwrap();
        let commands: Vec<_> = mcp
            .get_subcommands()
            .map(|command| command.get_name())
            .collect();
        assert_eq!(commands, ["start", "stop", "status"]);
        for action in commands {
            assert!(crate::Cli::try_parse_from(["rssh", "mcp", action]).is_ok());
        }
        assert!(crate::Cli::try_parse_from(["rssh", "mcp", "list"]).is_err());
    }

    #[test]
    fn selection_requires_one_matching_existing_owner() {
        let mut one = instance();
        assert!(select(std::slice::from_ref(&one), &Selection::default()).is_ok());
        one.targets.push(Target {
            owner_id: "window:other".into(),
            label: "Other".into(),
        });
        let error = select(std::slice::from_ref(&one), &Selection::default())
            .err()
            .unwrap();
        assert_eq!(error.code, "mcp_target_ambiguous");
        assert_eq!(error.candidates.len(), 2);
        let selected = Selection {
            instance: Some(uuid::Uuid::parse_str(&one.descriptor.instance_id).unwrap()),
            owner: Some("window:other".into()),
        };
        assert_eq!(
            select(std::slice::from_ref(&one), &selected)
                .unwrap()
                .1
                .owner_id,
            "window:other"
        );
        assert_eq!(
            select(&[], &Selection::default()).err().unwrap().code,
            "mcp_target_not_found"
        );
    }

    #[tokio::test]
    async fn descriptor_discovery_start_status_stop_need_no_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let instance = instance();
        let connection = connection(&instance);
        let owner_id = "window:main".to_owned();
        let server = serve(&instance, vec![
            (ControlRequest::Targets, json!({"instance_id": instance.descriptor.instance_id, "targets": instance.targets})),
            (ControlRequest::Start { owner_id: owner_id.clone() }, connection.clone()),
            (ControlRequest::Status { owner_id: owner_id.clone() }, json!({"instance_id": instance.descriptor.instance_id, "owner_id": owner_id, "running": true, "connection": connection})),
            (ControlRequest::Stop { owner_id: owner_id.clone() }, json!({"instance_id": instance.descriptor.instance_id, "owner_id": owner_id, "running": false, "connection": null})),
        ]).await;
        mcp_control::publish_descriptor(directory.path(), &instance.descriptor).unwrap();
        let scan = mcp_control::read_descriptors(directory.path()).unwrap();
        let (instances, warnings) = discover(scan.descriptors).await.unwrap();
        assert!(warnings.is_empty());
        let started = execute(&McpCmd::Start(Selection::default()), &instances)
            .await
            .unwrap();
        assert_eq!(started, connection);
        let status = execute(&McpCmd::Status(Selection::default()), &instances)
            .await
            .unwrap();
        assert_eq!(status["running"], true);
        assert_eq!(status["connection"], connection);
        let stopped = execute(&McpCmd::Stop(Selection::default()), &instances)
            .await
            .unwrap();
        assert_eq!(stopped["running"], false);
        assert_eq!(stopped["connection"], Value::Null);
        assert!(!directory.path().join("rssh.db").exists());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn server_error_code_and_message_remain_machine_readable() {
        let instance = instance();
        let expected = json!({"error":{"code":"mcp_owner_unavailable","message":"The selected frontend closed"}});
        let server = serve(&instance, vec![(ControlRequest::Targets, expected.clone())]).await;
        let error = request(&instance.descriptor, &ControlRequest::Targets)
            .await
            .unwrap_err();
        assert_eq!(error.json(), expected);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn stop_requires_a_stopped_response() {
        for running in [false, true] {
            let instance = instance();
            let response = json!({
                "running": running,
                "connection": if running { connection(&instance) } else { Value::Null },
                "instance_id": instance.descriptor.instance_id,
                "owner_id": "window:main",
            });
            let server = serve(
                &instance,
                vec![(
                    ControlRequest::Stop {
                        owner_id: "window:main".into(),
                    },
                    response.clone(),
                )],
            )
            .await;
            let selection = Selection {
                instance: Some(uuid::Uuid::parse_str(&instance.descriptor.instance_id).unwrap()),
                owner: Some("window:main".into()),
            };
            let result = execute(&McpCmd::Stop(selection), &[instance]).await;
            if running {
                let error = result.unwrap_err();
                assert_eq!(error.code, "mcp_control_response");
                assert_eq!(
                    error.message,
                    "MCP stop response reports the service is still running"
                );
            } else {
                assert_eq!(result.unwrap(), response);
            }
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn discovery_reports_stale_instances_without_hiding_live_frontends() {
        let good = instance();
        let stale = instance();
        let server = serve(
            &good,
            vec![(
                ControlRequest::Targets,
                json!({"instance_id":good.descriptor.instance_id,"targets":good.targets}),
            )],
        )
        .await;
        let (live, warnings) = discover(vec![stale.descriptor, good.descriptor])
            .await
            .unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(warnings.len(), 1);
        let (_, target) = select(&live, &Selection::default()).unwrap();
        assert_eq!(target.owner_id, "window:main");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn wrong_instance_response_is_not_a_live_target() {
        let instance = instance();
        let server = serve(
            &instance,
            vec![(
                ControlRequest::Targets,
                json!({"instance_id":uuid::Uuid::new_v4().to_string(),"targets":instance.targets}),
            )],
        )
        .await;
        let (live, warnings) = discover(vec![instance.descriptor]).await.unwrap();
        assert!(live.is_empty());
        assert_eq!(warnings[0]["error"]["code"], "mcp_control_response");
        server.await.unwrap();
    }

    #[test]
    fn invalid_connection_cannot_redirect_the_agent_to_an_external_host() {
        let instance = instance();
        let mut connection = connection(&instance);
        connection["url"] = json!("http://example.com/mcp");
        assert!(validate_connection(&connection, &instance, &instance.targets[0]).is_err());
    }

    #[tokio::test]
    async fn oversized_response_is_a_json_cli_error() {
        let instance = instance();
        let mut listener = native_transport()
            .bind(&instance.descriptor.endpoint)
            .await
            .unwrap();
        let server = tokio::spawn(async move {
            let mut stream = listener.accept().await.unwrap();
            let _: ControlRequest = read_json(&mut *stream, REQUEST_LIMIT).await.unwrap();
            stream
                .write_all(&((RESPONSE_LIMIT + 1) as u32).to_be_bytes())
                .await
                .unwrap();
            stream.flush().await.unwrap();
        });
        let error = request(&instance.descriptor, &ControlRequest::Targets)
            .await
            .unwrap_err();
        assert_eq!(error.json()["error"]["code"], "mcp_control_response");
        server.await.unwrap();
    }

    #[test]
    fn transport_timeouts_remain_json_cli_errors() {
        let error = Failure::from(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "MCP control request timed out",
        ));
        assert_eq!(
            error.json(),
            json!({"error":{"code":"mcp_control_unreachable","message":"MCP control request timed out"}})
        );
    }
}
