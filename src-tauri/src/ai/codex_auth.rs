//! Device-local Codex OAuth credentials. No Codex executable is involved.
//!
//! The device-code endpoints and public client ID follow OpenAI's Codex login
//! implementation. Only the encrypted SecretStore holds long-lived tokens;
//! neither provider rows nor configuration exports contain this state.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::db::{ai_provider, Db};
use crate::error::{AppError, AppResult};
use crate::secret::SecretStore;

pub const PROTOCOL: &str = "codex-subscription";
pub const ENDPOINT: &str = "https://chatgpt.com/backend-api/codex";
const AUTH_ORIGIN: &str = "https://auth.openai.com";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const LOGIN_LIFETIME: u64 = 15 * 60;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_LIMIT: usize = 256 * 1024;

pub fn credential_key(provider_id: &str) -> String {
    format!("ai_codex:{provider_id}:tokens")
}

fn login_key(provider_id: &str) -> String {
    format!("ai_codex:{provider_id}:login")
}

pub fn has_credentials(secrets: &dyn SecretStore, provider_id: &str) -> AppResult<bool> {
    secrets.exists(&credential_key(provider_id))
}

fn clear_credentials(secrets: &dyn SecretStore, provider_id: &str) -> AppResult<()> {
    // Invalidate pending login before deleting tokens, including when deletion
    // fails. A delayed authorization must never recreate a removed provider.
    secrets.delete(&login_key(provider_id))?;
    secrets.delete(&credential_key(provider_id))
}

fn open_lock(data_dir: &Path) -> AppResult<File> {
    std::fs::create_dir_all(data_dir)?;
    Ok(OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(data_dir.join("codex-auth.lock"))?)
}

/// Configuration import runs on a blocking thread. Use this when a provider is
/// deleted or changes protocol, so other GUI/server processes lose their old
/// login attempt as well. Never call while holding the database mutex.
pub fn forget_credentials(
    secrets: &dyn SecretStore,
    data_dir: &Path,
    provider_id: &str,
) -> AppResult<()> {
    with_removed_credentials(secrets, data_dir, &[provider_id], || Ok(()))
}

/// Clear removed grants before committing their provider-row changes. Holding
/// the same lock through the update prevents a pending login from restoring a
/// grant in between. A secret-store failure leaves the rows available to retry.
pub fn with_removed_credentials<T>(
    secrets: &dyn SecretStore,
    data_dir: &Path,
    provider_ids: &[&str],
    update: impl FnOnce() -> AppResult<T>,
) -> AppResult<T> {
    if provider_ids.is_empty() {
        return update();
    }
    let file = open_lock(data_dir)?;
    file.lock()?;
    for provider_id in provider_ids {
        clear_credentials(secrets, provider_id)?;
    }
    update()
}

#[derive(Serialize)]
pub struct AuthStatus {
    pub authenticated: bool,
}

#[derive(Serialize)]
pub struct DeviceLogin {
    pub login_id: String,
    pub verification_url: String,
    pub user_code: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Serialize)]
pub struct LoginPoll {
    pub status: &'static str,
    pub interval: u64,
}

/// Intentionally neither Debug nor Serialize: bearer credentials never enter
/// command responses, logs, or the configuration sync payload.
pub struct AccessToken {
    pub access_token: String,
    pub account_id: String,
}

#[derive(Serialize, Deserialize)]
struct Credentials {
    access_token: String,
    refresh_token: String,
    account_id: String,
    expires_at: u64,
}

struct PendingLogin {
    login_id: String,
    device_auth_id: String,
    user_code: String,
    interval: u64,
    expires_at: Instant,
    next_poll: Instant,
}

pub struct CodexAuth {
    db: Arc<Db>,
    secrets: Arc<dyn SecretStore>,
    data_dir: PathBuf,
    http: reqwest::Client,
    auth_origin: String,
    // One coordinator per host serves all windows and AI sessions. The file
    // lock additionally serializes separate GUI/server processes sharing a DB.
    pending: Mutex<HashMap<String, PendingLogin>>,
    // Cancellation can be requested while a poll owns the async/file locks.
    cancelled: std::sync::Mutex<HashSet<(String, String)>>,
}

