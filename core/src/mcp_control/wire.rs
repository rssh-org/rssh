use std::io;
use std::time::Duration;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::{native_transport, LocalTransport};

pub const REQUEST_LIMIT: usize = 8 * 1024;
pub const RESPONSE_LIMIT: usize = 128 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ControlRequest {
    Targets,
    Start { owner_id: String },
    Stop { owner_id: String },
    Status { owner_id: String },
}

/// Read exactly one u32 big-endian length-prefixed JSON frame. The bound is
/// checked before allocation, including when the peer never sends its payload.
pub async fn read_json<T: DeserializeOwned>(
    stream: &mut (impl AsyncRead + Unpin + ?Sized),
    limit: usize,
) -> io::Result<T> {
    let length = stream.read_u32().await? as usize;
    if length == 0 || length > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid control frame length",
        ));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid control JSON"))
}

pub async fn write_json<T: Serialize>(
    stream: &mut (impl AsyncWrite + Unpin + ?Sized),
    value: &T,
    limit: usize,
) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "cannot encode control JSON"))?;
    if bytes.len() > limit || bytes.len() > u32::MAX as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control frame exceeds limit",
        ));
    }
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await
}

/// Each CLI operation owns one short connection. The total deadline includes
/// connection establishment, writing, and reading; a busy pipe cannot hang it.
pub async fn request(endpoint: &str, request: &ControlRequest) -> io::Result<Value> {
    request_with_transport(
        native_transport(),
        endpoint,
        request,
        CONNECT_TIMEOUT,
        REQUEST_TIMEOUT,
    )
    .await
}

async fn request_with_transport(
    transport: &dyn LocalTransport,
    endpoint: &str,
    request: &ControlRequest,
    connect_timeout: Duration,
    request_timeout: Duration,
) -> io::Result<Value> {
    tokio::time::timeout(request_timeout, async {
        let mut stream = tokio::time::timeout(connect_timeout, transport.connect(endpoint))
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "control connection timed out")
            })??;
        write_json(&mut stream, request, REQUEST_LIMIT).await?;
        read_json(&mut stream, RESPONSE_LIMIT).await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "control request timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_control::{BoxStream, LocalListener};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    #[tokio::test]
    async fn frames_preserve_boundaries_and_validate_before_reading_payloads() {
        let (mut writer, mut reader) = tokio::io::duplex(256);
        write_json(&mut writer, &ControlRequest::Targets, REQUEST_LIMIT)
            .await
            .unwrap();
        write_json(
            &mut writer,
            &ControlRequest::Stop {
                owner_id: "window-main".into(),
            },
            REQUEST_LIMIT,
        )
        .await
        .unwrap();
        assert_eq!(
            read_json::<ControlRequest>(&mut reader, REQUEST_LIMIT)
                .await
                .unwrap(),
            ControlRequest::Targets
        );
        assert_eq!(
            read_json::<ControlRequest>(&mut reader, REQUEST_LIMIT)
                .await
                .unwrap(),
            ControlRequest::Stop {
                owner_id: "window-main".into()
            }
        );
        writer.write_u32((REQUEST_LIMIT + 1) as u32).await.unwrap();
        assert_eq!(
            read_json::<ControlRequest>(&mut reader, REQUEST_LIMIT)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        writer.write_u32(0).await.unwrap();
        assert_eq!(
            read_json::<ControlRequest>(&mut reader, REQUEST_LIMIT)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[tokio::test]
    async fn rejects_malformed_truncated_and_oversize_frames() {
        for payload in [b"{".as_slice(), b"not-json".as_slice()] {
            let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
            frame.extend_from_slice(payload);
            assert_eq!(
                read_json::<Value>(&mut frame.as_slice(), REQUEST_LIMIT)
                    .await
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        let mut truncated = 10u32.to_be_bytes().to_vec();
        truncated.extend_from_slice(b"{}");
        assert_eq!(
            read_json::<Value>(&mut truncated.as_slice(), REQUEST_LIMIT)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        let mut output = Vec::new();
        assert_eq!(
            write_json(&mut output, &json!({"value": "x".repeat(32)}), 16)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert!(output.is_empty());
    }

    #[test]
    fn request_contract_rejects_unknown_methods_and_fields() {
        assert_eq!(
            serde_json::to_value(ControlRequest::Targets).unwrap(),
            json!({"method":"targets"})
        );
        assert_eq!(
            serde_json::to_value(ControlRequest::Start {
                owner_id: "window-main".into()
            })
            .unwrap(),
            json!({"method":"start", "params":{"owner_id":"window-main"}})
        );
        for value in [
            json!({"method":"execute"}),
            json!({"method":"targets", "command":"execute"}),
            json!({"method":"start", "params":{"owner_id":"window-main", "command":"execute"}}),
            json!({"method":"status", "params":{}}),
        ] {
            assert!(serde_json::from_value::<ControlRequest>(value).is_err());
        }
    }

    struct SimulatedTransport {
        delay: Duration,
        stream: Mutex<Option<BoxStream>>,
    }

    #[async_trait]
    impl LocalTransport for SimulatedTransport {
        async fn connect(&self, _: &str) -> io::Result<BoxStream> {
            tokio::time::sleep(self.delay).await;
            self.stream
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| io::Error::other("no stream"))
        }
        async fn bind(&self, _: &str) -> io::Result<Box<dyn LocalListener>> {
            unreachable!()
        }
        fn endpoint(&self, _: &str) -> io::Result<String> {
            unreachable!()
        }
        fn validate_endpoint(&self, _: &str, _: &str) -> io::Result<()> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn connection_and_total_request_deadlines_are_bounded() {
        let (client, mut peer) = tokio::io::duplex(256);
        let slow_connect = SimulatedTransport {
            delay: Duration::from_secs(60),
            stream: Mutex::new(Some(Box::new(client))),
        };
        let error = request_with_transport(
            &slow_connect,
            "mock",
            &ControlRequest::Targets,
            Duration::from_millis(10),
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(error.to_string(), "control connection timed out");

        let waiting_response = SimulatedTransport {
            delay: Duration::ZERO,
            stream: Mutex::new(slow_connect.stream.lock().unwrap().take()),
        };
        let error = request_with_transport(
            &waiting_response,
            "mock",
            &ControlRequest::Targets,
            Duration::from_secs(1),
            Duration::from_millis(10),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(error.to_string(), "control request timed out");
        // Cancelling the request drops its connected stream, rather than leaving
        // a forgotten client that can receive a late response.
        let _: ControlRequest = read_json(&mut peer, REQUEST_LIMIT).await.unwrap();
        assert_eq!(
            peer.read_u8().await.unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}
