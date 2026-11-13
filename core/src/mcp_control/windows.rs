use std::io;
use std::time::Duration;

use async_trait::async_trait;
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

use super::{validate_instance, BoxStream, LocalListener, LocalTransport};

pub(super) static TRANSPORT: WindowsTransport = WindowsTransport;
pub(super) struct WindowsTransport;
const PREFIX: &str = r"\\.\pipe\rssh-mcp-";
const ERROR_PIPE_BUSY: i32 = 231;

fn validate_path(endpoint: &str) -> io::Result<()> {
    let instance_id = endpoint
        .strip_prefix(PREFIX)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid MCP pipe name"))?;
    TRANSPORT.validate_endpoint(endpoint, instance_id)
}

fn create_server(endpoint: &str, first: bool) -> io::Result<NamedPipeServer> {
    ServerOptions::new()
        .first_pipe_instance(first)
        .reject_remote_clients(true)
        .create(endpoint)
}

#[async_trait]
impl LocalTransport for WindowsTransport {
    async fn bind(&self, endpoint: &str) -> io::Result<Box<dyn LocalListener>> {
        validate_path(endpoint)?;
        Ok(Box::new(PipeListener {
            server: create_server(endpoint, true)?,
            endpoint: endpoint.into(),
        }))
    }

    async fn connect(&self, endpoint: &str) -> io::Result<BoxStream> {
        validate_path(endpoint)?;
        loop {
            // Preserve ClientOptions' SECURITY_IDENTIFICATION default: opening
            // another process's pipe must not grant it client impersonation.
            match ClientOptions::new().open(endpoint) {
                Ok(client) => return Ok(Box::new(client)),
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    // request() bounds this retry loop with its connect timeout.
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn endpoint(&self, instance_id: &str) -> io::Result<String> {
        validate_instance(instance_id)?;
        Ok(format!("{PREFIX}{instance_id}"))
    }

    fn validate_endpoint(&self, endpoint: &str, instance_id: &str) -> io::Result<()> {
        if endpoint != self.endpoint(instance_id)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "MCP pipe does not match its instance",
            ));
        }
        Ok(())
    }
}

struct PipeListener {
    server: NamedPipeServer,
    endpoint: String,
}

#[async_trait]
impl LocalListener for PipeListener {
    async fn accept(&mut self) -> io::Result<BoxStream> {
        self.server.connect().await?;
        // Keep at least one server instance alive between accepted clients.
        // The first-instance guard applies only to bind, not this replacement.
        // No await between creating the successor and handing off the client.
        let successor = create_server(&self.endpoint, false)?;
        let connected = std::mem::replace(&mut self.server, successor);
        Ok(Box::new(connected))
    }
}
