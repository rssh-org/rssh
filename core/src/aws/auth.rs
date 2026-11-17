//! AWS Sign-In's public devtools/cross-device flow. The browser handles console
//! passwords and MFA; RSSH receives only the user-pasted authorization response.
//!
//! Wire contract: aws/aws-cli awscli/customizations/login/utils.py and its
//! botocore Signin 2023-01-01 model. JSON requests use camelCase, the PKCE method
//! is `SHA-256`, and token exchange/refresh bind to a P-256 DPoP key.
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use super::api::{self, failure, invalid_response};
use crate::error::{AppError, AppResult};
use crate::secret::SecretStore;
use crate::state::SessionOwner;

const CLIENT_ID: &str = "arn:aws:signin:::devtools/cross-device";
const LOGIN_LIFETIME: u64 = 15 * 60;
const RESPONSE_LIMIT: usize = 256 * 1024;
const REFRESH_MARGIN: u64 = 60;

pub fn credential_key(source_id: &str) -> String {
    format!("aws_native:{source_id}:tokens")
}
fn login_key(source_id: &str) -> String {
    format!("aws_native:{source_id}:login")
}

fn validate_source(source_id: &str) -> AppResult<()> {
    if uuid::Uuid::parse_str(source_id).is_err() {
        return Err(failure("aws_source_invalid"));
    }
    Ok(())
}