impl CodexAuth {
    pub fn new(db: Arc<Db>, secrets: Arc<dyn SecretStore>, data_dir: PathBuf) -> AppResult<Self> {
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("rssh/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| failure("codex_auth_request_failed"))?;
        Ok(Self {
            db,
            secrets,
            data_dir,
            http,
            auth_origin: AUTH_ORIGIN.into(),
            pending: Mutex::new(HashMap::new()),
            cancelled: std::sync::Mutex::new(HashSet::new()),
        })
    }

    fn validate_provider(&self, provider_id: &str) -> AppResult<()> {
        if ai_provider::get(&self.db, provider_id)?.is_some_and(|row| row.protocol == PROTOCOL) {
            Ok(())
        } else {
            Err(failure("codex_provider_invalid"))
        }
    }

    pub fn status(&self, provider_id: &str) -> AppResult<AuthStatus> {
        self.validate_provider(provider_id)?;
        Ok(AuthStatus {
            authenticated: has_credentials(self.secrets.as_ref(), provider_id)?,
        })
    }

    async fn lock_file(&self) -> AppResult<File> {
        let file = open_lock(&self.data_dir)?;
        let deadline = Instant::now() + HTTP_TIMEOUT + Duration::from_secs(5);
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
                return Err(failure("codex_auth_busy"));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    pub async fn login_start(&self, provider_id: &str) -> AppResult<DeviceLogin> {
        let mut pending = self.pending.lock().await;
        let _lock = self.lock_file().await?;
        self.validate_provider(provider_id)?;
        pending.retain(|_, entry| entry.expires_at > Instant::now());
        // Starting another login cancels the earlier attempt even if this new
        // request fails. Existing usable credentials remain until success.
        pending.remove(provider_id);
        self.cancelled
            .lock()
            .map_err(|_| AppError::Lock)?
            .retain(|(id, _)| id != provider_id);
        self.secrets.delete(&login_key(provider_id))?;
        let response = self
            .http
            .post(format!(
                "{}/api/accounts/deviceauth/usercode",
                self.auth_origin
            ))
            .json(&json!({"client_id": CLIENT_ID}))
            .send()
            .await
            .map_err(|_| failure("codex_auth_request_failed"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(failure("codex_auth_unavailable"));
        }
        let body = success_json(response).await?;
        let device_auth_id = required_string(&body, "device_auth_id")?;
        let user_code = body
            .get("user_code")
            .or_else(|| body.get("usercode"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 256)
            .ok_or_else(|| failure("codex_auth_invalid_response"))?
            .to_string();
        let interval = seconds(&body["interval"])
            .unwrap_or(5)
            .clamp(1, LOGIN_LIFETIME);
        let login_id = uuid::Uuid::new_v4().to_string();
        self.validate_provider(provider_id)?;
        self.secrets.set(&login_key(provider_id), &login_id)?;
        pending.insert(
            provider_id.into(),
            PendingLogin {
                login_id: login_id.clone(),
                device_auth_id,
                user_code: user_code.clone(),
                interval,
                expires_at: Instant::now() + Duration::from_secs(LOGIN_LIFETIME),
                next_poll: Instant::now() + Duration::from_secs(interval),
            },
        );
        Ok(DeviceLogin {
            login_id,
            verification_url: format!("{}/codex/device", self.auth_origin),
            user_code,
            interval,
            expires_in: LOGIN_LIFETIME,
        })
    }

    pub async fn login_poll(&self, provider_id: &str, login_id: &str) -> AppResult<LoginPoll> {
        let mut pending = self.pending.lock().await;
        let _lock = self.lock_file().await?;
        self.validate_provider(provider_id)?;
        let generation = self.secrets.get(&login_key(provider_id))?;
        if generation.as_deref() == Some(format!("complete:{login_id}").as_str())
            && has_credentials(self.secrets.as_ref(), provider_id)?
        {
            return Ok(LoginPoll {
                status: "complete",
                interval: 0,
            });
        }
        if generation.as_deref() != Some(login_id) {
            return Err(failure("codex_login_cancelled"));
        }
        let Some(attempt) = pending
            .get_mut(provider_id)
            .filter(|p| p.login_id == login_id)
        else {
            return Err(failure("codex_login_cancelled"));
        };
        if Instant::now() >= attempt.expires_at {
            pending.remove(provider_id);
            self.secrets.delete(&login_key(provider_id))?;
            return Err(failure("codex_login_expired"));
        }
        if Instant::now() < attempt.next_poll {
            return Ok(LoginPoll {
                status: "pending",
                interval: attempt.interval,
            });
        }
        attempt.next_poll = Instant::now() + Duration::from_secs(attempt.interval);
        let response = self
            .http
            .post(format!(
                "{}/api/accounts/deviceauth/token",
                self.auth_origin
            ))
            .timeout(
                attempt
                    .expires_at
                    .saturating_duration_since(Instant::now())
                    .min(HTTP_TIMEOUT),
            )
            .json(
                &json!({"device_auth_id": attempt.device_auth_id, "user_code": attempt.user_code}),
            )
            .send()
            .await
            .map_err(|_| failure("codex_auth_request_failed"))?;
        // Codex's device endpoint reports pending authorization as 403/404.
        if matches!(response.status().as_u16(), 403 | 404) {
            return Ok(LoginPoll {
                status: "pending",
                interval: attempt.interval,
            });
        }
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            attempt.interval = (attempt.interval + 5).min(LOGIN_LIFETIME);
            attempt.next_poll = Instant::now() + Duration::from_secs(attempt.interval);
            return Ok(LoginPoll {
                status: "pending",
                interval: attempt.interval,
            });
        }
        let body = success_json(response).await?;
        let code = required_string(&body, "authorization_code")?;
        let verifier = required_string(&body, "code_verifier")?;
        let response = self
            .http
            .post(format!("{}/oauth/token", self.auth_origin))
            .timeout(
                attempt
                    .expires_at
                    .saturating_duration_since(Instant::now())
                    .min(HTTP_TIMEOUT),
            )
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", CLIENT_ID),
                ("code", &code),
                ("code_verifier", &verifier),
                (
                    "redirect_uri",
                    &format!("{}/deviceauth/callback", self.auth_origin),
                ),
            ])
            .send()
            .await
            .map_err(|_| failure("codex_auth_request_failed"))?;
        let credentials = credentials_from_response(success_json(response).await?, None)?;
        if self
            .cancelled
            .lock()
            .map_err(|_| AppError::Lock)?
            .contains(&(provider_id.into(), login_id.into()))
        {
            pending.remove(provider_id);
            self.secrets.delete(&login_key(provider_id))?;
            return Err(failure("codex_login_cancelled"));
        }
        if Instant::now() >= attempt.expires_at {
            pending.remove(provider_id);
            self.secrets.delete(&login_key(provider_id))?;
            return Err(failure("codex_login_expired"));
        }
        self.validate_provider(provider_id)?;
        self.save(provider_id, &credentials)?;
        self.secrets
            .set(&login_key(provider_id), &format!("complete:{login_id}"))?;
        pending.remove(provider_id);
        Ok(LoginPoll {
            status: "complete",
            interval: 0,
        })
    }

    pub async fn login_cancel(&self, provider_id: &str, login_id: &str) -> AppResult<()> {
        let key = (provider_id.to_owned(), login_id.to_owned());
        self.cancelled
            .lock()
            .map_err(|_| AppError::Lock)?
            .insert(key.clone());
        let mut pending = self.pending.lock().await;
        let _lock = self.lock_file().await?;
        if pending
            .get(provider_id)
            .is_some_and(|p| p.login_id == login_id)
        {
            pending.remove(provider_id);
        }
        let generation = self.secrets.get(&login_key(provider_id))?;
        if generation.as_deref() == Some(login_id) {
            self.secrets.delete(&login_key(provider_id))?;
        } else if generation.as_deref() == Some(format!("complete:{login_id}").as_str()) {
            // A poll in another process may have completed while cancellation
            // waited for its file lock. Cancel only this exact attempt; never
            // clear a later login. UI completion must stop sending cancel.
            clear_credentials(self.secrets.as_ref(), provider_id)?;
        }
        self.cancelled
            .lock()
            .map_err(|_| AppError::Lock)?
            .remove(&key);
        Ok(())
    }

    pub async fn logout(&self, provider_id: &str) -> AppResult<()> {
        let mut pending = self.pending.lock().await;
        let _lock = self.lock_file().await?;
        pending.remove(provider_id);
        clear_credentials(self.secrets.as_ref(), provider_id)
    }

    /// Clear secrets before deleting the row so a storage failure remains
    /// recoverable from settings; serialize with poll/refresh through removal.
    pub async fn delete_provider(&self, provider_id: &str) -> AppResult<()> {
        let mut pending = self.pending.lock().await;
        let _lock = self.lock_file().await?;
        clear_credentials(self.secrets.as_ref(), provider_id)?;
        ai_provider::delete(&self.db, provider_id)?;
        pending.remove(provider_id);
        Ok(())
    }

    /// A second 401 rejects the refreshed grant. Do not erase a newer login or
    /// refresh completed by another request while the failing HTTP call ran.
    pub async fn reject(&self, provider_id: &str, rejected_access_token: &str) -> AppResult<()> {
        let _pending = self.pending.lock().await;
        let _lock = self.lock_file().await?;
        if !has_credentials(self.secrets.as_ref(), provider_id)? {
            return Ok(());
        }
        if self.load(provider_id)?.access_token == rejected_access_token {
            // An independently started device login can still finish; only
            // the bearer credential that was actually rejected is removed.
            self.secrets.delete(&credential_key(provider_id))?;
        }
        Ok(())
    }

    fn load(&self, provider_id: &str) -> AppResult<Credentials> {
        let raw = self
            .secrets
            .get(&credential_key(provider_id))?
            .ok_or_else(|| failure("codex_login_required"))?;
        serde_json::from_str(&raw).map_err(|_| failure("codex_auth_invalid_response"))
    }

    fn save(&self, provider_id: &str, credentials: &Credentials) -> AppResult<()> {
        let raw = serde_json::to_string(credentials)
            .map_err(|_| failure("codex_auth_invalid_response"))?;
        self.secrets.set(&credential_key(provider_id), &raw)
    }

    /// Refresh shortly before expiry. On a 401, pass the token rejected by the
    /// request: another session/process may already have refreshed it, in which
    /// case the new saved token is reused instead of consuming refresh twice.
    pub async fn authorize(
        self: &Arc<Self>,
        provider_id: &str,
        rejected_access_token: Option<&str>,
    ) -> AppResult<AccessToken> {
        let auth = self.clone();
        let provider_id = provider_id.to_owned();
        let rejected_access_token = rejected_access_token.map(str::to_owned);
        // Cancelling an AI turn must not abort refresh after the server has
        // consumed the old refresh token but before its replacement is saved.
        // This bounded transaction outlives its caller; logout waits for the
        // same lock and then removes its result.
        tokio::spawn(async move {
            auth.authorize_inner(&provider_id, rejected_access_token.as_deref())
                .await
        })
        .await
        .map_err(|_| failure("codex_auth_request_failed"))?
    }

    async fn authorize_inner(
        &self,
        provider_id: &str,
        rejected_access_token: Option<&str>,
    ) -> AppResult<AccessToken> {
        let _pending = self.pending.lock().await;
        let _lock = self.lock_file().await?;
        self.validate_provider(provider_id)?;
        let current = self.load(provider_id)?;
        let rejected = rejected_access_token == Some(current.access_token.as_str());
        if !rejected && current.expires_at > now().saturating_add(60) {
            return Ok(AccessToken {
                access_token: current.access_token,
                account_id: current.account_id,
            });
        }
        let response = self
            .http
            .post(format!("{}/oauth/token", self.auth_origin))
            .json(&json!({"grant_type":"refresh_token", "client_id":CLIENT_ID, "refresh_token":current.refresh_token}))
            .send()
            .await
            .map_err(|_| failure("codex_auth_request_failed"))?;
        let status = response.status();
        // A failed refresh invalidates only the existing grant. An independent
        // device login may still be waiting for the user's authorization.
        if !status.is_success() {
            let body = response_json(response).await.unwrap_or(Value::Null);
            if permanent_refresh_failure(status.as_u16(), &body) {
                self.secrets.delete(&credential_key(provider_id))?;
                return Err(failure("codex_login_required"));
            }
            return Err(http_failure(status.as_u16()));
        }
        let refreshed = credentials_from_response(response_json(response).await?, Some(&current))?;
        if refreshed.account_id != current.account_id {
            self.secrets.delete(&credential_key(provider_id))?;
            return Err(failure("codex_login_required"));
        }
        self.validate_provider(provider_id)?;
        // Save the replacement refresh token before exposing the access token.
        self.save(provider_id, &refreshed)?;
        Ok(AccessToken {
            access_token: refreshed.access_token,
            account_id: refreshed.account_id,
        })
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn failure(code: &'static str) -> AppError {
    AppError::config(code, json!({}))
}

fn http_failure(status: u16) -> AppError {
    // OAuth error descriptions can echo tokens. Never propagate response text.
    AppError::other("codex_auth_failed", json!({"status": status}))
}

async fn success_json(response: reqwest::Response) -> AppResult<Value> {
    if !response.status().is_success() {
        return Err(http_failure(response.status().as_u16()));
    }
    response_json(response).await
}

async fn response_json(mut response: reqwest::Response) -> AppResult<Value> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure("codex_auth_request_failed"))?
    {
        if bytes.len().saturating_add(chunk.len()) > RESPONSE_LIMIT {
            return Err(failure("codex_auth_invalid_response"));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| failure("codex_auth_invalid_response"))
}

fn required_string(body: &Value, field: &str) -> AppResult<String> {
    body[field]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= 128 * 1024)
        .map(str::to_owned)
        .ok_or_else(|| failure("codex_auth_invalid_response"))
}

fn seconds(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| *n > 0)
}

