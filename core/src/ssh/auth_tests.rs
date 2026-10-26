use std::collections::HashSet;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use russh::keys::ssh_key::{private::Ed25519Keypair, Cipher, Kdf, LineEnding};
use russh::keys::PrivateKey;
use serde_json::{json, Value};

use crate::commands::lifecycle::reserve_resource;
use crate::error::AppResult;
use crate::state::{AppState, SessionKind, SessionOwner};

use super::auth::decode_key_with_prompt;
use super::prompt::AuthCtx;
use super::test_support::headless_host;

const PASSPHRASE: &str = "correct test passphrase";
const CACHE_KEY: &str = "cred:test-encrypted-key";
const PROMPT: &str = "Enter passphrase for test key: ";
const TIMEOUT: Duration = Duration::from_secs(10);

fn test_key() -> PrivateKey {
    Ed25519Keypair::from_seed(&[7; 32]).into()
}

fn encrypted_key() -> PrivateKey {
    test_key()
        .encrypt_with(
            Cipher::Aes256Ctr,
            Kdf::Bcrypt {
                salt: vec![11; 16],
                rounds: 1,
            },
            0x1234_5678,
            PASSPHRASE,
        )
        .unwrap()
}

fn encrypted_pem() -> String {
    encrypted_key()
        .to_openssh(LineEnding::LF)
        .unwrap()
        .to_string()
}

fn cached_passphrase(state: &AppState) -> Option<String> {
    state
        .passphrase_cache
        .lock()
        .unwrap()
        .get(CACHE_KEY)
        .map(|value| value.as_str().to_owned())
}

fn terminal_feedback(event: (String, Value), session_id: &str) -> Vec<u8> {
    assert_eq!(event.0, format!("ssh:data:{session_id}"));
    let encoded = event
        .1
        .as_str()
        .expect("terminal byte events must contain a base64 string");
    STANDARD.decode(encoded).unwrap()
}

struct UnlockRun {
    result: AppResult<PrivateKey>,
    cache_at_prompts: Vec<Option<String>>,
    feedback: Vec<Vec<u8>>,
    cache_after: Option<String>,
}

async fn unlock(pem: &str, cached: Option<&str>, replies: &[Option<&str>]) -> UnlockRun {
    let (host, mut events) = headless_host();
    let state = host.state();
    let session_id = uuid::Uuid::new_v4().to_string();
    let owner = SessionOwner::Headless(uuid::Uuid::new_v4());
    let _reservation =
        reserve_resource(&state, &session_id, SessionKind::Ssh, owner.clone()).unwrap();
    if let Some(cached) = cached {
        state
            .passphrase_cache
            .lock()
            .unwrap()
            .insert(CACHE_KEY.into(), zeroize::Zeroizing::new(cached.to_owned()));
    }
    let ctx = AuthCtx {
        app: host.clone(),
        resource_id: session_id.clone(),
        prompt_id: session_id.clone(),
        owner: owner.clone(),
    };
    let prompt_event = format!("ssh:passphrase_prompt:{session_id}");
    let mut cache_at_prompts = Vec::new();
    let mut feedback = Vec::new();
    let mut prompt_nonces = HashSet::new();
    let decoding = decode_key_with_prompt(pem, Some(CACHE_KEY), PROMPT, Some(&ctx));
    tokio::pin!(decoding);

    let result = tokio::time::timeout(TIMEOUT, async {
        loop {
            tokio::select! {
                result = &mut decoding => break result,
                event = events.recv() => {
                    let event = event.expect("host event channel closed during key decoding");
                    if event.0 != prompt_event {
                        feedback.push(terminal_feedback(event, &session_id));
                        continue;
                    }
                    assert_eq!(event.1, json!({ "prompt": PROMPT }));
                    let reply = replies.get(cache_at_prompts.len()).expect("unexpected passphrase prompt");
                    cache_at_prompts.push(cached_passphrase(&state));
                    let waiter = state.passphrase_waiters.lock().unwrap()
                        .remove(&session_id).expect("prompt must register its waiter before emitting");
                    assert_eq!(waiter.owner, owner);
                    assert!(prompt_nonces.insert(waiter.nonce), "each prompt needs a fresh nonce");
                    if let Some(reply) = reply {
                        waiter.sender.send((*reply).to_owned()).unwrap();
                    }
                    // Dropping the sender models the user's cancel action.
                }
            }
        }
    })
    .await
    .expect("private-key prompt flow timed out");

    // The final error feedback can be queued in the same poll that completes decoding.
    while let Ok(event) = events.try_recv() {
        feedback.push(terminal_feedback(event, &session_id));
    }
    assert!(state.passphrase_waiters.lock().unwrap().is_empty());
    UnlockRun {
        result,
        cache_at_prompts,
        feedback,
        cache_after: cached_passphrase(&state),
    }
}

#[tokio::test]
async fn wrong_passphrase_can_retry_and_caches_only_the_successful_reply() {
    let run = unlock(
        &encrypted_pem(),
        None,
        &[Some("wrong passphrase"), Some(PASSPHRASE)],
    )
    .await;

    assert_eq!(run.result.unwrap().public_key(), test_key().public_key());
    assert_eq!(run.cache_at_prompts, vec![None, None]);
    assert_eq!(run.cache_after.as_deref(), Some(PASSPHRASE));
    assert_eq!(
        run.feedback,
        vec![b"\x1b[31mIncorrect passphrase, 2 attempt(s) left.\x1b[0m\r\n".to_vec()]
    );
}

