use std::io;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(any(macos, linux))]
#[path = "unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;
#[cfg(any(android, ios, ohos))]
#[path = "unsupported.rs"]
mod platform;

pub trait LocalStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> LocalStream for T {}
pub type BoxStream = Box<dyn LocalStream>;

#[async_trait]
pub trait LocalListener: Send {
    async fn accept(&mut self) -> io::Result<BoxStream>;
}

/// The only platform boundary for the control protocol. Application and CLI
/// callers exchange the same requests without knowing socket or pipe details.
#[async_trait]
pub trait LocalTransport: Send + Sync {
    async fn bind(&self, endpoint: &str) -> io::Result<Box<dyn LocalListener>>;
    async fn connect(&self, endpoint: &str) -> io::Result<BoxStream>;
    fn endpoint(&self, instance_id: &str) -> io::Result<String>;
    fn validate_endpoint(&self, endpoint: &str, instance_id: &str) -> io::Result<()>;
}

pub fn native_transport() -> &'static dyn LocalTransport {
    &platform::TRANSPORT
}

pub(super) fn validate_instance(instance_id: &str) -> io::Result<()> {
    let id = uuid::Uuid::parse_str(instance_id)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid instance UUID"))?;
    if id.to_string() != instance_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "instance UUID must use canonical lowercase form",
        ));
    }
    Ok(())
}

#[cfg(all(test, any(macos, linux, windows)))]
mod tests {
    use super::*;
    use crate::mcp_control::{
        read_json, write_json, ControlRequest, REQUEST_LIMIT, RESPONSE_LIMIT,
    };
    use serde_json::{json, Value};

    #[tokio::test]
    async fn native_round_trip_supports_concurrent_clients() {
        let id = uuid::Uuid::new_v4().to_string();
        let endpoint = native_transport().endpoint(&id).unwrap();
        let mut listener = native_transport().bind(&endpoint).await.unwrap();
        assert!(native_transport().bind(&endpoint).await.is_err());
        let serving = tokio::spawn(async move {
            let mut clients = tokio::task::JoinSet::new();
            for _ in 0..8 {
                let mut stream = listener.accept().await.unwrap();
                clients.spawn(async move {
                    let request: ControlRequest =
                        read_json(&mut stream, REQUEST_LIMIT).await.unwrap();
                    assert!(matches!(request, ControlRequest::Targets));
                    write_json(&mut stream, &json!({"targets": []}), RESPONSE_LIMIT)
                        .await
                        .unwrap();
                });
            }
            while let Some(client) = clients.join_next().await {
                client.unwrap();
            }
        });
        let mut clients = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let endpoint = endpoint.clone();
            clients.spawn(async move {
                let response: Value =
                    crate::mcp_control::request(&endpoint, &ControlRequest::Targets)
                        .await
                        .unwrap();
                assert_eq!(response, json!({"targets": []}));
            });
        }
        while let Some(client) = clients.join_next().await {
            client.unwrap();
        }
        serving.await.unwrap();
        assert!(native_transport().connect(&endpoint).await.is_err());
        // A clean shutdown permits a future listener to reuse the same name.
        let _replacement = native_transport().bind(&endpoint).await.unwrap();
    }

    #[tokio::test]
    async fn cancelling_accept_keeps_the_listener_available() {
        let id = uuid::Uuid::new_v4().to_string();
        let endpoint = native_transport().endpoint(&id).unwrap();
        let mut listener = native_transport().bind(&endpoint).await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(5), listener.accept())
                .await
                .is_err()
        );
        let client = tokio::spawn(async move {
            crate::mcp_control::request(&endpoint, &ControlRequest::Targets)
                .await
                .unwrap()
        });
        let mut stream = listener.accept().await.unwrap();
        assert_eq!(
            read_json::<ControlRequest>(&mut stream, REQUEST_LIMIT)
                .await
                .unwrap(),
            ControlRequest::Targets
        );
        write_json(&mut stream, &json!({"targets": []}), RESPONSE_LIMIT)
            .await
            .unwrap();
        assert_eq!(client.await.unwrap(), json!({"targets": []}));
    }

    #[tokio::test]
    async fn disconnected_client_does_not_poison_the_next_accept() {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let id = uuid::Uuid::new_v4().to_string();
            let endpoint = native_transport().endpoint(&id).unwrap();
            let mut listener = native_transport().bind(&endpoint).await.unwrap();

            // In particular, Windows may report ERROR_NO_DATA when a client
            // opens and closes the pipe before the server calls connect().
            drop(native_transport().connect(&endpoint).await.unwrap());
            let mut abandoned = listener.accept().await.unwrap();
            assert!(read_json::<ControlRequest>(&mut abandoned, REQUEST_LIMIT)
                .await
                .is_err());
            drop(abandoned);

            let client = tokio::spawn(async move {
                crate::mcp_control::request(&endpoint, &ControlRequest::Targets)
                    .await
                    .unwrap()
            });
            let mut stream = listener.accept().await.unwrap();
            assert_eq!(
                read_json::<ControlRequest>(&mut stream, REQUEST_LIMIT)
                    .await
                    .unwrap(),
                ControlRequest::Targets
            );
            write_json(&mut stream, &json!({"targets": []}), RESPONSE_LIMIT)
                .await
                .unwrap();
            assert_eq!(client.await.unwrap(), json!({"targets": []}));
        })
        .await
        .expect("a disconnected client must not block subsequent requests");
    }

    #[test]
    fn endpoints_are_bound_to_the_native_platform_and_instance() {
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        let endpoint = native_transport().endpoint(&first).unwrap();
        native_transport()
            .validate_endpoint(&endpoint, &first)
            .unwrap();
        assert!(native_transport()
            .validate_endpoint(&endpoint, &second)
            .is_err());
        assert!(native_transport()
            .validate_endpoint("http://127.0.0.1:1234", &first)
            .is_err());
        assert!(native_transport().endpoint("../../outside").is_err());
    }
}