fn claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?).ok()
}

fn credentials_from_response(
    body: Value,
    previous: Option<&Credentials>,
) -> AppResult<Credentials> {
    let access_token = required_string(&body, "access_token")?;
    let refresh_token = body["refresh_token"]
        .as_str()
        .filter(|v| !v.is_empty())
        .or_else(|| previous.map(|p| p.refresh_token.as_str()))
        .ok_or_else(|| failure("codex_auth_invalid_response"))?
        .to_owned();
    // These claims come from the fixed OAuth server over TLS. They are used for
    // expiry/routing only, not as proof of an independently supplied identity.
    let access_claims = claims(&access_token).unwrap_or(Value::Null);
    let id_claims = body["id_token"]
        .as_str()
        .and_then(claims)
        .unwrap_or(Value::Null);
    let account_id = access_claims["https://api.openai.com/auth"]["chatgpt_account_id"]
        .as_str()
        .or_else(|| id_claims["https://api.openai.com/auth"]["chatgpt_account_id"].as_str())
        .or_else(|| previous.map(|p| p.account_id.as_str()))
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 256
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .ok_or_else(|| failure("codex_auth_invalid_response"))?
        .to_string();
    let duration_expiry = seconds(&body["expires_in"]).map(|seconds| now().saturating_add(seconds));
    let jwt_expiry = seconds(&access_claims["exp"]);
    let expires_at = match (duration_expiry, jwt_expiry) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) | (None, Some(a)) => a,
        _ => return Err(failure("codex_auth_invalid_response")),
    };
    if expires_at <= now() {
        return Err(failure("codex_auth_invalid_response"));
    }
    Ok(Credentials {
        access_token,
        refresh_token,
        account_id,
        expires_at,
    })
}

