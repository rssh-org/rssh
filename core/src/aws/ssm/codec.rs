//! Session Manager data-channel framing. Wire layout follows AWS's published
//! `session-manager-plugin/src/message/messageparser.go` (Apache-2.0).
//! In particular, UUIDs are transmitted least-significant 64-bit half first;
//! the four-byte payload length is *not* included in HeaderLength.

use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{protocol_error, AppResult};

pub(super) const OUTPUT: u32 = 1;
pub(super) const ERROR: u32 = 2;
pub(super) const SIZE: u32 = 3;
pub(super) const HANDSHAKE_REQUEST: u32 = 5;
pub(super) const HANDSHAKE_RESPONSE: u32 = 6;
pub(super) const HANDSHAKE_COMPLETE: u32 = 7;
pub(super) const CHALLENGE_REQUEST: u32 = 8;
pub(super) const CHALLENGE_RESPONSE: u32 = 9;
pub(super) const STDERR: u32 = 11;
pub(super) const EXIT_CODE: u32 = 12;
pub(super) const MAX_PAYLOAD: usize = 1024 * 1024;
const HEADER: usize = 116;
const PAYLOAD: usize = HEADER + 4;

#[derive(Clone, Debug)]
pub(super) struct Frame {
    pub message_type: String,
    pub sequence: i64,
    pub flags: u64,
    pub id: Uuid,
    pub payload_type: u32,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn input(sequence: i64, payload_type: u32, payload: Vec<u8>) -> Self {
        Self {
            message_type: "input_stream_data".into(),
            sequence,
            flags: 0,
            id: Uuid::new_v4(),
            payload_type,
            payload,
        }
    }

    pub fn acknowledgement(&self) -> Self {
        Self {
            message_type: "acknowledge".into(),
            sequence: 0,
            flags: 3,
            id: Uuid::new_v4(),
            payload_type: 0,
            payload: serde_json::to_vec(&serde_json::json!({
                "AcknowledgedMessageType": self.message_type,
                "AcknowledgedMessageId": self.id.to_string(),
                "AcknowledgedMessageSequenceNumber": self.sequence,
                "IsSequentialMessage": true,
            }))
            .expect("fixed JSON acknowledgement is serializable"),
        }
    }

    pub fn encode(&self) -> AppResult<Vec<u8>> {
        if self.payload.len() > MAX_PAYLOAD
            || self.message_type.is_empty()
            || self.message_type.len() > 32
            || !self.message_type.is_ascii()
        {
            return Err(protocol_error("invalid outgoing frame"));
        }
        let mut bytes = vec![0; PAYLOAD + self.payload.len()];
        bytes[0..4].copy_from_slice(&(HEADER as u32).to_be_bytes());
        bytes[4..36].fill(b' ');
        bytes[4..4 + self.message_type.len()].copy_from_slice(self.message_type.as_bytes());
        bytes[36..40].copy_from_slice(&1u32.to_be_bytes());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| protocol_error("system clock precedes epoch"))?
            .as_millis() as u64;
        bytes[40..48].copy_from_slice(&now.to_be_bytes());
        bytes[48..56].copy_from_slice(&self.sequence.to_be_bytes());
        bytes[56..64].copy_from_slice(&self.flags.to_be_bytes());
        bytes[64..72].copy_from_slice(&self.id.as_bytes()[8..16]);
        bytes[72..80].copy_from_slice(&self.id.as_bytes()[0..8]);
        bytes[80..112].copy_from_slice(&Sha256::digest(&self.payload));
        bytes[112..116].copy_from_slice(&self.payload_type.to_be_bytes());
        bytes[116..120].copy_from_slice(&(self.payload.len() as u32).to_be_bytes());
        bytes[120..].copy_from_slice(&self.payload);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> AppResult<Self> {
        if bytes.len() < PAYLOAD || bytes.len() > PAYLOAD + MAX_PAYLOAD {
            return Err(protocol_error("invalid frame length"));
        }
        let uint = |offset| u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap());
        if uint(0) != HEADER as u32 || uint(36) != 1 {
            return Err(protocol_error("unsupported frame schema"));
        }
        if uint(116) as usize != bytes.len() - PAYLOAD {
            return Err(protocol_error("payload length mismatch"));
        }
        let message_type = std::str::from_utf8(&bytes[4..36])
            .map_err(|_| protocol_error("invalid message type"))?
            .trim_matches([' ', '\0'])
            .to_owned();
        if message_type.is_empty() {
            return Err(protocol_error("missing message type"));
        }
        let payload = &bytes[PAYLOAD..];
        // AWS permits empty publication messages without a digest.
        if !payload.is_empty() && Sha256::digest(payload).as_slice() != &bytes[80..112] {
            return Err(protocol_error("payload digest mismatch"));
        }
        let mut id = [0; 16];
        id[..8].copy_from_slice(&bytes[72..80]);
        id[8..].copy_from_slice(&bytes[64..72]);
        Ok(Self {
            message_type,
            sequence: i64::from_be_bytes(bytes[48..56].try_into().unwrap()),
            flags: u64::from_be_bytes(bytes[56..64].try_into().unwrap()),
            id: Uuid::from_bytes(id),
            payload_type: uint(112),
            payload: payload.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_matches_aws_header_and_uuid_order() {
        let frame = Frame {
            message_type: "input_stream_data".into(),
            sequence: 17,
            flags: 0,
            id: Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").unwrap(),
            payload_type: OUTPUT,
            payload: b"hello".to_vec(),
        };
        let bytes = frame.encode().unwrap();
        // Independent offsets from the AWS Go messageparser contract.
        assert_eq!(&bytes[..4], &[0, 0, 0, 116]);
        assert_eq!(
            &bytes[64..80],
            &[
                0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55,
                0x66, 0x77,
            ]
        );
        assert_eq!(&bytes[116..], &[0, 0, 0, 5, b'h', b'e', b'l', b'l', b'o']);
        assert_eq!(
            &bytes[80..112],
            &[
                0x2c, 0xf2, 0x4d, 0xba, 0x5f, 0xb0, 0xa3, 0x0e, 0x26, 0xe8, 0x3b, 0x2a, 0xc5, 0xb9,
                0xe2, 0x9e, 0x1b, 0x16, 0x1e, 0x5c, 0x1f, 0xa7, 0x42, 0x5e, 0x73, 0x04, 0x33, 0x62,
                0x93, 0x8b, 0x98, 0x24,
            ]
        );
        let decoded = Frame::decode(&bytes).unwrap();
        assert_eq!(decoded.id, frame.id);
        assert_eq!(decoded.sequence, 17);
    }

    #[test]
    fn malformed_frames_are_rejected_without_panics() {
        let valid = Frame::input(0, OUTPUT, b"test".to_vec()).encode().unwrap();
        for n in 0..valid.len() {
            assert!(Frame::decode(&valid[..n]).is_err());
        }
        for offset in [0, 36, 80, 116, 120] {
            let mut malformed = valid.clone();
            malformed[offset] ^= 1;
            assert!(Frame::decode(&malformed).is_err());
        }
    }
}
