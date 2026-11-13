use super::{BoxStream, LocalListener, LocalTransport};
use async_trait::async_trait;
use std::io;

pub(super) static TRANSPORT: UnsupportedTransport = UnsupportedTransport;
pub(super) struct UnsupportedTransport;

fn unsupported<T>() -> io::Result<T> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "local MCP control is unavailable on this platform",
    ))
}

#[async_trait]
impl LocalTransport for UnsupportedTransport {
    async fn bind(&self, _: &str) -> io::Result<Box<dyn LocalListener>> {
        unsupported()
    }
    async fn connect(&self, _: &str) -> io::Result<BoxStream> {
        unsupported()
    }
    fn endpoint(&self, _: &str) -> io::Result<String> {
        unsupported()
    }
    fn validate_endpoint(&self, _: &str, _: &str) -> io::Result<()> {
        unsupported()
    }
}
