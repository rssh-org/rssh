//! Machine-readable bootstrap for the MCP service owned by a running frontend.

use std::time::Duration;

use clap::{Args, Subcommand};
use reqwest::{Client, Method};
use rssh_core::error::AppError;
use rssh_core::mcp_control::{self, ControlDescriptor};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const NO_TARGET: &str = "Run an updated RSSH desktop GUI or a local headless/JetBrains frontend, then retry. The GUI/server and CLI must both support MCP control.";
const MAX_RESPONSE_BYTES: usize = 128 * 1024;

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

fn client() -> Result<Client, Failure> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| Failure::new("mcp_control_http", error.to_string()))
}

async fn request(
    client: &Client,
    descriptor: &ControlDescriptor,
    path: &str,
    body: Option<Value>,
) -> Result<Value, Failure> {
    let url = format!("http://127.0.0.1:{}{path}", descriptor.port);
    let mut request = client.request(
        if body.is_some() {
            Method::POST
        } else {
            Method::GET
        },
        url,
    );
    if let Some(body) = body {
        request = request.json(&body);
    }
    let mut response = request
        .send()
        .await
        .map_err(|error| Failure::new("mcp_control_unreachable", error.to_string()))?;
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| Failure::new("mcp_control_response", error.to_string()))?
    {
        if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(Failure::new(
                "mcp_control_response",
                "Control response exceeds the size limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        Failure::new(
            "mcp_control_response",
            format!("Invalid control JSON response (HTTP {})", status.as_u16()),
        )
    })?;
    if !status.is_success() {
        return Err(Failure::new(
            value["error"]["code"]
                .as_str()
                .unwrap_or("mcp_control_http"),
            value["error"]["message"]
                .as_str()
                .unwrap_or("Local MCP control request failed"),
        ));
    }
    Ok(value)
}