#[tokio::test]
async fn stale_cached_passphrase_is_removed_before_prompting_and_replaced_on_success() {
    let run = unlock(
        &encrypted_pem(),
        Some("outdated passphrase"),
        &[Some(PASSPHRASE)],
    )
    .await;

    assert_eq!(run.result.unwrap().public_key(), test_key().public_key());
    assert_eq!(run.cache_at_prompts, vec![None]);
    assert_eq!(run.cache_after.as_deref(), Some(PASSPHRASE));
    assert!(run.feedback.is_empty());
}

#[tokio::test]
async fn three_wrong_passphrases_exhaust_retries_without_caching_or_leaking_waiters() {
    let run = unlock(&encrypted_pem(), None, &[Some("wrong passphrase"); 3]).await;

    assert_eq!(run.result.unwrap_err().code(), "ssh_passphrase_too_many");
    assert_eq!(run.cache_at_prompts, vec![None, None, None]);
    assert_eq!(run.cache_after, None);
    assert_eq!(
        run.feedback,
        vec![
            b"\x1b[31mIncorrect passphrase, 2 attempt(s) left.\x1b[0m\r\n".to_vec(),
            b"\x1b[31mIncorrect passphrase, 1 attempt(s) left.\x1b[0m\r\n".to_vec(),
            b"\x1b[31mIncorrect passphrase.\x1b[0m\r\n".to_vec(),
        ]
    );
}

#[tokio::test]
async fn correct_cached_passphrase_unlocks_without_prompting() {
    let run = unlock(&encrypted_pem(), Some(PASSPHRASE), &[]).await;

    assert_eq!(run.result.unwrap().public_key(), test_key().public_key());
    assert!(run.cache_at_prompts.is_empty());
    assert!(run.feedback.is_empty());
    assert_eq!(run.cache_after.as_deref(), Some(PASSPHRASE));
}

#[tokio::test]
async fn cancelling_a_passphrase_prompt_does_not_retry() {
    let run = unlock(&encrypted_pem(), None, &[None]).await;

    assert_eq!(
        run.result.unwrap_err().code(),
        "ssh_user_cancelled_passphrase"
    );
    assert_eq!(run.cache_at_prompts, vec![None]);
    assert!(run.feedback.is_empty());
    assert_eq!(run.cache_after, None);
}

#[tokio::test]
async fn encrypted_key_without_terminal_context_fails_without_waiting() {
    let pem = encrypted_pem();
    let result = tokio::time::timeout(TIMEOUT, decode_key_with_prompt(&pem, None, PROMPT, None))
        .await
        .expect("encrypted key without a terminal must not wait for a prompt");

    assert_eq!(result.unwrap_err().code(), "ssh_privkey_encrypted_no_ctx");
}

#[tokio::test]
async fn unencrypted_key_unlocks_without_prompting() {
    let pem = test_key().to_openssh(LineEnding::LF).unwrap();
    let run = unlock(&pem, None, &[]).await;

    assert_eq!(run.result.unwrap().public_key(), test_key().public_key());
    assert!(run.cache_at_prompts.is_empty());
    assert!(run.feedback.is_empty());
    assert_eq!(run.cache_after, None);
}

#[tokio::test]
async fn truncated_key_fails_parsing_without_prompting() {
    let key = test_key().to_bytes().unwrap();
    let pem = format!(
        "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
        STANDARD.encode(&key[..16]),
    );
    let run = unlock(&pem, None, &[]).await;

    assert_eq!(run.result.unwrap_err().code(), "ssh_privkey_parse_failed");
    assert!(run.cache_at_prompts.is_empty());
    assert!(run.feedback.is_empty());
    assert_eq!(run.cache_after, None);
}

#[tokio::test]
async fn malformed_encrypted_block_is_not_mistaken_for_an_incorrect_passphrase() {
    let key = encrypted_key();
    let ciphertext_len = key.key_data().encrypted().unwrap().len();
    let mut bytes = key.to_bytes().unwrap();
    // AES-CTR has no trailing authentication tag: shorten the final ciphertext
    // field and its length prefix so parsing reaches the block-size check.
    let length_offset = bytes.len() - ciphertext_len - 4;
    bytes[length_offset..length_offset + 4]
        .copy_from_slice(&((ciphertext_len - 1) as u32).to_be_bytes());
    bytes.pop();
    let pem = format!(
        "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
        STANDARD.encode(bytes.as_slice()),
    );
    assert!(matches!(
        russh::keys::decode_secret_key(&pem, None),
        Err(russh::keys::Error::SshKey(
            russh::keys::ssh_key::Error::Crypto
        ))
    ));

    let run = unlock(&pem, None, &[]).await;

    assert_eq!(run.result.unwrap_err().code(), "ssh_privkey_parse_failed");
    assert!(run.cache_at_prompts.is_empty());
    assert!(run.feedback.is_empty());
    assert_eq!(run.cache_after, None);
}