fn owner_key(owner: &SessionOwner) -> String {
    match owner {
        SessionOwner::Window(id) => format!("window:{id}"),
        SessionOwner::Headless(id) => format!("headless:{id}"),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn open_lock(data_dir: &Path) -> AppResult<File> {
    std::fs::create_dir_all(data_dir)?;
    Ok(OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(data_dir.join("aws-auth.lock"))?)
}

fn source_revision(data_dir: &Path, source_id: &str) -> AppResult<Option<String>> {
    validate_source(source_id)?;
    let path = data_dir.join("aws-auth-revisions").join(source_id);
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut value = String::new();
    file.take(64).read_to_string(&mut value)?;
    uuid::Uuid::parse_str(&value).map_err(|_| invalid_response("source_revision"))?;
    Ok(Some(value))
}

// Only a random revision identifier is persisted here, never OAuth material.
// All reads/writes are serialized by aws-auth.lock, including other processes.
fn advance_revision(data_dir: &Path, source_id: &str) -> AppResult<String> {
    validate_source(source_id)?;
    let dir = data_dir.join("aws-auth-revisions");
    std::fs::create_dir_all(&dir)?;
    let value = uuid::Uuid::new_v4().to_string();
    let temporary = dir.join(format!(".{source_id}.{value}.tmp"));
    let result = (|| -> AppResult<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(value.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, dir.join(source_id))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result?;
    Ok(value)
}

/// Call while holding the auth file lock. Invalidate old drafts before deleting
/// the persisted grant, so another process cannot later restore it from memory.
pub(crate) fn clear_credentials(
    secrets: &dyn SecretStore,
    data_dir: &Path,
    source_id: &str,
) -> AppResult<()> {
    advance_revision(data_dir, source_id)?;
    // Remove metadata left by earlier versions; new drafts never use SecretStore.
    secrets.delete(&login_key(source_id))?;
    secrets.delete(&credential_key(source_id))
}

pub(crate) fn with_auth_lock<T>(
    data_dir: &Path,
    update: impl FnOnce() -> AppResult<T>,
) -> AppResult<T> {
    let lock = open_lock(data_dir)?;
    lock.lock()?;
    update()
}

/// Share the refresh/login lock with DB source removal. Call on a blocking
/// thread and before taking the DB mutex, never while holding it.
pub fn with_removed_credentials<T>(
    secrets: &dyn SecretStore,
    data_dir: &Path,
    source_ids: &[&str],
    update: impl FnOnce() -> AppResult<T>,
) -> AppResult<T> {
    if source_ids.is_empty() {
        return update();
    }
    with_auth_lock(data_dir, || {
        for source_id in source_ids {
            clear_credentials(secrets, data_dir, source_id)?;
        }
        update()
    })
}

#[derive(Serialize)]
pub struct AuthStatus {
    pub authenticated: bool,
    pub account_id: Option<String>,
    pub arn: Option<String>,
}

#[derive(Serialize)]
pub struct LoginStart {
    pub login_id: String,
    pub authorization_url: String,
    pub expires_in: u64,
}

/// Deliberately no Debug or Serialize: transport credentials must not cross IPC.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub account_id: String,
    pub arn: String,
    pub expires_at: u64,
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct StoredCredentials {
    access_key_id: String,
    secret_access_key: String,
    session_token: String,
    account_id: String,
    arn: String,
    expires_at: u64,
    refresh_token: String,
    dpop_key: String,
    signin_region: String,
}

impl StoredCredentials {
    fn credentials(&self) -> AwsCredentials {
        AwsCredentials {
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.clone(),
            session_token: self.session_token.clone(),
            account_id: self.account_id.clone(),
            arn: self.arn.clone(),
            expires_at: self.expires_at,
        }
    }
    fn status(&self) -> AuthStatus {
        AuthStatus {
            authenticated: true,
            account_id: Some(self.account_id.clone()),
            arn: Some(self.arn.clone()),
        }
    }
}

#[derive(Clone)]
struct PendingLogin {
    login_id: String,
    owner: String,
    owner_epoch: u64,
    owner_generation: Arc<AtomicU64>,
    region: String,
    state: String,
    verifier: Zeroizing<String>,
    key: SigningKey,
    expires_at: Instant,
    revision: Option<String>,
    ready: Option<StoredCredentials>,
    completing: bool,
}

type CancelKey = (String, String, String);

#[derive(Clone)]
pub struct AwsAuth {
    /// Session opens share this gate; source removal/logout takes it exclusively
    /// so a new remote session cannot appear between cleanup and grant deletion.
    pub(crate) source_gate: Arc<tokio::sync::RwLock<()>>,
    secrets: Arc<dyn SecretStore>,
    data_dir: PathBuf,
    http: reqwest::Client,
    pending: Arc<Mutex<HashMap<String, PendingLogin>>>,
    cancelled: Arc<std::sync::Mutex<HashSet<CancelKey>>>,
    owner_epochs: Arc<std::sync::Mutex<HashMap<String, Arc<AtomicU64>>>>,
    #[cfg(test)]
    endpoint_override: Option<String>,
    #[cfg(test)]
    identity_endpoint_override: Option<String>,
}

impl AwsAuth {
    pub fn new(secrets: Arc<dyn SecretStore>, data_dir: PathBuf) -> AppResult<Self> {
        Ok(Self {
            source_gate: Arc::new(tokio::sync::RwLock::new(())),
            secrets,
            data_dir,
            http: api::client()?,
            pending: Arc::new(Mutex::new(HashMap::new())),
            cancelled: Arc::new(std::sync::Mutex::new(HashSet::new())),
            owner_epochs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            #[cfg(test)]
            endpoint_override: None,
            #[cfg(test)]
            identity_endpoint_override: None,
        })
    }

    fn base_url(&self, region: &str) -> AppResult<String> {
        api::validate_region(region)?;
        #[cfg(test)]
        if let Some(url) = &self.endpoint_override {
            return Ok(url.clone());
        }
        let domain = if region.starts_with("cn-") {
            "signin.amazonaws.cn"
        } else if region.starts_with("us-gov-") {
            "signin.amazonaws-us-gov.com"
        } else {
            "signin.aws.amazon.com"
        };
        Ok(format!("https://{region}.{domain}"))
    }

    // This map mutex protects only a short lookup, never keychain or disk I/O.
    fn owner_generation(&self, owner: &str) -> AppResult<Arc<AtomicU64>> {
        Ok(self
            .owner_epochs
            .lock()
            .map_err(|_| AppError::Lock)?
            .entry(owner.into())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .clone())
    }

    async fn blocking<T: Send + 'static>(
        &self,
        operation: impl FnOnce(Self) -> AppResult<T> + Send + 'static,
    ) -> AppResult<T> {
        let auth = self.clone();
        tokio::task::spawn_blocking(move || operation(auth))
            .await
            .map_err(|_| failure("aws_auth_request_failed"))?
    }

    async fn lock_file(&self) -> AppResult<File> {
        self.blocking(|auth| {
            let file = open_lock(&auth.data_dir)?;
            let deadline = Instant::now() + Duration::from_secs(70);
            loop {
                match file.try_lock() {
                    Ok(()) => return Ok(file),
                    Err(error) => {
                        let error: std::io::Error = error.into();
                        if error.kind() != std::io::ErrorKind::WouldBlock {
                            return Err(error.into());
                        }
                    }
                }
                if Instant::now() >= deadline {
                    return Err(failure("aws_auth_busy"));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        })
        .await
    }

    fn load(&self, source_id: &str) -> AppResult<StoredCredentials> {
        let raw = Zeroizing::new(
            self.secrets
                .get(&credential_key(source_id))?
                .ok_or_else(|| failure("aws_login_required"))?,
        );
        serde_json::from_str(&raw).map_err(|_| invalid_response("stored_credentials_json"))
    }

    fn save(&self, source_id: &str, credentials: &StoredCredentials) -> AppResult<()> {
        let raw = Zeroizing::new(
            serde_json::to_string(credentials)
                .map_err(|_| invalid_response("credentials_serialize"))?,
        );
        self.secrets.set(&credential_key(source_id), &raw)
    }

    pub fn status(&self, source_id: &str) -> AppResult<AuthStatus> {
        validate_source(source_id)?;
        if !self.secrets.exists(&credential_key(source_id))? {
            return Ok(AuthStatus {
                authenticated: false,
                account_id: None,
                arn: None,
            });
        }
        // A cached refresh grant counts as signed in; credentials() refreshes
        // it on first use and removes an explicitly rejected grant.
        Ok(self.load(source_id)?.status())
    }

    pub async fn status_async(&self, source_id: &str) -> AppResult<AuthStatus> {
        let source_id = source_id.to_owned();
        self.blocking(move |auth| auth.status(&source_id)).await
    }

    pub async fn start(
        &self,
        source_id: &str,
        region: &str,
        owner: &SessionOwner,
    ) -> AppResult<LoginStart> {
        validate_source(source_id)?;
        let owner = owner_key(owner);
        let generation = self.owner_generation(&owner)?;
        let epoch = generation.load(Ordering::SeqCst);
        let auth = self.clone();
        let source_id = source_id.to_owned();
        let region = region.to_owned();
        // Capture the owner's generation before spawning: teardown must also
        // invalidate a worker that has not started running yet.
        tokio::spawn(async move {
            auth.start_inner(&source_id, &region, owner, generation, epoch)
                .await
        })
        .await
        .map_err(|_| failure("aws_auth_request_failed"))?
    }

    async fn start_inner(
        &self,
        source_id: &str,
        region: &str,
        owner: String,
        owner_generation: Arc<AtomicU64>,
        owner_epoch: u64,
    ) -> AppResult<LoginStart> {
        let base = self.base_url(region)?;
        let mut pending = self.pending.clone().lock_owned().await;
        let file_lock = self.lock_file().await?;
        pending.retain(|_, entry| entry.expires_at > Instant::now());
        let login_id = uuid::Uuid::new_v4().to_string();
        let state = uuid::Uuid::new_v4().to_string();
        let mut bytes = Zeroizing::new([0u8; 48]);
        getrandom::getrandom(&mut *bytes).map_err(|_| failure("aws_auth_request_failed"))?;
        let verifier = Zeroizing::new(URL_SAFE_NO_PAD.encode(&*bytes));
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let key = SigningKey::random(&mut p256::elliptic_curve::rand_core::OsRng);
        let mut url = url::Url::parse(&format!("{base}/v1/authorize"))
            .map_err(|_| failure("aws_auth_request_failed"))?;
        url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("state", state.as_str()),
            ("code_challenge_method", "SHA-256"),
            ("scope", "openid"),
            (
                "redirect_uri",
                format!("{base}/v1/sessions/confirmation").as_str(),
            ),
            ("code_challenge", challenge.as_str()),
        ]);
        let source_id = source_id.to_owned();
        let region = region.to_owned();
        self.blocking(move |auth| {
            let _file_lock = file_lock;
            let revision = source_revision(&auth.data_dir, &source_id)?;
            if owner_generation.load(Ordering::SeqCst) != owner_epoch {
                return Err(failure("aws_login_cancelled"));
            }
            if pending
                .get(&source_id)
                .is_some_and(|draft| draft.owner != owner)
            {
                return Err(failure("aws_auth_busy"));
            }
            pending.insert(
                source_id,
                PendingLogin {
                    login_id: login_id.clone(),
                    owner,
                    owner_epoch,
                    owner_generation,
                    region,
                    state,
                    verifier,
                    key,
                    expires_at: Instant::now() + Duration::from_secs(LOGIN_LIFETIME),
                    revision,
                    ready: None,
                    completing: false,
                },
            );
            Ok(LoginStart {
                login_id,
                authorization_url: url.into(),
                expires_in: LOGIN_LIFETIME,
            })
        })
        .await
    }

    pub async fn complete(
        &self,
        source_id: &str,
        login_id: &str,
        verification_code: &str,
        owner: &SessionOwner,
    ) -> AppResult<AuthStatus> {
        validate_source(source_id)?;
        let auth = self.clone();
        let source_id = source_id.to_owned();
        let login_id = login_id.to_owned();
        let code = Zeroizing::new(verification_code.to_owned());
        let owner = owner.clone();
        tokio::spawn(async move {
            auth.complete_inner(&source_id, &login_id, &code, &owner)
                .await
        })
        .await
        .map_err(|_| failure("aws_auth_request_failed"))?
    }

    fn check_attempt(&self, source_id: &str, attempt: &PendingLogin) -> AppResult<()> {
        if attempt.owner_generation.load(Ordering::SeqCst) != attempt.owner_epoch
            || self
                .cancelled
                .lock()
                .map_err(|_| AppError::Lock)?
                .contains(&(
                    source_id.to_owned(),
                    attempt.login_id.clone(),
                    attempt.owner.clone(),
                ))
        {
            return Err(failure("aws_login_cancelled"));
        }
        if Instant::now() >= attempt.expires_at {
            return Err(failure("aws_login_expired"));
        }
        Ok(())
    }

    async fn check_revision(&self, source_id: &str, expected: Option<String>) -> AppResult<()> {
        let file_lock = self.lock_file().await?;
        let source_id = source_id.to_owned();
        self.blocking(move |auth| {
            let _file_lock = file_lock;
            if source_revision(&auth.data_dir, &source_id)? != expected {
                return Err(failure("aws_login_cancelled"));
            }
            Ok(())
        })
        .await
    }

    async fn complete_inner(
        &self,
        source_id: &str,
        login_id: &str,
        verification_code: &str,
        owner: &SessionOwner,
    ) -> AppResult<AuthStatus> {
        let owner = owner_key(owner);
        let attempt = {
            let mut pending = self.pending.lock().await;
            let attempt = pending
                .get_mut(source_id)
                .filter(|p| p.login_id == login_id)
                .ok_or_else(|| failure("aws_login_cancelled"))?;
            if attempt.owner != owner {
                return Err(failure("aws_login_owner_mismatch"));
            }
            self.check_attempt(source_id, attempt)?;
            if attempt.completing {
                return Err(failure("aws_auth_busy"));
            }
            attempt.completing = true;
            attempt.clone()
        };
        // Neither the in-process draft map nor the cross-process auth lock is
        // held over network I/O. Other sources can refresh while a user logs in.
        let result = async {
            self.check_revision(source_id, attempt.revision.clone()).await?;
            let credentials = if let Some(ready) = &attempt.ready {
                ready.clone()
            } else {
                let code = parse_verification_code(verification_code, &attempt.state)?;
                let base = self.base_url(&attempt.region)?;
                let body = self.token_request(&base, &attempt.key, &json!({
                    "clientId":CLIENT_ID,"grantType":"authorization_code","code":&*code,
                    "codeVerifier":&*attempt.verifier,"redirectUri":format!("{base}/v1/sessions/confirmation"),
                })).await?;
                // AWS CLI treats this as metadata, not a closed enum.
                required_string(&body, "tokenType")?;
                let mut credentials = parse_credentials(&body, &attempt.region, &attempt.key)?;
                // JWT claims are not trusted identity evidence. Confirm the
                // actual SigV4 credentials before making the draft saveable.
                let identity = self.confirm_identity(&credentials.credentials(), &attempt.region).await?;
                credentials.account_id = identity.account;
                credentials.arn = identity.arn;
                credentials
            };
            let mut pending = self.pending.clone().lock_owned().await;
            let file_lock = self.lock_file().await?;
            let source_id = source_id.to_owned();
            let login_id = login_id.to_owned();
            self.blocking(move |auth| {
                let _file_lock = file_lock;
                let current = pending.get_mut(&source_id)
                    .filter(|p| p.login_id == login_id && p.owner == owner)
                    .ok_or_else(|| failure("aws_login_cancelled"))?;
                auth.check_attempt(&source_id, current)?;
                if source_revision(&auth.data_dir, &source_id)? != current.revision {
                    return Err(failure("aws_login_cancelled"));
                }
                let status = credentials.status();
                current.ready = Some(credentials);
                current.completing = false;
                Ok(status)
            }).await
        }.await;
        if result.is_err() {
            if let Some(current) = self
                .pending
                .lock()
                .await
                .get_mut(source_id)
                .filter(|p| p.login_id == login_id)
            {
                current.completing = false;
            }
        }
        result
    }

    /// Save a ready draft and its source configuration as one serialized local
    /// operation. The callback must not reacquire the auth lock. It must return
    /// an error without committing configuration if its own transaction fails.
    pub async fn commit_with<T, F>(
        &self,
        source_id: &str,
        login_id: &str,
        owner: &SessionOwner,
        update: F,
    ) -> AppResult<T>
    where
        T: Send + 'static,
        F: FnOnce() -> AppResult<T> + Send + 'static,
    {
        self.commit_result(source_id, login_id, owner, update)
            .await
            .map(|(value, _)| value)
    }

    pub async fn commit(
        &self,
        source_id: &str,
        login_id: &str,
        owner: &SessionOwner,
    ) -> AppResult<AuthStatus> {
        self.commit_result(source_id, login_id, owner, || Ok(()))
            .await
            .map(|(_, status)| status)
    }

    async fn commit_result<T, F>(
        &self,
        source_id: &str,
        login_id: &str,
        owner: &SessionOwner,
        update: F,
    ) -> AppResult<(T, AuthStatus)>
    where
        T: Send + 'static,
        F: FnOnce() -> AppResult<T> + Send + 'static,
    {
        validate_source(source_id)?;
        let auth = self.clone();
        let source_id = source_id.to_owned();
        let login_id = login_id.to_owned();
        let owner = owner_key(owner);
        // Dropping the IPC future cannot release either lock while a keychain
        // write or the configuration transaction is still running.
        tokio::spawn(async move {
            let mut pending = auth.pending.clone().lock_owned().await;
            let file_lock = auth.lock_file().await?;
            auth.blocking(move |auth| {
                let _file_lock = file_lock;
                let attempt = pending
                    .get_mut(&source_id)
                    .filter(|p| p.login_id == login_id)
                    .ok_or_else(|| failure("aws_login_cancelled"))?;
                if attempt.owner != owner {
                    return Err(failure("aws_login_owner_mismatch"));
                }
                auth.check_attempt(&source_id, attempt)?;
                if source_revision(&auth.data_dir, &source_id)? != attempt.revision {
                    return Err(failure("aws_login_cancelled"));
                }
                let credentials = attempt
                    .ready
                    .as_ref()
                    .ok_or_else(|| failure("aws_auth_busy"))?;
                let status = credentials.status();
                let previous = auth
                    .secrets
                    .get(&credential_key(&source_id))?
                    .map(Zeroizing::new);
                // All fallible revision I/O precedes the configuration write.
                // On failure, only this draft can retry against the new revision.
                attempt.revision = Some(advance_revision(&auth.data_dir, &source_id)?);
                let committed = (|| -> AppResult<T> {
                    auth.save(&source_id, credentials)?;
                    // Observe teardown/cancel even while SecretStore was blocked.
                    auth.check_attempt(&source_id, attempt)?;
                    update()
                })();
                match committed {
                    Ok(value) => {
                        // Save has committed. A later cancel only discards a
                        // draft and must never delete these saved credentials.
                        pending.remove(&source_id);
                        Ok((value, status))
                    }
                    Err(error) => {
                        let rollback = if let Some(previous) = previous {
                            auth.secrets.set(&credential_key(&source_id), &previous)
                        } else {
                            auth.secrets.delete(&credential_key(&source_id))
                        };
                        if let Err(rollback_error) = rollback {
                            // Persisted state is uncertain. Do not allow this
                            // draft to overwrite it again on a blind retry.
                            pending.remove(&source_id);
                            return Err(rollback_error);
                        }
                        Err(error)
                    }
                }
            })
            .await
        })
        .await
        .map_err(|_| failure("aws_auth_request_failed"))?
    }

    pub async fn cancel(
        &self,
        source_id: &str,
        login_id: &str,
        owner: &SessionOwner,
    ) -> AppResult<()> {
        validate_source(source_id)?;
        let owner = owner_key(owner);
        let key = (source_id.to_owned(), login_id.to_owned(), owner.clone());
        self.cancelled
            .lock()
            .map_err(|_| AppError::Lock)?
            .insert(key.clone());
        let auth = self.clone();
        let source_id = source_id.to_owned();
        let login_id = login_id.to_owned();
        tokio::spawn(async move { auth.cancel_inner(&source_id, &login_id, owner, key).await })
            .await
            .map_err(|_| failure("aws_auth_request_failed"))?
    }

    async fn cancel_inner(
        &self,
        source_id: &str,
        login_id: &str,
        owner: String,
        key: CancelKey,
    ) -> AppResult<()> {
        let result = async {
            let mut pending = self.pending.lock().await;
            if let Some(attempt) = pending.get(source_id).filter(|p| p.login_id == login_id) {
                if attempt.owner != owner {
                    return Err(failure("aws_login_owner_mismatch"));
                }
                pending.remove(source_id);
            }
            Ok(())
        }
        .await;
        self.cancelled
            .lock()
            .map_err(|_| AppError::Lock)?
            .remove(&key);
        result
    }

    pub async fn logout(&self, source_id: &str) -> AppResult<()> {
        validate_source(source_id)?;
        let auth = self.clone();
        let source_id = source_id.to_owned();
        tokio::spawn(async move {
            let mut pending = auth.pending.clone().lock_owned().await;
            let file_lock = auth.lock_file().await?;
            auth.blocking(move |auth| {
                let _file_lock = file_lock;
                clear_credentials(auth.secrets.as_ref(), &auth.data_dir, &source_id)?;
                pending.remove(&source_id);
                Ok(())
            })
            .await
        })
        .await
        .map_err(|_| failure("aws_auth_request_failed"))?
    }

    /// Host teardown discards all unfinished and ready drafts for that host.
    /// Explicitly committed grants remain device-local account settings.
    pub async fn cancel_owner(&self, owner: &SessionOwner) -> AppResult<()> {
        let owner_id = owner_key(owner);
        let epoch = self
            .owner_generation(&owner_id)?
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        let auth = self.clone();
        let owner = owner.clone();
        tokio::spawn(async move { auth.cancel_owner_inner(owner_id, epoch, &owner).await })
            .await
            .map_err(|_| failure("aws_auth_request_failed"))?
    }

    async fn cancel_owner_inner(
        &self,
        owner_id: String,
        epoch: u64,
        owner: &SessionOwner,
    ) -> AppResult<()> {
        let attempts: Vec<_> = self
            .pending
            .lock()
            .await
            .iter()
            .filter(|(_, pending)| pending.owner == owner_id && pending.owner_epoch < epoch)
            .map(|(source, pending)| (source.clone(), pending.login_id.clone()))
            .collect();
        for (source, login) in attempts {
            self.cancel(&source, &login, owner).await?;
        }
        Ok(())
    }

    /// An in-flight refresh survives cancellation of the caller so a rotating
    /// refresh token is always persisted before releasing the process lock.
    pub async fn credentials(&self, source_id: &str) -> AppResult<AwsCredentials> {
        validate_source(source_id)?;
        let auth = self.clone();
        let source_id = source_id.to_owned();
        tokio::spawn(async move { auth.credentials_locked(&source_id).await })
            .await
            .map_err(|_| failure("aws_auth_request_failed"))?
    }

    async fn credentials_locked(&self, source_id: &str) -> AppResult<AwsCredentials> {
        let _pending = self.pending.lock().await;
        let _lock = self.lock_file().await?;
        let load_source = source_id.to_owned();
        let current = self.blocking(move |auth| auth.load(&load_source)).await?;
        if current.expires_at > now().saturating_add(REFRESH_MARGIN) {
            return Ok(current.credentials());
        }
        let bytes = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(&current.dpop_key)
                .map_err(|_| invalid_response("stored_dpop_base64"))?,
        );
        let key =
            SigningKey::from_slice(&bytes).map_err(|_| invalid_response("stored_dpop_key"))?;
        let base = self.base_url(&current.signin_region)?;
        let body = match self.token_request(&base, &key, &json!({"clientId":CLIENT_ID,"grantType":"refresh_token","refreshToken":current.refresh_token})).await {
            Ok(body) => body,
            Err(error) => {
                if error.code() == "aws_login_required" {
                    let source_id=source_id.to_owned();
                    self.blocking(move |auth|auth.secrets.delete(&credential_key(&source_id))).await?;
                }
                return Err(error);
            }
        };
        let mut renewed = parse_credentials(&body, &current.signin_region, &key)?;
        // A refresh belongs to the same AWS Sign-In grant. Reuse identity that
        // was confirmed by STS, and persist rotation before any later API call.
        renewed.account_id = current.account_id.clone();
        renewed.arn = current.arn.clone();
        let source_id = source_id.to_owned();
        self.blocking(move |auth| {
            auth.save(&source_id, &renewed)?;
            Ok(renewed.credentials())
        })
        .await
    }

    async fn token_request(&self, base: &str, key: &SigningKey, body: &Value) -> AppResult<Value> {
        let uri = format!("{base}/v1/token");
        let proof = Zeroizing::new(dpop_proof(
            key,
            &uri,
            now(),
            &uuid::Uuid::new_v4().to_string(),
        )?);
        let response = self
            .http
            .post(&uri)
            .header("DPoP", proof.as_str())
            .json(body)
            .send()
            .await
            .map_err(|_| failure("aws_auth_request_failed"))?;
        let status = response.status();
        let bytes = Zeroizing::new(api::response_bytes(response, RESPONSE_LIMIT).await?);
        let body: Value = serde_json::from_slice(&bytes).map_err(|_| {
            log::warn!(
                "AWS token response is not JSON: http_status={}, response_bytes={}",
                status.as_u16(),
                bytes.len()
            );
            invalid_response("token_json")
        })?;
        if !status.is_success() {
            return Err(failure(match body.get("error").and_then(Value::as_str) {
                Some("TOKEN_EXPIRED" | "USER_CREDENTIALS_CHANGED" | "invalid_grant") => {
                    "aws_login_required"
                }
                Some("AUTHCODE_EXPIRED") => "aws_login_expired",
                Some("INSUFFICIENT_PERMISSIONS") => "aws_auth_access_denied",
                _ if status.as_u16() == 403 => "aws_auth_access_denied",
                _ => "aws_auth_request_failed",
            }));
        }
        Ok(body)
    }

    async fn confirm_identity(
        &self,
        credentials: &AwsCredentials,
        region: &str,
    ) -> AppResult<api::CallerIdentity> {
        #[cfg(test)]
        if let Some(url) = &self.identity_endpoint_override {
            return api::caller_identity_at(credentials, region, url).await;
        }
        api::caller_identity(credentials, region).await
    }
}

fn parse_verification_code(input: &str, expected_state: &str) -> AppResult<Zeroizing<String>> {
    let rejected = |code: &'static str, stage: &'static str| {
        // Only fixed stage names and lengths are diagnostic data. Never log
        // the response, decoded authorization code, or either state value.
        log::warn!(
            "AWS authorization response rejected: stage={stage}, input_bytes={}",
            input.len()
        );
        failure(code)
    };
    if input.len() > 16 * 1024 {
        return Err(rejected("aws_login_code_length", "response_length"));
    }
    // AWS CLI's base64.b64decode accepts whitespace inside a copied response.
    // Preserve that behavior without accepting arbitrary non-base64 content.
    let normalized = Zeroizing::new(
        input
            .trim()
            .bytes()
            .filter(|b| !b.is_ascii_whitespace())
            .collect::<Vec<_>>(),
    );
    let decoded = Zeroizing::new(
        STANDARD
            .decode(normalized.as_slice())
            .map_err(|_| rejected("aws_login_code_invalid", "base64"))?,
    );
    let text =
        std::str::from_utf8(&decoded).map_err(|_| rejected("aws_login_code_invalid", "utf8"))?;
    let mut state = None;
    let mut code = None;
    for (key, value) in url::form_urlencoded::parse(text.as_bytes()) {
        match key.as_ref() {
            "state" if state.is_none() => state = Some(value.into_owned()),
            "code" if code.is_none() => code = Some(Zeroizing::new(value.into_owned())),
            "state" | "code" => return Err(rejected("aws_login_code_fields", "duplicate_field")),
            _ => {}
        }
    }
    let state = state
        .filter(|s| !s.is_empty())
        .ok_or_else(|| rejected("aws_login_code_fields", "missing_state"))?;
    let code = code
        .filter(|c| !c.is_empty())
        .ok_or_else(|| rejected("aws_login_code_fields", "missing_code"))?;
    if state != expected_state {
        return Err(rejected("aws_login_state_mismatch", "state_mismatch"));
    }
    // Treat the code as opaque. AWS can issue codes longer than the service
    // model's max=512, which AWS CLI's botocore validator does not enforce.
    // The total response size is bounded above; AWS validates the code itself.
    Ok(code)
}

fn dpop_proof(key: &SigningKey, uri: &str, timestamp: u64, id: &str) -> AppResult<String> {
    let point = key.verifying_key().to_encoded_point(false);
    let x = point
        .x()
        .ok_or_else(|| failure("aws_auth_request_failed"))?;
    let y = point
        .y()
        .ok_or_else(|| failure("aws_auth_request_failed"))?;
    let header = json!({"typ":"dpop+jwt","alg":"ES256","jwk":{"kty":"EC","crv":"P-256","x":URL_SAFE_NO_PAD.encode(x),"y":URL_SAFE_NO_PAD.encode(y)}});
    let payload = json!({"htm":"POST","htu":uri,"iat":timestamp,"jti":id});
    let message = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        URL_SAFE_NO_PAD.encode(payload.to_string())
    );
    let signature: Signature = key.sign(message.as_bytes());
    Ok(format!(
        "{message}.{}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    ))
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null_or_missing",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn required_string(body: &Value, key: &'static str) -> AppResult<String> {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 64 * 1024)
        .map(str::to_owned)
        .ok_or_else(|| {
            // Field names are static call-site literals. Never log field values.
            log::warn!(
                "AWS token field rejected: field={key}, kind={}, string_bytes={:?}",
                json_kind(&body[key]),
                body[key].as_str().map(str::len)
            );
            invalid_response(key)
        })
}

fn parse_credentials(body: &Value, region: &str, key: &SigningKey) -> AppResult<StoredCredentials> {
    // Like AWS CLI's refresh provider, consume the credential object rather than
    // interpreting tokenType. Initial grants are also verified with signed STS.
    let expires_in = body
        .get("expiresIn")
        .and_then(token_lifetime)
        .ok_or_else(|| {
            // Expiry is protocol metadata, never a token or identity value.
            log::warn!(
                "AWS token expiry rejected: kind={}, seconds={:?}",
                json_kind(&body["expiresIn"]),
                body["expiresIn"].as_f64()
            );
            invalid_response("token_expires_in")
        })?;
    let expires_at = now()
        .checked_add(expires_in)
        .ok_or_else(|| invalid_response("token_expiry_overflow"))?;
    let token = &body["accessToken"];
    Ok(StoredCredentials {
        access_key_id: required_string(token, "accessKeyId")?,
        secret_access_key: required_string(token, "secretAccessKey")?,
        session_token: required_string(token, "sessionToken")?,
        account_id: String::new(),
        arn: String::new(),
        expires_at,
        refresh_token: required_string(body, "refreshToken")?,
        dpop_key: URL_SAFE_NO_PAD.encode(key.to_bytes()),
        signin_region: region.into(),
    })
}

fn token_lifetime(value: &Value) -> Option<u64> {
    // Botocore converts integer shapes with int(value). Accept equivalent JSON
    // representations, while rejecting fractional seconds and overflow.
    let seconds = match value {
        Value::Number(number) => number.as_u64().or_else(|| {
            number
                .as_f64()
                .filter(|n| n.is_finite() && *n > 0.0 && n.fract() == 0.0 && *n < u64::MAX as f64)
                .map(|n| n as u64)
        }),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    }?;
    (seconds > 0).then_some(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Verifier;

    #[derive(Default)]
    struct MemorySecrets(std::sync::Mutex<HashMap<String, String>>);
    impl SecretStore for MemorySecrets {
        fn get(&self, k: &str) -> AppResult<Option<String>> {
            Ok(self.0.lock().unwrap().get(k).cloned())
        }
        fn set(&self, k: &str, v: &str) -> AppResult<()> {
            self.0.lock().unwrap().insert(k.into(), v.into());
            Ok(())
        }
        fn delete(&self, k: &str) -> AppResult<()> {
            self.0.lock().unwrap().remove(k);
            Ok(())
        }
        fn backend_name(&self) -> &'static str {
            "test"
        }
    }

    #[test]
    fn token_response_diagnostics_expose_stages_without_secret_values() {
        let key = SigningKey::from_slice(&[1u8; 32]).unwrap();
        let valid = json!({"tokenType":"aws_sigv4","expiresIn":900,"refreshToken":"refresh-secret","accessToken":{"accessKeyId":"access-secret","secretAccessKey":"key-secret","sessionToken":"session-secret"}});
        for (pointer, replacement, stage) in [
            ("/expiresIn", json!("unexpected-secret"), "token_expires_in"),
            ("/accessToken/accessKeyId", Value::Null, "accessKeyId"),
            ("/accessToken/secretAccessKey", json!(""), "secretAccessKey"),
            (
                "/accessToken/sessionToken",
                json!(["unexpected-secret"]),
                "sessionToken",
            ),
            ("/refreshToken", Value::Null, "refreshToken"),
        ] {
            let mut body = valid.clone();
            *body.pointer_mut(pointer).unwrap() = replacement;
            let error = parse_credentials(&body, "us-east-1", &key).err().unwrap();
            assert_eq!(error.code(), "aws_auth_response_rejected");
            let wire = error.to_string();
            assert!(wire.contains(stage));
            for secret in [
                "unexpected-secret",
                "access-secret",
                "key-secret",
                "session-secret",
                "refresh-secret",
            ] {
                assert!(!wire.contains(secret));
            }
        }
    }

    #[tokio::test]
    async fn non_json_token_response_diagnostic_excludes_response_content() {
        let mut server = mockito::Server::new_async().await;
        let response = server
            .mock("POST", "/v1/token")
            .with_status(200)
            .with_body("<html>sensitive-response-content</html>")
            .expect(1)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let auth = AwsAuth::new(Arc::new(MemorySecrets::default()), dir.path().into()).unwrap();
        let key = SigningKey::from_slice(&[1u8; 32]).unwrap();
        let error = auth
            .token_request(&server.url(), &key, &json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "aws_auth_response_rejected");
        assert!(error.to_string().contains("token_json"));
        assert!(!error.to_string().contains("sensitive-response-content"));
        response.assert_async().await;
    }

    #[test]
    fn credential_response_accepts_metadata_without_model_enum_or_lifetime_caps() {
        let key = SigningKey::from_slice(&[1u8; 32]).unwrap();
        let body = json!({"tokenType":"service-defined-type","expiresIn":7200,"refreshToken":"refresh-secret","accessToken":{"accessKeyId":"ACCESSKEY","secretAccessKey":"key-secret","sessionToken":"session-secret"}});
        let before = now();
        let credentials = parse_credentials(&body, "us-east-1", &key).unwrap();
        assert_eq!(credentials.access_key_id, "ACCESSKEY");
        assert!(credentials.expires_at >= before + 7200);
        assert!(credentials.expires_at <= now() + 7200);
        for expires_in in [json!(900), json!(900.0), json!("900")] {
            let mut compatible = body.clone();
            compatible["expiresIn"] = expires_in;
            let before = now();
            let parsed = parse_credentials(&compatible, "us-east-1", &key).unwrap();
            assert!(parsed.expires_at >= before + 900);
            assert!(parsed.expires_at <= now() + 900);
        }
        for expires_in in [json!(0), json!(-1), json!(1.5), json!(u64::MAX)] {
            let mut invalid = body.clone();
            invalid["expiresIn"] = expires_in;
            assert!(parse_credentials(&invalid, "us-east-1", &key).is_err());
        }
        for access_token in [
            json!("opaque-bearer-token"),
            json!({"accessKeyId":"ACCESSKEY"}),
        ] {
            let mut invalid = body.clone();
            invalid["accessToken"] = access_token;
            assert!(parse_credentials(&invalid, "us-east-1", &key).is_err());
        }
    }

    #[test]
    fn confirmation_code_decodes_query_and_binds_state() {
        let code = STANDARD.encode("state=expected&code=a%2Bb%2Fc%3D");
        assert_eq!(
            &*parse_verification_code(&code, "expected").unwrap(),
            "a+b/c="
        );
        assert!(parse_verification_code(&code, "other").is_err());
        assert!(parse_verification_code(
            &STANDARD.encode("state=expected&state=expected&code=one"),
            "expected"
        )
        .is_err());
        assert!(parse_verification_code("not-base64", "expected").is_err());
    }

    #[test]
    fn confirmation_code_accepts_whitespace_from_copying_but_preserves_binding() {
        let encoded = STANDARD.encode("state=expected&code=a%2Bb%2Fc%3D");
        let wrapped = encoded
            .as_bytes()
            .chunks(7)
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect::<Vec<_>>()
            .join(" \r\n\t");
        assert_eq!(
            &*parse_verification_code(&format!("\n{wrapped}\n"), "expected").unwrap(),
            "a+b/c="
        );
        assert_eq!(
            parse_verification_code(&wrapped, "other")
                .unwrap_err()
                .code(),
            "aws_login_state_mismatch"
        );
        assert_eq!(
            parse_verification_code(&format!("{encoded}!"), "expected")
                .unwrap_err()
                .code(),
            "aws_login_code_invalid"
        );
    }

    #[test]
    fn confirmation_code_failures_identify_the_stage_without_exposing_secrets() {
        for (input, expected) in [
            ("not-base64".to_string(), "aws_login_code_invalid"),
            (STANDARD.encode("code=secret-code"), "aws_login_code_fields"),
            (
                STANDARD.encode("state=secret-state"),
                "aws_login_code_fields",
            ),
            (
                STANDARD.encode("state=expected&code=secret-code&code=duplicate"),
                "aws_login_code_fields",
            ),
            (
                STANDARD.encode("state=secret-state&code=secret-code"),
                "aws_login_state_mismatch",
            ),
            ("a".repeat(16 * 1024 + 1), "aws_login_code_length"),
        ] {
            let error = parse_verification_code(&input, "expected").unwrap_err();
            assert_eq!(error.code(), expected);
            assert!(!error.to_string().contains("secret-code"));
            assert!(!error.to_string().contains("secret-state"));
        }
    }

    #[test]
    fn confirmation_code_accepts_long_codes_and_still_binds_state() {
        for length in [512, 513, 1896, 8192] {
            let code = "a".repeat(length);
            let response = STANDARD.encode(format!("state=expected&code={code}"));
            assert_eq!(
                &*parse_verification_code(&response, "expected").unwrap(),
                &code
            );
            assert_eq!(
                parse_verification_code(&response, "other")
                    .unwrap_err()
                    .code(),
                "aws_login_state_mismatch"
            );
        }
    }

    #[test]
    fn dpop_is_verifiable_es256_jose_and_binds_request() {
        let key = SigningKey::from_slice(&[1u8; 32]).unwrap();
        let proof = dpop_proof(
            &key,
            "https://us-east-1.signin.aws.amazon.com/v1/token",
            123456789,
            "unique-id",
        )
        .unwrap();
        let parts: Vec<_> = proof.split('.').collect();
        let header: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        let payload: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(header["typ"], "dpop+jwt");
        assert_eq!(header["jwk"]["crv"], "P-256");
        assert_eq!(payload["htm"], "POST");
        assert_eq!(payload["jti"], "unique-id");
        let bytes = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        assert_eq!(bytes.len(), 64);
        key.verifying_key()
            .verify(
                format!("{}.{}", parts[0], parts[1]).as_bytes(),
                &Signature::from_slice(&bytes).unwrap(),
            )
            .unwrap();
    }

    #[tokio::test]
    async fn start_uses_official_cross_device_contract_and_owner_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemorySecrets::default());
        let auth = AwsAuth::new(secrets.clone(), dir.path().into()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let owner = SessionOwner::Window("a".into());
        let login = auth.start(&id, "us-east-1", &owner).await.unwrap();
        let url = url::Url::parse(&login.authorization_url).unwrap();
        assert_eq!(url.host_str(), Some("us-east-1.signin.aws.amazon.com"));
        let query: HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(query["client_id"], CLIENT_ID);
        assert_eq!(query["code_challenge_method"], "SHA-256");
        assert_eq!(query["code_challenge"].len(), 43);
        assert_eq!(
            auth.cancel(&id, &login.login_id, &SessionOwner::Window("b".into()))
                .await
                .unwrap_err()
                .code(),
            "aws_login_owner_mismatch"
        );
        assert!(auth.pending.lock().await.contains_key(&id));
        auth.cancel(&id, &login.login_id, &owner).await.unwrap();
        assert!(secrets.get(&login_key(&id)).unwrap().is_none());
        assert!(auth.pending.lock().await.is_empty());
    }

    #[tokio::test]
    async fn logout_invalidates_pending_across_process_coordinators() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemorySecrets::default());
        let first = AwsAuth::new(secrets.clone(), dir.path().into()).unwrap();
        let second = AwsAuth::new(secrets, dir.path().into()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let owner = SessionOwner::Window("a".into());
        let login = first.start(&id, "us-east-1", &owner).await.unwrap();
        second.logout(&id).await.unwrap();
        let error = first
            .complete(&id, &login.login_id, "irrelevant", &owner)
            .await
            .err()
            .unwrap();
        assert_eq!(error.code(), "aws_login_cancelled");
    }

    #[tokio::test]
    async fn concurrent_refresh_rotates_once_across_coordinators() {
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemorySecrets::default());
        let key = SigningKey::from_slice(&[1u8; 32]).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        // Refresh providers do not depend on tokenType or an idToken.
        let token = json!({"expiresIn":"7200","refreshToken":"rotated","accessToken":{"accessKeyId":"NEWKEY","secretAccessKey":"newsecret","sessionToken":"newsession"}});
        let m = server
            .mock("POST", "/v1/token")
            .match_header("content-type", "application/json")
            .match_header(
                "dpop",
                mockito::Matcher::Regex("^[^.]+\\.[^.]+\\.[^.]+$".into()),
            )
            .match_body(mockito::Matcher::Json(
                json!({"clientId":CLIENT_ID,"grantType":"refresh_token","refreshToken":"old"}),
            ))
            .with_status(200)
            .with_body(token.to_string())
            .expect(1)
            .create_async()
            .await;
        let mut first = AwsAuth::new(secrets.clone(), dir.path().into()).unwrap();
        first.endpoint_override = Some(server.url());
        let mut second = AwsAuth::new(secrets, dir.path().into()).unwrap();
        second.endpoint_override = Some(server.url());
        let current = StoredCredentials {
            access_key_id: "OLDKEY".into(),
            secret_access_key: "oldsecret".into(),
            session_token: "oldsession".into(),
            account_id: "123456789012".into(),
            arn: "arn:aws:iam::123456789012:user/test".into(),
            expires_at: 0,
            refresh_token: "old".into(),
            dpop_key: URL_SAFE_NO_PAD.encode(key.to_bytes()),
            signin_region: "us-east-1".into(),
        };
        first.save(&id, &current).unwrap();
        let (a, b) = tokio::join!(first.credentials(&id), second.credentials(&id));
        assert_eq!(a.unwrap().access_key_id, "NEWKEY");
        assert_eq!(b.unwrap().access_key_id, "NEWKEY");
        assert_eq!(first.load(&id).unwrap().refresh_token, "rotated");
        m.assert_async().await;
    }

    #[tokio::test]
    async fn exchange_verifies_identity_with_sts_and_ignores_id_token_claims() {
        assert_authorization_code_exchange("authorization+code", "aws_sigv4").await;
    }

    #[tokio::test]
    async fn exchange_sends_long_authorization_code_unchanged() {
        // Match the observed real response length, including query-escaped bytes.
        assert_authorization_code_exchange(&format!("{}+/=", "a".repeat(1893)), "aws_sigv4").await;
    }

    #[tokio::test]
    async fn exchange_verifies_credentials_with_service_defined_token_type() {
        assert_authorization_code_exchange("authorization+code", "service-defined-type").await;
    }

    async fn assert_authorization_code_exchange(authorization_code: &str, token_type: &str) {
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemorySecrets::default());
        let mut auth = AwsAuth::new(secrets, dir.path().into()).unwrap();
        auth.endpoint_override = Some(server.url());
        auth.identity_endpoint_override = Some(format!("{}/identity", server.url()));
        let id = uuid::Uuid::new_v4().to_string();
        let owner = SessionOwner::Window("main".into());
        let login = auth.start(&id, "us-east-1", &owner).await.unwrap();
        let (state, verifier) = {
            let pending = auth.pending.lock().await;
            let p = pending.get(&id).unwrap();
            (p.state.clone(), p.verifier.to_string())
        };
        let token=server.mock("POST","/v1/token").match_header("dpop",mockito::Matcher::Regex("^[^.]+\\.[^.]+\\.[^.]+$".into())).match_body(mockito::Matcher::Json(json!({"clientId":CLIENT_ID,"grantType":"authorization_code","code":authorization_code,"codeVerifier":verifier,"redirectUri":format!("{}/v1/sessions/confirmation",server.url())}))).with_status(200).with_body(json!({"tokenType":token_type,"expiresIn":900,"refreshToken":"refresh-secret","idToken":"deliberately-invalid-and-ignored","accessToken":{"accessKeyId":"ACCESSKEY","secretAccessKey":"secret-key","sessionToken":"session-secret"}}).to_string()).expect(1).create_async().await;
        let identity=server.mock("POST","/identity").match_header("authorization",mockito::Matcher::Regex("AWS4-HMAC-SHA256 Credential=ACCESSKEY/.*".into())).match_header("x-amz-security-token","session-secret").with_status(200).with_body("<GetCallerIdentityResponse><GetCallerIdentityResult><Account>123456789012</Account><Arn>arn:aws:iam::123456789012:user/verified</Arn></GetCallerIdentityResult></GetCallerIdentityResponse>").expect(1).create_async().await;
        let response = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("state", &state)
            .append_pair("code", authorization_code)
            .finish();
        let code = STANDARD.encode(response);
        let status = auth
            .complete(&id, &login.login_id, &code, &owner)
            .await
            .unwrap();
        assert_eq!(status.account_id.as_deref(), Some("123456789012"));
        assert_eq!(
            status.arn.as_deref(),
            Some("arn:aws:iam::123456789012:user/verified")
        );
        let wire = serde_json::to_string(&status).unwrap();
        assert!(!wire.contains("secret"));
        assert!(!wire.contains("ACCESSKEY"));
        assert!(!auth.status(&id).unwrap().authenticated);
        auth.commit(&id, &login.login_id, &owner).await.unwrap();
        assert_eq!(
            auth.credentials(&id).await.unwrap().access_key_id,
            "ACCESSKEY"
        );
        token.assert_async().await;
        identity.assert_async().await;
        auth.cancel(&id, &login.login_id, &owner).await.unwrap();
        assert!(auth.status(&id).unwrap().authenticated);
    }

    #[tokio::test]
    async fn exchange_never_stores_credentials_rejected_by_sts() {
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemorySecrets::default());
        let mut auth = AwsAuth::new(secrets, dir.path().into()).unwrap();
        auth.endpoint_override = Some(server.url());
        auth.identity_endpoint_override = Some(format!("{}/identity", server.url()));
        let id = uuid::Uuid::new_v4().to_string();
        let owner = SessionOwner::Window("main".into());
        let login = auth.start(&id, "us-east-1", &owner).await.unwrap();
        let state = auth.pending.lock().await.get(&id).unwrap().state.clone();
        let token = server.mock("POST", "/v1/token")
            .with_status(200)
            .with_body(json!({"tokenType":"service-defined-type","expiresIn":900,"refreshToken":"refresh-secret","accessToken":{"accessKeyId":"ACCESSKEY","secretAccessKey":"secret-key","sessionToken":"session-secret"}}).to_string())
            .expect(1).create_async().await;
        let identity = server
            .mock("POST", "/identity")
            .with_status(403)
            .with_body(
                "<ErrorResponse><Error><Code>InvalidClientTokenId</Code></Error></ErrorResponse>",
            )
            .expect(1)
            .create_async()
            .await;
        let code = STANDARD.encode(format!("state={state}&code=test"));
        assert!(auth
            .complete(&id, &login.login_id, &code, &owner)
            .await
            .is_err());
        assert!(!auth.status(&id).unwrap().authenticated);
        assert!(auth.secrets.get(&credential_key(&id)).unwrap().is_none());
        token.assert_async().await;
        identity.assert_async().await;
    }

    async fn assert_cancellation_during_exchange(close_owner: bool) {
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemorySecrets::default());
        let mut auth = AwsAuth::new(secrets, dir.path().into()).unwrap();
        auth.endpoint_override = Some(server.url());
        auth.identity_endpoint_override = Some(format!("{}/identity", server.url()));
        let auth = Arc::new(auth);
        let id = uuid::Uuid::new_v4().to_string();
        let owner = SessionOwner::Window("main".into());
        let login = auth.start(&id, "us-east-1", &owner).await.unwrap();
        let state = auth.pending.lock().await.get(&id).unwrap().state.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let started = Arc::new(std::sync::Mutex::new(Some(started_tx)));
        let token_body=json!({"tokenType":"aws_sigv4","expiresIn":900,"refreshToken":"refresh","accessToken":{"accessKeyId":"ACCESSKEY","secretAccessKey":"secret","sessionToken":"session"}}).to_string();
        let token = server
            .mock("POST", "/v1/token")
            .with_status(200)
            .with_chunked_body(move |writer| {
                if let Some(sender) = started.lock().unwrap().take() {
                    let _ = sender.send(());
                }
                std::thread::sleep(Duration::from_millis(100));
                writer.write_all(token_body.as_bytes())
            })
            .create_async()
            .await;
        let identity=server.mock("POST","/identity").with_status(200).with_body("<GetCallerIdentityResponse><GetCallerIdentityResult><Account>123456789012</Account><Arn>arn:aws:iam::123456789012:user/test</Arn></GetCallerIdentityResult></GetCallerIdentityResponse>").create_async().await;
        let complete = {
            let auth = auth.clone();
            let id = id.clone();
            let login_id = login.login_id.clone();
            let owner = owner.clone();
            let code = STANDARD.encode(format!("state={state}&code=test"));
            tokio::spawn(async move { auth.complete(&id, &login_id, &code, &owner).await })
        };
        started_rx.await.unwrap();
        assert!(auth.pending.try_lock().is_ok());
        assert!(open_lock(dir.path()).unwrap().try_lock().is_ok());
        if close_owner {
            auth.cancel_owner(&owner).await.unwrap();
        } else {
            auth.cancel(&id, &login.login_id, &owner).await.unwrap();
        }
        assert_eq!(
            complete.await.unwrap().err().unwrap().code(),
            "aws_login_cancelled"
        );
        assert!(!auth.status(&id).unwrap().authenticated);
        token.assert_async().await;
        identity.assert_async().await;
    }

    #[tokio::test]
    async fn cancellation_during_code_exchange_never_commits_credentials() {
        assert_cancellation_during_exchange(false).await;
    }

    #[tokio::test]
    async fn owner_teardown_during_code_exchange_never_commits_credentials() {
        assert_cancellation_during_exchange(true).await;
    }

    #[tokio::test]
    async fn owner_teardown_cancels_start_queued_on_cross_process_lock() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemorySecrets::default());
        let auth = AwsAuth::new(secrets.clone(), dir.path().into()).unwrap();
        let lock = open_lock(dir.path()).unwrap();
        lock.lock().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let owner = SessionOwner::Window("closing".into());
        let start = {
            let auth = auth.clone();
            let id = id.clone();
            let owner = owner.clone();
            tokio::spawn(async move { auth.start(&id, "us-east-1", &owner).await })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            while auth.pending.try_lock().is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let teardown = {
            let auth = auth.clone();
            let owner = owner.clone();
            tokio::spawn(async move { auth.cancel_owner(&owner).await })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if auth
                    .owner_generation(&owner_key(&owner))
                    .unwrap()
                    .load(Ordering::SeqCst)
                    == 1
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(lock);
        assert_eq!(
            start.await.unwrap().err().unwrap().code(),
            "aws_login_cancelled"
        );
        teardown.await.unwrap().unwrap();
        assert!(secrets.get(&login_key(&id)).unwrap().is_none());
        assert!(auth.pending.lock().await.is_empty());
        // A later lifetime using the same window label can start normally.
        auth.start(&id, "us-east-1", &owner).await.unwrap();
    }

    struct BlockIo {
        operation: &'static str,
        suffix: &'static str,
        entered: tokio::sync::oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }

    #[derive(Default)]
    struct BlockingSecrets {
        memory: MemorySecrets,
        blocked: std::sync::Mutex<Option<BlockIo>>,
    }

    impl BlockingSecrets {
        fn arm(
            &self,
            operation: &'static str,
            suffix: &'static str,
        ) -> (
            tokio::sync::oneshot::Receiver<()>,
            std::sync::mpsc::Sender<()>,
        ) {
            let (entered, receiver) = tokio::sync::oneshot::channel();
            let (sender, release) = std::sync::mpsc::channel();
            *self.blocked.lock().unwrap() = Some(BlockIo {
                operation,
                suffix,
                entered,
                release,
            });
            (receiver, sender)
        }

        fn before(&self, operation: &str, key: &str) {
            let blocked = {
                let mut blocked = self.blocked.lock().unwrap();
                if blocked
                    .as_ref()
                    .is_some_and(|b| b.operation == operation && key.ends_with(b.suffix))
                {
                    blocked.take()
                } else {
                    None
                }
            };
            if let Some(blocked) = blocked {
                let _ = blocked.entered.send(());
                // A timeout makes an accidental runtime-blocking regression
                // fail the heartbeat assertion instead of hanging the suite.
                let _ = blocked.release.recv_timeout(Duration::from_secs(3));
            }
        }
    }

    impl SecretStore for BlockingSecrets {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            self.before("get", key);
            self.memory.get(key)
        }
        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            self.before("set", key);
            self.memory.set(key, value)
        }
        fn delete(&self, key: &str) -> AppResult<()> {
            self.before("delete", key);
            self.memory.delete(key)
        }
        fn backend_name(&self) -> &'static str {
            "blocking-test"
        }
    }

    async fn heartbeat(entered: tokio::sync::oneshot::Receiver<()>, started: Instant) {
        entered.await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "SecretStore blocked the current-thread runtime"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn status_keychain_io_keeps_current_thread_runtime_responsive() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(BlockingSecrets::default());
        let auth = AwsAuth::new(secrets.clone(), dir.path().into()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let (entered, release) = secrets.arm("get", ":tokens");
        let started = Instant::now();
        let status = tokio::spawn(async move { auth.status_async(&id).await });
        heartbeat(entered, started).await;
        assert!(!status.is_finished());
        release.send(()).unwrap();
        assert!(!status.await.unwrap().unwrap().authenticated);
    }

    fn test_credentials(access_key: &str) -> StoredCredentials {
        let key = SigningKey::from_slice(&[1u8; 32]).unwrap();
        let mut credentials = parse_credentials(&json!({
            "expiresIn": 900, "refreshToken": "refresh-secret",
            "accessToken": {"accessKeyId": access_key, "secretAccessKey": "secret", "sessionToken": "session"}
        }), "us-east-1", &key).unwrap();
        credentials.account_id = "123456789012".into();
        credentials.arn = "arn:aws:iam::123456789012:user/test".into();
        credentials
    }

    async fn stage_ready_draft(auth: &AwsAuth, id: &str, owner: &SessionOwner) -> LoginStart {
        let login = auth.start(id, "us-east-1", owner).await.unwrap();
        auth.pending.lock().await.get_mut(id).unwrap().ready = Some(test_credentials("NEWKEY"));
        login
    }

    #[derive(Default)]
    struct ObservedSecrets {
        memory: MemorySecrets,
        accesses: AtomicU64,
    }

    impl SecretStore for ObservedSecrets {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            self.accesses.fetch_add(1, Ordering::SeqCst);
            self.memory.get(key)
        }
        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            self.accesses.fetch_add(1, Ordering::SeqCst);
            self.memory.set(key, value)
        }
        fn delete(&self, key: &str) -> AppResult<()> {
            self.accesses.fetch_add(1, Ordering::SeqCst);
            self.memory.delete(key)
        }
        fn backend_name(&self) -> &'static str {
            "observed-test"
        }
    }

    #[tokio::test]
    async fn unsaved_login_and_cancellation_never_access_secrets_or_replace_saved_grant() {
        for had_previous in [false, true] {
            let mut server = mockito::Server::new_async().await;
            let dir = tempfile::tempdir().unwrap();
            let secrets = Arc::new(ObservedSecrets::default());
            let mut auth = AwsAuth::new(secrets.clone(), dir.path().into()).unwrap();
            auth.endpoint_override = Some(server.url());
            auth.identity_endpoint_override = Some(format!("{}/identity", server.url()));
            let id = uuid::Uuid::new_v4().to_string();
            let owner = SessionOwner::Window("draft".into());
            if had_previous {
                secrets
                    .memory
                    .set(
                        &credential_key(&id),
                        &serde_json::to_string(&test_credentials("OLDKEY")).unwrap(),
                    )
                    .unwrap();
            }
            let previous = secrets.memory.get(&credential_key(&id)).unwrap();
            let token = server.mock("POST", "/v1/token").with_status(200)
                .with_body(json!({"tokenType":"service-defined","expiresIn":900,"refreshToken":"new-refresh", "accessToken":{"accessKeyId":"NEWKEY","secretAccessKey":"new-secret","sessionToken":"new-session"}}).to_string())
                .expect(1).create_async().await;
            let identity = server.mock("POST", "/identity").with_status(200)
                .with_body("<GetCallerIdentityResponse><GetCallerIdentityResult><Account>123456789012</Account><Arn>arn:aws:iam::123456789012:user/new</Arn></GetCallerIdentityResult></GetCallerIdentityResponse>")
                .expect(1).create_async().await;
            let login = auth.start(&id, "us-east-1", &owner).await.unwrap();
            let state = auth.pending.lock().await.get(&id).unwrap().state.clone();
            let code = STANDARD.encode(format!("state={state}&code=test"));
            let status = auth
                .complete(&id, &login.login_id, &code, &owner)
                .await
                .unwrap();
            assert!(status.authenticated);
            assert!(
                auth.complete(&id, &login.login_id, "ignored-for-ready-draft", &owner)
                    .await
                    .unwrap()
                    .authenticated
            );
            assert_eq!(secrets.accesses.load(Ordering::SeqCst), 0);
            assert_eq!(secrets.memory.get(&credential_key(&id)).unwrap(), previous);
            auth.cancel(&id, &login.login_id, &owner).await.unwrap();
            auth.cancel_owner(&owner).await.unwrap();
            assert_eq!(secrets.accesses.load(Ordering::SeqCst), 0);
            assert_eq!(secrets.memory.get(&credential_key(&id)).unwrap(), previous);
            assert!(auth.pending.lock().await.is_empty());
            assert!(source_revision(dir.path(), &id).unwrap().is_none());
            assert!(secrets.memory.get(&login_key(&id)).unwrap().is_none());
            token.assert_async().await;
            identity.assert_async().await;
        }
    }

    #[tokio::test]
    async fn failed_source_save_rolls_back_saved_grant_and_keeps_draft_for_retry() {
        for had_previous in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let secrets = Arc::new(MemorySecrets::default());
            let auth = AwsAuth::new(secrets.clone(), dir.path().into()).unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            let owner = SessionOwner::Window("save".into());
            if had_previous {
                auth.save(&id, &test_credentials("OLDKEY")).unwrap();
            }
            let previous = secrets.get(&credential_key(&id)).unwrap();
            let login = stage_ready_draft(&auth, &id, &owner).await;
            let error = auth
                .commit_with(&id, &login.login_id, &owner, || {
                    Err::<(), _>(failure("aws_auth_request_failed"))
                })
                .await
                .unwrap_err();
            assert_eq!(error.code(), "aws_auth_request_failed");
            assert_eq!(secrets.get(&credential_key(&id)).unwrap(), previous);
            assert!(auth.pending.lock().await.get(&id).unwrap().ready.is_some());
            assert_eq!(
                auth.commit_with(&id, &login.login_id, &owner, || Ok(42))
                    .await
                    .unwrap(),
                42
            );
            assert_eq!(auth.load(&id).unwrap().access_key_id, "NEWKEY");
            auth.cancel(&id, &login.login_id, &owner).await.unwrap();
            auth.cancel_owner(&owner).await.unwrap();
            assert_eq!(auth.load(&id).unwrap().access_key_id, "NEWKEY");
        }
    }

    #[tokio::test]
    async fn another_owner_cannot_replace_a_ready_draft_and_teardown_keeps_saved_grant() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(ObservedSecrets::default());
        let auth = AwsAuth::new(secrets.clone(), dir.path().into()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let first = SessionOwner::Window("first".into());
        let second = SessionOwner::Window("second".into());
        secrets
            .memory
            .set(
                &credential_key(&id),
                &serde_json::to_string(&test_credentials("OLDKEY")).unwrap(),
            )
            .unwrap();
        let previous = secrets.memory.get(&credential_key(&id)).unwrap();
        let login = stage_ready_draft(&auth, &id, &first).await;
        assert_eq!(
            auth.start(&id, "us-west-2", &second)
                .await
                .err()
                .unwrap()
                .code(),
            "aws_auth_busy"
        );
        assert_eq!(
            auth.pending.lock().await.get(&id).unwrap().login_id,
            login.login_id
        );
        auth.cancel_owner(&second).await.unwrap();
        assert!(auth.pending.lock().await.get(&id).unwrap().ready.is_some());
        auth.cancel_owner(&first).await.unwrap();
        assert!(auth.pending.lock().await.is_empty());
        assert_eq!(secrets.accesses.load(Ordering::SeqCst), 0);
        assert_eq!(secrets.memory.get(&credential_key(&id)).unwrap(), previous);
    }

    #[tokio::test]
    async fn revisions_prevent_stale_drafts_after_other_process_logout_delete_or_commit() {
        for action in ["logout", "delete", "commit"] {
            let dir = tempfile::tempdir().unwrap();
            let secrets = Arc::new(MemorySecrets::default());
            let first = AwsAuth::new(secrets.clone(), dir.path().into()).unwrap();
            let second = AwsAuth::new(secrets.clone(), dir.path().into()).unwrap();
            let id = uuid::Uuid::new_v4().to_string();
            let owner = SessionOwner::Window("first".into());
            let draft = stage_ready_draft(&first, &id, &owner).await;
            match action {
                "logout" => second.logout(&id).await.unwrap(),
                "delete" => {
                    with_removed_credentials(secrets.as_ref(), dir.path(), &[&id], || Ok(()))
                        .unwrap()
                }
                _ => {
                    let other_owner = SessionOwner::Window("second".into());
                    let other = stage_ready_draft(&second, &id, &other_owner).await;
                    second
                        .commit(&id, &other.login_id, &other_owner)
                        .await
                        .unwrap();
                }
            }
            let previous = secrets.get(&credential_key(&id)).unwrap();
            assert_eq!(
                first
                    .commit(&id, &draft.login_id, &owner)
                    .await
                    .err()
                    .unwrap()
                    .code(),
                "aws_login_cancelled"
            );
            assert_eq!(secrets.get(&credential_key(&id)).unwrap(), previous);
            let revision = source_revision(dir.path(), &id).unwrap().unwrap();
            assert!(uuid::Uuid::parse_str(&revision).is_ok());
            assert_eq!(revision.len(), 36);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn owner_teardown_rolls_back_inflight_save_after_caller_disappears() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(BlockingSecrets::default());
        let auth = AwsAuth::new(secrets.clone(), dir.path().into()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let owner = SessionOwner::Window("closing".into());
        auth.save(&id, &test_credentials("OLDKEY")).unwrap();
        let login = stage_ready_draft(&auth, &id, &owner).await;
        let (entered, release) = secrets.arm("set", ":tokens");
        let updated = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started = Instant::now();
        let save = {
            let auth = auth.clone();
            let id = id.clone();
            let owner = owner.clone();
            let updated = updated.clone();
            tokio::spawn(async move {
                auth.commit_with(&id, &login.login_id, &owner, move || {
                    updated.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .await
            })
        };
        heartbeat(entered, started).await;
        save.abort();
        let _ = save.await;
        let cleanup = {
            let auth = auth.clone();
            let owner = owner.clone();
            tokio::spawn(async move { auth.cancel_owner(&owner).await })
        };
        tokio::task::yield_now().await;
        assert_eq!(
            auth.owner_generation(&owner_key(&owner))
                .unwrap()
                .load(Ordering::SeqCst),
            1
        );
        assert!(open_lock(dir.path()).unwrap().try_lock().is_err());
        release.send(()).unwrap();
        cleanup.await.unwrap().unwrap();
        assert_eq!(auth.load(&id).unwrap().access_key_id, "OLDKEY");
        assert!(!updated.load(Ordering::SeqCst));
        assert!(auth.pending.lock().await.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropped_save_caller_finishes_transaction_and_success_wins_late_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(MemorySecrets::default());
        let auth = AwsAuth::new(secrets, dir.path().into()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let owner = SessionOwner::Window("closing-after-save".into());
        let login = stage_ready_draft(&auth, &id, &owner).await;
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let updated = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let save = {
            let auth = auth.clone();
            let id = id.clone();
            let owner = owner.clone();
            let updated = updated.clone();
            tokio::spawn(async move {
                auth.commit_with(&id, &login.login_id, &owner, move || {
                    let _ = entered_tx.send(());
                    release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                    updated.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .await
            })
        };
        entered_rx.await.unwrap();
        save.abort();
        let _ = save.await;
        assert!(open_lock(dir.path()).unwrap().try_lock().is_err());
        let cleanup = {
            let auth = auth.clone();
            let owner = owner.clone();
            tokio::spawn(async move { auth.cancel_owner(&owner).await })
        };
        tokio::task::yield_now().await;
        release_tx.send(()).unwrap();
        cleanup.await.unwrap().unwrap();
        assert!(updated.load(Ordering::SeqCst));
        assert_eq!(auth.load(&id).unwrap().access_key_id, "NEWKEY");
    }

    struct FailingTokenWrite {
        memory: MemorySecrets,
        fail: std::sync::atomic::AtomicBool,
        write_before_error: bool,
    }
    impl SecretStore for FailingTokenWrite {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            self.memory.get(key)
        }
        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            if self.fail.swap(false, Ordering::SeqCst) {
                if self.write_before_error {
                    self.memory.set(key, value)?;
                }
                return Err(failure("aws_auth_request_failed"));
            }
            self.memory.set(key, value)
        }
        fn delete(&self, key: &str) -> AppResult<()> {
            self.memory.delete(key)
        }
        fn backend_name(&self) -> &'static str {
            "failed-write-test"
        }
    }

    #[tokio::test]
    async fn failed_secret_write_restores_previous_and_draft_can_retry() {
        for had_previous in [false, true] {
            for write_before_error in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let secrets = Arc::new(FailingTokenWrite {
                    memory: MemorySecrets::default(),
                    fail: std::sync::atomic::AtomicBool::new(true),
                    write_before_error,
                });
                let auth = AwsAuth::new(secrets.clone(), dir.path().into()).unwrap();
                let id = uuid::Uuid::new_v4().to_string();
                let owner = SessionOwner::Window("failed-write".into());
                if had_previous {
                    secrets
                        .memory
                        .set(
                            &credential_key(&id),
                            &serde_json::to_string(&test_credentials("OLDKEY")).unwrap(),
                        )
                        .unwrap();
                }
                let previous = secrets.memory.get(&credential_key(&id)).unwrap();
                let login = stage_ready_draft(&auth, &id, &owner).await;
                let updated = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let observed = updated.clone();
                assert!(auth
                    .commit_with(&id, &login.login_id, &owner, move || {
                        observed.store(true, Ordering::SeqCst);
                        Ok(())
                    })
                    .await
                    .is_err());
                assert!(!updated.load(Ordering::SeqCst));
                assert_eq!(secrets.memory.get(&credential_key(&id)).unwrap(), previous);
                assert!(auth.pending.lock().await.get(&id).unwrap().ready.is_some());
                auth.commit(&id, &login.login_id, &owner).await.unwrap();
                assert_eq!(auth.load(&id).unwrap().access_key_id, "NEWKEY");
            }
        }
    }

    #[tokio::test]
    async fn failed_rollback_discards_draft_instead_of_retrying_unknown_saved_state() {
        struct BrokenStore;
        impl SecretStore for BrokenStore {
            fn get(&self, _: &str) -> AppResult<Option<String>> {
                Ok(None)
            }
            fn set(&self, _: &str, _: &str) -> AppResult<()> {
                Err(failure("aws_auth_request_failed"))
            }
            fn delete(&self, _: &str) -> AppResult<()> {
                Err(failure("aws_auth_request_failed"))
            }
            fn backend_name(&self) -> &'static str {
                "broken-test"
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let auth = AwsAuth::new(Arc::new(BrokenStore), dir.path().into()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let owner = SessionOwner::Window("rollback".into());
        let login = stage_ready_draft(&auth, &id, &owner).await;
        assert!(auth.commit(&id, &login.login_id, &owner).await.is_err());
        assert!(auth.pending.lock().await.is_empty());
        assert_eq!(
            auth.commit(&id, &login.login_id, &owner)
                .await
                .err()
                .unwrap()
                .code(),
            "aws_login_cancelled"
        );
    }
}