async fn discover(
    client: &Client,
    descriptors: Vec<ControlDescriptor>,
) -> Result<(Vec<Instance>, Vec<Value>), Failure> {
    let mut live = Vec::new();
    let mut ignored = Vec::new();
    for descriptor in descriptors {
        let result = request(client, &descriptor, "/v1/targets", None)
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
    let endpoint = value["url"]
        .as_str()
        .and_then(|s| reqwest::Url::parse(s).ok());
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

async fn execute(
    action: &McpCmd,
    client: &Client,
    instances: &[Instance],
) -> Result<Value, Failure> {
    let (path, selection) = match action {
        McpCmd::Start(selection) => ("/v1/mcp/start", selection),
        McpCmd::Status(selection) => ("/v1/mcp/status", selection),
        McpCmd::Stop(selection) => ("/v1/mcp/stop", selection),
    };
    let (instance, target) = select(instances, selection)?;
    let value = request(
        client,
        &instance.descriptor,
        path,
        Some(json!({"owner_id": target.owner_id})),
    )
    .await?;
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
        let client = client()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| Failure::new("mcp_control_runtime", error.to_string()))?;
        runtime.block_on(async {
            let (instances, ignored) = discover(&client, scan.descriptors).await?;
            warnings.extend(ignored);
            execute(action, &client, &instances).await
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

    async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            let count = stream.read(&mut chunk).await.unwrap();
            assert!(count > 0, "request ended before its complete body");
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&bytes[..end])
                    .unwrap()
                    .to_ascii_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .map(|value| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    return String::from_utf8(bytes).unwrap();
                }
            }
            assert!(bytes.len() <= 8192);
        }
    }

    fn instance() -> Instance {
        Instance {
            descriptor: ControlDescriptor {
                version: mcp_control::CONTROL_VERSION,
                instance_id: uuid::Uuid::new_v4().to_string(),
                pid: 1,
                port: 12345,
                host: "desktop".into(),
            },
            targets: vec![Target {
                owner_id: "window:main".into(),
                label: "Main".into(),
            }],
        }
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
    async fn start_posts_selected_owner_and_returns_the_connection_json() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut instance = instance();
        instance.descriptor.port = listener.local_addr().unwrap().port();
        let connection = json!({"url":"http://127.0.0.1:12345/mcp", "headers":{"Authorization":"Bearer endpoint-token"}, "transport":"streamable-http", "instance_id": instance.descriptor.instance_id, "owner_id":"window:main"});
        let expected = connection.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            assert!(request.starts_with("POST /v1/mcp/start HTTP/1.1"));
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
            assert!(request.contains("\"owner_id\":\"window:main\""));
            let body = connection.to_string();
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        });
        let result = execute(
            &McpCmd::Start(Selection::default()),
            &client().unwrap(),
            &[instance],
        )
        .await
        .unwrap();
        assert_eq!(result, expected);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn control_client_does_not_follow_redirects() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut instance = instance();
        instance.descriptor.port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            let body = r#"{"error":{"code":"redirect_denied","message":"No redirect"}}"#;
            stream.write_all(format!("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/steal\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        });
        let error = request(
            &client().unwrap(),
            &instance.descriptor,
            "/v1/targets",
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "redirect_denied");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn stop_posts_selected_owner_and_requires_a_stopped_response() {
        for running in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mut instance = instance();
            instance.descriptor.port = listener.local_addr().unwrap().port();
            let connection = json!({"url":"http://127.0.0.1:12345/mcp", "headers":{"Authorization":"Bearer endpoint-token"}, "transport":"streamable-http", "instance_id": instance.descriptor.instance_id, "owner_id":"window:main"});
            let response = json!({"running": running, "connection": if running { connection } else { Value::Null }, "instance_id": instance.descriptor.instance_id, "owner_id":"window:main"});
            let expected = response.clone();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_request(&mut stream).await;
                assert!(request.starts_with("POST /v1/mcp/stop HTTP/1.1"));
                assert!(!request.to_ascii_lowercase().contains("authorization:"));
                let (_, body) = request.split_once("\r\n\r\n").unwrap();
                assert_eq!(
                    serde_json::from_str::<Value>(body).unwrap(),
                    json!({"owner_id":"window:main"})
                );
                let body = response.to_string();
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            });
            let selection = Selection {
                instance: Some(uuid::Uuid::parse_str(&instance.descriptor.instance_id).unwrap()),
                owner: Some("window:main".into()),
            };
            let result = execute(&McpCmd::Stop(selection), &client().unwrap(), &[instance]).await;
            if running {
                let error = result.unwrap_err();
                assert_eq!(error.code, "mcp_control_response");
                assert_eq!(
                    error.message,
                    "MCP stop response reports the service is still running"
                );
            } else {
                assert_eq!(result.unwrap(), expected);
            }
            server.await.unwrap();
        }
    }

    #[test]
    fn invalid_connection_cannot_redirect_the_agent_to_an_external_host() {
        let instance = instance();
        let connection = json!({"url":"http://example.com/mcp", "headers":{"Authorization":"Bearer endpoint-token"}, "transport":"streamable-http", "instance_id": instance.descriptor.instance_id, "owner_id":"window:main"});
        assert!(validate_connection(&connection, &instance, &instance.targets[0]).is_err());
    }

    #[tokio::test]
    async fn discovery_reports_stale_instances_without_hiding_live_frontends() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut good = instance();
        good.descriptor.port = listener.local_addr().unwrap().port();
        let reply = json!({"instance_id": good.descriptor.instance_id, "targets":good.targets});
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert!(read_request(&mut stream)
                .await
                .starts_with("GET /v1/targets HTTP/1.1"));
            let body = reply.to_string();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let stale_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut stale = instance();
        stale.descriptor.port = stale_listener.local_addr().unwrap().port();
        drop(stale_listener);
        let client = client().unwrap();
        let (live, warnings) = discover(&client, vec![stale.descriptor, good.descriptor])
            .await
            .unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(warnings.len(), 1);
        let (_, target) = select(&live, &Selection::default()).unwrap();
        assert_eq!(target.owner_id, "window:main");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn descriptor_discovery_start_status_stop_need_no_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut instance = instance();
        instance.descriptor.port = listener.local_addr().unwrap().port();
        mcp_control::publish_descriptor(directory.path(), &instance.descriptor).unwrap();
        assert!(!directory.path().join("rssh.db").exists());
        let instance_id = instance.descriptor.instance_id.clone();
        let connection = json!({"url":"http://127.0.0.1:12345/mcp", "headers":{"Authorization":"Bearer endpoint-token"}, "transport":"streamable-http", "instance_id": instance_id, "owner_id":"window:main"});
        let expected_connection = connection.clone();
        let server = tokio::spawn(async move {
            for route in [
                "GET /v1/targets",
                "POST /v1/mcp/start",
                "POST /v1/mcp/status",
                "POST /v1/mcp/stop",
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_request(&mut stream).await;
                assert!(request.starts_with(&format!("{route} HTTP/1.1")));
                let (headers, body) = request.split_once("\r\n\r\n").unwrap();
                assert!(!headers.to_ascii_lowercase().contains("authorization:"));
                if route.starts_with("POST ") {
                    assert_eq!(
                        serde_json::from_str::<Value>(body).unwrap(),
                        json!({"owner_id":"window:main"})
                    );
                }
                let response = match route {
                    "GET /v1/targets" => {
                        json!({"instance_id": instance_id, "targets":[{"owner_id":"window:main","label":"Main"}]})
                    }
                    "POST /v1/mcp/start" => connection.clone(),
                    "POST /v1/mcp/status" => {
                        json!({"instance_id": instance_id, "owner_id":"window:main", "running":true,"connection":connection})
                    }
                    _ => {
                        json!({"instance_id": instance_id, "owner_id":"window:main", "running":false,"connection":null})
                    }
                };
                let body = response.to_string();
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            }
        });
        let client = client().unwrap();
        let scan = mcp_control::read_descriptors(directory.path()).unwrap();
        let (instances, warnings) = discover(&client, scan.descriptors).await.unwrap();
        assert!(warnings.is_empty());
        let started = execute(&McpCmd::Start(Selection::default()), &client, &instances)
            .await
            .unwrap();
        assert_eq!(started, expected_connection);
        let status = execute(&McpCmd::Status(Selection::default()), &client, &instances)
            .await
            .unwrap();
        assert_eq!(status["running"], true);
        assert_eq!(status["connection"], expected_connection);
        let stopped = execute(&McpCmd::Stop(Selection::default()), &client, &instances)
            .await
            .unwrap();
        assert_eq!(stopped["running"], false);
        assert_eq!(stopped["connection"], Value::Null);
        assert!(!directory.path().join("rssh.db").exists());
        server.await.unwrap();
    }
}
