use aes_gcm::{aead::Aead, Aes256Gcm, KeyInit, Nonce};
use zeroize::Zeroizing;

use super::{protocol_error, AppResult};

/// AWS requests 64 bytes from KMS: the agent encrypts with the first 32 bytes,
/// the client with the second 32. Each payload is nonce || ciphertext || tag.
pub(super) struct Encryption {
    key: Zeroizing<Vec<u8>>,
}

impl Encryption {
    pub fn new(key: Vec<u8>) -> AppResult<Self> {
        let key = Zeroizing::new(key);
        if key.len() != 64 {
            return Err(protocol_error("KMS returned an invalid data-key length"));
        }
        Ok(Self { key })
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> AppResult<Vec<u8>> {
        let mut nonce = [0; 12];
        getrandom::getrandom(&mut nonce)
            .map_err(|_| protocol_error("secure randomness unavailable"))?;
        self.encrypt_with_nonce(plaintext, &nonce)
    }

    fn encrypt_with_nonce(&self, plaintext: &[u8], nonce: &[u8; 12]) -> AppResult<Vec<u8>> {
        let cipher = Aes256Gcm::new_from_slice(&self.key[32..])
            .map_err(|_| protocol_error("invalid encryption key"))?;
        let encrypted = cipher
            .encrypt(Nonce::from_slice(nonce), plaintext)
            .map_err(|_| protocol_error("payload encryption failed"))?;
        let mut result = nonce.to_vec();
        result.extend_from_slice(&encrypted);
        Ok(result)
    }

    pub fn decrypt(&self, ciphertext: &[u8]) -> AppResult<Vec<u8>> {
        if ciphertext.len() < 12 + 16 {
            return Err(protocol_error("encrypted payload is truncated"));
        }
        let cipher = Aes256Gcm::new_from_slice(&self.key[..32])
            .map_err(|_| protocol_error("invalid decryption key"))?;
        cipher
            .decrypt(Nonce::from_slice(&ciphertext[..12]), &ciphertext[12..])
            .map_err(|_| protocol_error("payload authentication failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_opposite_direction_keys_and_nonce_prefix() {
        let key: Vec<u8> = (0..64).collect();
        let client = Encryption::new(key.clone()).unwrap();
        let agent = Encryption::new([&key[32..], &key[..32]].concat()).unwrap();
        let encrypted = client
            .encrypt_with_nonce(b"test challenge", &[7; 12])
            .unwrap();
        assert_eq!(&encrypted[..12], &[7; 12]);
        assert_eq!(agent.decrypt(&encrypted).unwrap(), b"test challenge");
        assert!(client.decrypt(&encrypted).is_err());
        assert!(client.decrypt(&[0; 27]).is_err());
        let mut changed = encrypted;
        *changed.last_mut().unwrap() ^= 1;
        assert!(agent.decrypt(&changed).is_err());
    }
}