fn permanent_refresh_failure(status: u16, body: &Value) -> bool {
    let code = body["error"]
        .as_str()
        .or_else(|| body["error"]["code"].as_str())
        .unwrap_or("");
    status == 401
        || matches!(
            code,
            "invalid_grant"
                | "refresh_token_expired"
                | "refresh_token_reused"
                | "refresh_token_invalidated"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct MemorySecrets {
        values: std::sync::Mutex<HashMap<String, String>>,
        reads: AtomicUsize,
        fail_delete: std::sync::atomic::AtomicBool,
    }

    impl SecretStore for MemorySecrets {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            Ok(self.values.lock().unwrap().get(key).cloned())
        }
        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            self.values.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
        fn delete(&self, key: &str) -> AppResult<()> {
            if self.fail_delete.load(Ordering::SeqCst) {
                return Err(AppError::config("test_delete_failed", json!({})));
            }
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
        fn exists(&self, key: &str) -> AppResult<bool> {
            Ok(self.values.lock().unwrap().contains_key(key))
        }
        fn backend_name(&self) -> &'static str {
            "memory"
        }
    }

    const PROVIDER: &str = "provider-codex";

    struct Fixture {
        auth: Arc<CodexAuth>,
        db: Arc<Db>,
        secrets: Arc<MemorySecrets>,
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new(origin: &str) -> Self {
            let db = Arc::new(Db::open_in_memory().unwrap());
            ai_provider::upsert(
                &db,
                &ai_provider::ProviderRow {
                    id: PROVIDER.into(),
                    name: "Codex".into(),
                    protocol: PROTOCOL.into(),
                    endpoint: ENDPOINT.into(),
                    model: String::new(),
                },
            )
            .unwrap();
            let secrets = Arc::new(MemorySecrets::default());
            let dir = tempfile::tempdir().unwrap();
            let mut auth = CodexAuth::new(db.clone(), secrets.clone(), dir.path().into()).unwrap();
            auth.auth_origin = origin.into();
            Self {
                auth: Arc::new(auth),
                db,
                secrets,
                dir,
            }
        }

        fn other_manager(&self, origin: &str) -> Arc<CodexAuth> {
            let mut auth = CodexAuth::new(
                self.db.clone(),
                self.secrets.clone(),
                self.dir.path().into(),
            )
            .unwrap();
            auth.auth_origin = origin.into();
            Arc::new(auth)
        }

        fn seed(&self, expires_at: u64) {
            self.auth
                .save(
                    PROVIDER,
                    &Credentials {
                        access_token: "old-access".into(),
                        refresh_token: "old-refresh".into(),
                        account_id: "account-1".into(),
                        expires_at,
                    },
                )
                .unwrap();
        }

        async fn due(&self) {
            self.auth
                .pending
                .lock()
                .await
                .get_mut(PROVIDER)
                .unwrap()
                .next_poll = Instant::now();
        }
    }

    fn token_response(refresh: Option<&str>) -> Value {
        let claims = json!({"exp": now() + 3600, "https://api.openai.com/auth": {"chatgpt_account_id":"account-1"}});
        let access = format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let mut value = json!({"access_token":access, "expires_in":3600});
        if let Some(refresh) = refresh {
            value["refresh_token"] = json!(refresh);
        }
        value
    }

    #[tokio::test]
    async fn device_login_exchanges_code_and_keeps_tokens_in_secret_store() {
        let mut server = mockito::Server::new_async().await;
        let fixture = Fixture::new(&server.url());
        let start = server
            .mock("POST", "/api/accounts/deviceauth/usercode")
            .match_body(mockito::Matcher::Json(json!({"client_id":CLIENT_ID})))
            .with_body(
                json!({"device_auth_id":"device-secret", "usercode":"ABCD-EFGH", "interval":"5"})
                    .to_string(),
            )
            .create_async()
            .await;
        let poll = server
            .mock("POST", "/api/accounts/deviceauth/token")
            .match_body(mockito::Matcher::Json(
                json!({"device_auth_id":"device-secret", "user_code":"ABCD-EFGH"}),
            ))
            .with_body(
                json!({"authorization_code":"exchange-secret", "code_verifier":"verifier-secret"})
                    .to_string(),
            )
            .create_async()
            .await;
        let exchange = server
            .mock("POST", "/oauth/token")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("grant_type".into(), "authorization_code".into()),
                mockito::Matcher::UrlEncoded("code".into(), "exchange-secret".into()),
                mockito::Matcher::UrlEncoded("code_verifier".into(), "verifier-secret".into()),
            ]))
            .with_body(token_response(Some("refresh-secret")).to_string())
            .create_async()
            .await;
        let login = fixture.auth.login_start(PROVIDER).await.unwrap();
        assert_eq!(login.user_code, "ABCD-EFGH");
        assert_eq!(login.interval, 5);
        assert_eq!(login.expires_in, 900);
        assert!(!fixture.auth.status(PROVIDER).unwrap().authenticated);
        fixture.due().await;
        assert_eq!(
            fixture
                .auth
                .login_poll(PROVIDER, &login.login_id)
                .await
                .unwrap()
                .status,
            "complete"
        );
        assert_eq!(
            fixture
                .auth
                .login_poll(PROVIDER, &login.login_id)
                .await
                .unwrap()
                .status,
            "complete"
        );
        assert!(fixture.auth.status(PROVIDER).unwrap().authenticated);
        assert_eq!(
            fixture.auth.load(PROVIDER).unwrap().refresh_token,
            "refresh-secret"
        );
        let ui = serde_json::to_string(&fixture.auth.status(PROVIDER).unwrap()).unwrap();
        assert_eq!(ui, "{\"authenticated\":true}");
        let row = ai_provider::get(&fixture.db, PROVIDER).unwrap().unwrap();
        assert_eq!(row.endpoint, ENDPOINT);
        start.assert_async().await;
        poll.assert_async().await;
        exchange.assert_async().await;
    }

    #[tokio::test]
    async fn polling_obeys_interval_and_cancel_prevents_exchange() {
        let mut server = mockito::Server::new_async().await;
        let fixture = Fixture::new(&server.url());
        let _start = server
            .mock("POST", "/api/accounts/deviceauth/usercode")
            .with_body(r#"{"device_auth_id":"d","user_code":"code","interval":30}"#)
            .create_async()
            .await;
        let poll = server
            .mock("POST", "/api/accounts/deviceauth/token")
            .with_status(403)
            .expect(1)
            .create_async()
            .await;
        let login = fixture.auth.login_start(PROVIDER).await.unwrap();
        assert_eq!(
            fixture
                .auth
                .login_poll(PROVIDER, &login.login_id)
                .await
                .unwrap()
                .status,
            "pending"
        );
        fixture.due().await;
        assert_eq!(
            fixture
                .auth
                .login_poll(PROVIDER, &login.login_id)
                .await
                .unwrap()
                .status,
            "pending"
        );
        assert_eq!(
            fixture
                .auth
                .login_poll(PROVIDER, &login.login_id)
                .await
                .unwrap()
                .status,
            "pending"
        );
        fixture
            .auth
            .login_cancel(PROVIDER, &login.login_id)
            .await
            .unwrap();
        let err = fixture
            .auth
            .login_poll(PROVIDER, &login.login_id)
            .await
            .err()
            .unwrap();
        assert_eq!(err.code(), "codex_login_cancelled");
        assert!(!fixture.auth.status(PROVIDER).unwrap().authenticated);
        poll.assert_async().await;
    }

    #[tokio::test]
    async fn expired_login_and_provider_removal_never_save_tokens() {
        let mut server = mockito::Server::new_async().await;
        let fixture = Fixture::new(&server.url());
        let _start = server
            .mock("POST", "/api/accounts/deviceauth/usercode")
            .with_body(r#"{"device_auth_id":"d","user_code":"code"}"#)
            .expect(2)
            .create_async()
            .await;
        let login = fixture.auth.login_start(PROVIDER).await.unwrap();
        fixture
            .auth
            .pending
            .lock()
            .await
            .get_mut(PROVIDER)
            .unwrap()
            .expires_at = Instant::now();
        assert_eq!(
            fixture
                .auth
                .login_poll(PROVIDER, &login.login_id)
                .await
                .err()
                .unwrap()
                .code(),
            "codex_login_expired"
        );
        let login = fixture.auth.login_start(PROVIDER).await.unwrap();
        ai_provider::delete(&fixture.db, PROVIDER).unwrap();
        assert_eq!(
            fixture
                .auth
                .login_poll(PROVIDER, &login.login_id)
                .await
                .err()
                .unwrap()
                .code(),
            "codex_provider_invalid"
        );
        fixture.auth.logout(PROVIDER).await.unwrap();
        assert!(!has_credentials(fixture.secrets.as_ref(), PROVIDER).unwrap());
    }

    #[tokio::test]
    async fn sync_cleanup_invalidates_pending_login_in_another_manager() {
        let mut server = mockito::Server::new_async().await;
        let fixture = Fixture::new(&server.url());
        let _start = server
            .mock("POST", "/api/accounts/deviceauth/usercode")
            .with_body(r#"{"device_auth_id":"d","user_code":"code"}"#)
            .create_async()
            .await;
        fixture.seed(now() + 3600);
        let login = fixture.auth.login_start(PROVIDER).await.unwrap();
        forget_credentials(fixture.secrets.as_ref(), fixture.dir.path(), PROVIDER).unwrap();
        assert_eq!(
            fixture
                .auth
                .login_poll(PROVIDER, &login.login_id)
                .await
                .err()
                .unwrap()
                .code(),
            "codex_login_cancelled"
        );
        assert!(!fixture.auth.status(PROVIDER).unwrap().authenticated);
    }

    #[tokio::test]
    async fn cancelling_during_exchange_preserves_the_previous_login() {
        let mut server = mockito::Server::new_async().await;
        let fixture = Fixture::new(&server.url());
        fixture.seed(now() + 3600);
        let _start = server
            .mock("POST", "/api/accounts/deviceauth/usercode")
            .with_body(r#"{"device_auth_id":"d","user_code":"code"}"#)
            .create_async()
            .await;
        let _poll = server
            .mock("POST", "/api/accounts/deviceauth/token")
            .with_body(r#"{"authorization_code":"c","code_verifier":"v"}"#)
            .create_async()
            .await;
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = started.clone();
        let body = token_response(Some("unwanted-new-login")).to_string();
        let _exchange = server
            .mock("POST", "/oauth/token")
            .with_body_from_fn(move |writer| {
                flag.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(50));
                writer.write_all(body.as_bytes())
            })
            .create_async()
            .await;
        let login = fixture.auth.login_start(PROVIDER).await.unwrap();
        fixture.due().await;
        let auth = fixture.auth.clone();
        let id = login.login_id.clone();
        let poll = tokio::spawn(async move { auth.login_poll(PROVIDER, &id).await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !started.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        fixture
            .auth
            .login_cancel(PROVIDER, &login.login_id)
            .await
            .unwrap();
        assert_eq!(
            poll.await.unwrap().err().unwrap().code(),
            "codex_login_cancelled"
        );
        assert_eq!(
            fixture.auth.load(PROVIDER).unwrap().refresh_token,
            "old-refresh"
        );
    }

    #[tokio::test]
    async fn deleting_provider_preserves_row_when_secret_cleanup_fails() {
        let fixture = Fixture::new(AUTH_ORIGIN);
        fixture.seed(now() + 3600);
        fixture.secrets.fail_delete.store(true, Ordering::SeqCst);
        assert!(fixture.auth.delete_provider(PROVIDER).await.is_err());
        assert!(ai_provider::get(&fixture.db, PROVIDER).unwrap().is_some());
        fixture.secrets.fail_delete.store(false, Ordering::SeqCst);
        fixture.auth.delete_provider(PROVIDER).await.unwrap();
        assert!(ai_provider::get(&fixture.db, PROVIDER).unwrap().is_none());
        assert!(!has_credentials(fixture.secrets.as_ref(), PROVIDER).unwrap());
    }

    #[test]
    fn importing_removal_or_protocol_change_can_retry_failed_grant_cleanup() {
        let changed = json!({"provider":PROVIDER, "name":"API", "protocol":"openai-completions", "endpoint":"https://example.com/v1"});
        for providers in [json!([]), json!([changed])] {
            let fixture = Fixture::new(AUTH_ORIGIN);
            fixture.seed(now() + 3600);
            fixture
                .secrets
                .set(&login_key(PROVIDER), "pending-login")
                .unwrap();
            let payload = json!({"providers": providers});
            fixture.secrets.fail_delete.store(true, Ordering::SeqCst);
            assert!(crate::ai::commands::import_ai_settings(
                &fixture.db,
                fixture.secrets.as_ref(),
                fixture.dir.path(),
                &payload,
            )
            .is_err());
            assert_eq!(
                ai_provider::get(&fixture.db, PROVIDER)
                    .unwrap()
                    .unwrap()
                    .protocol,
                PROTOCOL
            );
            assert!(has_credentials(fixture.secrets.as_ref(), PROVIDER).unwrap());
            fixture.secrets.fail_delete.store(false, Ordering::SeqCst);
            crate::ai::commands::import_ai_settings(
                &fixture.db,
                fixture.secrets.as_ref(),
                fixture.dir.path(),
                &payload,
            )
            .unwrap();
            assert!(!has_credentials(fixture.secrets.as_ref(), PROVIDER).unwrap());
            assert!(fixture.secrets.get(&login_key(PROVIDER)).unwrap().is_none());
            let current = ai_provider::get(&fixture.db, PROVIDER).unwrap();
            if providers.as_array().unwrap().is_empty() {
                assert!(current.is_none());
            } else {
                assert_eq!(current.unwrap().protocol, "openai-completions");
            }
        }
    }

    #[test]
    fn importing_same_subscription_preserves_grant_without_auth_lock() {
        let fixture = Fixture::new(AUTH_ORIGIN);
        fixture.seed(now() + 3600);
        let payload = json!({"providers":[{"provider":PROVIDER, "name":"Renamed", "protocol":PROTOCOL, "model":"new-model"}]});
        crate::ai::commands::import_ai_settings(
            &fixture.db,
            fixture.secrets.as_ref(),
            fixture.dir.path(),
            &payload,
        )
        .unwrap();
        assert!(!fixture.dir.path().join("codex-auth.lock").exists());
        assert_eq!(
            fixture.auth.load(PROVIDER).unwrap().refresh_token,
            "old-refresh"
        );
        assert_eq!(
            ai_provider::get(&fixture.db, PROVIDER)
                .unwrap()
                .unwrap()
                .model,
            "new-model"
        );
    }

    #[tokio::test]
    async fn rejected_stale_token_does_not_clear_newer_grant_or_pending_login() {
        let fixture = Fixture::new(AUTH_ORIGIN);
        fixture.seed(now() + 3600);
        fixture
            .secrets
            .set(&login_key(PROVIDER), "independent-login")
            .unwrap();
        fixture.auth.reject(PROVIDER, "stale-access").await.unwrap();
        assert!(fixture.auth.status(PROVIDER).unwrap().authenticated);
        fixture.auth.reject(PROVIDER, "old-access").await.unwrap();
        assert!(!fixture.auth.status(PROVIDER).unwrap().authenticated);
        assert_eq!(
            fixture
                .secrets
                .get(&login_key(PROVIDER))
                .unwrap()
                .as_deref(),
            Some("independent-login")
        );
        fixture.auth.reject(PROVIDER, "old-access").await.unwrap();
    }

    #[tokio::test]
    async fn restarting_during_login_does_not_mistake_previous_credentials_for_success() {
        let mut server = mockito::Server::new_async().await;
        let fixture = Fixture::new(&server.url());
        let _start = server
            .mock("POST", "/api/accounts/deviceauth/usercode")
            .with_body(r#"{"device_auth_id":"d","user_code":"code"}"#)
            .create_async()
            .await;
        fixture.seed(now() + 3600);
        let login = fixture.auth.login_start(PROVIDER).await.unwrap();
        let restarted = fixture.other_manager(&server.url());
        assert_eq!(
            restarted
                .login_poll(PROVIDER, &login.login_id)
                .await
                .err()
                .unwrap()
                .code(),
            "codex_login_cancelled"
        );
    }

    #[tokio::test]
    async fn concurrent_managers_refresh_once_and_reuse_rotated_credentials() {
        let mut server = mockito::Server::new_async().await;
        let fixture = Fixture::new(&server.url());
        fixture.seed(0);
        let other = fixture.other_manager(&server.url());
        let refresh = server.mock("POST", "/oauth/token")
            .match_body(mockito::Matcher::Json(json!({"grant_type":"refresh_token", "client_id":CLIENT_ID, "refresh_token":"old-refresh"})))
            .with_body(token_response(Some("rotated-refresh")).to_string()).expect(1).create_async().await;
        let (a, b) = tokio::join!(
            fixture.auth.authorize(PROVIDER, None),
            other.authorize(PROVIDER, None)
        );
        assert_eq!(a.unwrap().access_token, b.unwrap().access_token);
        assert_eq!(
            fixture.auth.load(PROVIDER).unwrap().refresh_token,
            "rotated-refresh"
        );
        // A late 401 from the previous access token must not refresh again.
        fixture
            .auth
            .authorize(PROVIDER, Some("old-access"))
            .await
            .unwrap();
        refresh.assert_async().await;
    }

    #[tokio::test]
    async fn rejected_current_access_token_refreshes_and_optional_refresh_is_preserved() {
        let mut server = mockito::Server::new_async().await;
        let fixture = Fixture::new(&server.url());
        fixture.seed(now() + 3600);
        let refresh = server
            .mock("POST", "/oauth/token")
            .with_body(token_response(None).to_string())
            .expect(1)
            .create_async()
            .await;
        assert_eq!(
            fixture
                .auth
                .authorize(PROVIDER, None)
                .await
                .unwrap()
                .access_token,
            "old-access"
        );
        fixture
            .auth
            .authorize(PROVIDER, Some("old-access"))
            .await
            .unwrap();
        assert_eq!(
            fixture.auth.load(PROVIDER).unwrap().refresh_token,
            "old-refresh"
        );
        refresh.assert_async().await;
    }

    #[tokio::test]
    async fn cancelling_a_request_still_persists_the_rotated_refresh_token() {
        let mut server = mockito::Server::new_async().await;
        let fixture = Fixture::new(&server.url());
        fixture.seed(0);
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let body = token_response(Some("rotated-after-cancel")).to_string();
        let flag = started.clone();
        let refresh = server
            .mock("POST", "/oauth/token")
            .with_body_from_fn(move |writer| {
                flag.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(50));
                writer.write_all(body.as_bytes())
            })
            .expect(1)
            .create_async()
            .await;
        let auth = fixture.auth.clone();
        let request = tokio::spawn(async move { auth.authorize(PROVIDER, None).await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !started.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        request.abort();
        // This caller queues behind the detached transaction and reuses its
        // newly persisted token; it must not submit the old refresh token.
        fixture.auth.authorize(PROVIDER, None).await.unwrap();
        assert_eq!(
            fixture.auth.load(PROVIDER).unwrap().refresh_token,
            "rotated-after-cancel"
        );
        refresh.assert_async().await;
    }

    #[tokio::test]
    async fn logout_during_refresh_cannot_be_undone_by_the_refresh_response() {
        let mut server = mockito::Server::new_async().await;
        let fixture = Fixture::new(&server.url());
        fixture.seed(0);
        let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = started.clone();
        let body = token_response(Some("rotated-after-logout")).to_string();
        let refresh = server
            .mock("POST", "/oauth/token")
            .with_body_from_fn(move |writer| {
                flag.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(50));
                writer.write_all(body.as_bytes())
            })
            .create_async()
            .await;
        let auth = fixture.auth.clone();
        let request = tokio::spawn(async move { auth.authorize(PROVIDER, None).await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !started.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        fixture.auth.logout(PROVIDER).await.unwrap();
        request.await.unwrap().unwrap();
        assert!(!fixture.auth.status(PROVIDER).unwrap().authenticated);
        assert_eq!(
            fixture
                .auth
                .authorize(PROVIDER, None)
                .await
                .err()
                .unwrap()
                .code(),
            "codex_login_required"
        );
        refresh.assert_async().await;
    }

    #[tokio::test]
    async fn refresh_rejection_requires_login_but_transient_failure_keeps_credentials() {
        let mut server = mockito::Server::new_async().await;
        let fixture = Fixture::new(&server.url());
        fixture.seed(0);
        let transient = server
            .mock("POST", "/oauth/token")
            .with_status(503)
            .with_body("old-refresh MUST NOT LEAK")
            .create_async()
            .await;
        let err = fixture.auth.authorize(PROVIDER, None).await.err().unwrap();
        assert_eq!(err.code(), "codex_auth_failed");
        assert!(!err.to_string().contains("old-refresh"));
        assert!(fixture.auth.status(PROVIDER).unwrap().authenticated);
        transient.remove_async().await;
        let _permanent = server
            .mock("POST", "/oauth/token")
            .with_status(400)
            .with_body(r#"{"error":{"code":"refresh_token_reused","message":"old-refresh"}}"#)
            .create_async()
            .await;
        let err = fixture.auth.authorize(PROVIDER, None).await.err().unwrap();
        assert_eq!(err.code(), "codex_login_required");
        assert!(!err.to_string().contains("old-refresh"));
        assert!(!fixture.auth.status(PROVIDER).unwrap().authenticated);
    }

    async fn assert_refresh_failure_keeps_pending_login(status: usize, body: Value) {
        let mut server = mockito::Server::new_async().await;
        let fixture = Fixture::new(&server.url());
        fixture.seed(0);
        let start = server
            .mock("POST", "/api/accounts/deviceauth/usercode")
            .with_body(r#"{"device_auth_id":"device", "user_code":"code"}"#)
            .create_async()
            .await;
        let refresh = server
            .mock("POST", "/oauth/token")
            .match_body(mockito::Matcher::Json(json!({
                "grant_type": "refresh_token",
                "client_id": CLIENT_ID,
                "refresh_token": "old-refresh",
            })))
            .with_status(status)
            .with_body(body.to_string())
            .create_async()
            .await;
        let poll = server
            .mock("POST", "/api/accounts/deviceauth/token")
            .with_body(r#"{"authorization_code":"exchange", "code_verifier":"verifier"}"#)
            .create_async()
            .await;
        let exchange = server
            .mock("POST", "/oauth/token")
            .match_body(mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "authorization_code".into(),
            ))
            .with_body(token_response(Some("new-login-refresh")).to_string())
            .create_async()
            .await;

        let login = fixture.auth.login_start(PROVIDER).await.unwrap();
        let err = fixture.auth.authorize(PROVIDER, None).await.err().unwrap();
        assert_eq!(err.code(), "codex_login_required");
        assert!(!fixture.auth.status(PROVIDER).unwrap().authenticated);
        assert_eq!(
            fixture.secrets.get(&login_key(PROVIDER)).unwrap(),
            Some(login.login_id.clone())
        );
        refresh.assert_async().await;
        refresh.remove_async().await;
        fixture.due().await;
        assert_eq!(
            fixture
                .auth
                .login_poll(PROVIDER, &login.login_id)
                .await
                .unwrap()
                .status,
            "complete"
        );
        assert_eq!(
            fixture.auth.load(PROVIDER).unwrap().refresh_token,
            "new-login-refresh"
        );
        start.assert_async().await;
        poll.assert_async().await;
        exchange.assert_async().await;
    }

    #[tokio::test]
    async fn permanent_refresh_failure_keeps_pending_device_login() {
        assert_refresh_failure_keeps_pending_login(
            400,
            json!({"error": {"code": "refresh_token_reused"}}),
        )
        .await;
    }

    #[tokio::test]
    async fn mismatched_refresh_account_keeps_pending_device_login() {
        let mut response = token_response(Some("unexpected-refresh"));
        let claims = json!({"exp": now() + 3600, "https://api.openai.com/auth": {"chatgpt_account_id":"account-2"}});
        response["access_token"] = json!(format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        ));
        assert_refresh_failure_keeps_pending_login(200, response).await;
    }

    #[test]
    fn status_does_not_decrypt_credentials() {
        let fixture = Fixture::new(AUTH_ORIGIN);
        fixture.seed(now() + 3600);
        assert!(fixture.auth.status(PROVIDER).unwrap().authenticated);
        assert_eq!(fixture.secrets.reads.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn token_claims_and_expiry_are_required_and_refresh_errors_are_classified() {
        assert!(credentials_from_response(
            json!({"access_token":"opaque", "refresh_token":"r", "expires_in":3600}),
            None
        )
        .is_err());
        let mut expired = token_response(Some("r"));
        let claims =
            json!({"exp":1, "https://api.openai.com/auth":{"chatgpt_account_id":"account-1"}});
        expired["access_token"] = json!(format!(
            "h.{}.s",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        ));
        assert!(credentials_from_response(expired, None).is_err());
        for code in [
            "invalid_grant",
            "refresh_token_expired",
            "refresh_token_reused",
            "refresh_token_invalidated",
        ] {
            assert!(permanent_refresh_failure(400, &json!({"error":code})));
        }
        assert!(!permanent_refresh_failure(503, &Value::Null));
    }
}
