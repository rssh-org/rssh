use serde::Serialize;
use serde_json::json;
use tauri::State;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::{Credential, CredentialType, Profile, SshAlgorithmCatalog, SshAlgorithms};
use crate::secret::cred_secret_key;
use crate::state::AppState;

#[tauri::command]
pub fn list_profiles(state: State<AppState>) -> Result<Vec<Profile>, AppError> {
    crate::db::profile::list(&state.db)
}

#[tauri::command]
pub fn get_profile(state: State<AppState>, id: String) -> Result<Profile, AppError> {
    crate::db::profile::get(&state.db, &id)
}

#[tauri::command]
pub fn create_profile(state: State<AppState>, profile: Profile) -> Result<(), AppError> {
    crate::db::profile::insert(&state.db, &profile)
}

#[tauri::command]
pub fn update_profile(state: State<AppState>, profile: Profile) -> Result<(), AppError> {
    crate::db::profile::update(&state.db, &profile)
}

#[tauri::command]
pub fn delete_profile(state: State<AppState>, id: String) -> Result<(), AppError> {
    crate::db::profile::delete(&state.db, &id)
}

#[tauri::command]
pub fn ssh_algorithm_catalog() -> SshAlgorithmCatalog {
    crate::ssh::algorithms::catalog()
}

// ---------------------------------------------------------------------------
// Credentials — secret 走 SecretStore，metadata 走 DB
// 私钥 passphrase 不再持久化：连接时终端内交互输入，仅进程内缓存。
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_credentials(state: State<AppState>) -> Result<Vec<Credential>, AppError> {
    // 列表场景不返回 secret，避免无谓 keychain 查询
    crate::db::credential::list(&state.db)
}

#[tauri::command]
pub fn get_credential(state: State<AppState>, id: String) -> Result<Credential, AppError> {
    let mut cred = crate::db::credential::get(&state.db, &id)?;
    cred.secret = state.secret_store.get(&cred_secret_key(&id))?;
    Ok(cred)
}

#[tauri::command]
pub fn create_credential(state: State<AppState>, credential: Credential) -> Result<(), AppError> {
    crate::db::credential::insert(&state.db, &credential)?;
    save_credential_secrets(&state, &credential)
}

#[tauri::command]
pub fn update_credential(state: State<AppState>, credential: Credential) -> Result<(), AppError> {
    crate::db::credential::update(&state.db, &credential)?;
    save_credential_secrets(&state, &credential)
}

#[tauri::command]
pub fn delete_credential(state: State<AppState>, id: String) -> Result<(), AppError> {
    crate::db::credential::delete(&state.db, &id)?;
    state.secret_store.delete(&cred_secret_key(&id))?;
    Ok(())
}

fn save_credential_secrets(state: &State<AppState>, c: &Credential) -> Result<(), AppError> {
    let secret_key = cred_secret_key(&c.id);
    match c.secret.as_deref() {
        Some(s) if !s.is_empty() => state.secret_store.set(&secret_key, s)?,
        _ => state.secret_store.delete(&secret_key)?,
    }
    Ok(())
}

/// 允许快速填充的默认私钥名。webview 是不可信边界——用白名单而非黑名单：
/// 只放行这两个，`~/.ssh` 下的 config / known_hosts / 其它私钥一概读不到。
/// 与前端 `CredentialEditor.svelte` 的 DEFAULT_KEY_NAMES 保持一致。
const ALLOWED_DEFAULT_KEYS: &[&str] = &["id_rsa", "id_ed25519"];

/// A real private key is a few KB; anything past 1 MiB is not a key. Matches the
/// webview pick-file path's cap so both key-import routes reject oversized files
/// identically (see `pickTextFile({ maxBytes })` in the frontend).
const MAX_KEY_FILE_BYTES: u64 = 1024 * 1024;

/// 读 `~/.ssh/<name>` 私钥文件原文，供"快速填充默认密钥"用。
/// name 必须在 `ALLOWED_DEFAULT_KEYS` 白名单内，否则拒绝。
/// 文件不存在 → not_found，让前端给友好提示而不是 IO 噪声。
#[tauri::command]
pub fn read_default_key_file(name: String) -> AppResult<String> {
    if !ALLOWED_DEFAULT_KEYS.contains(&name.as_str()) {
        return Err(AppError::other("invalid_key_name", json!({ "name": name })));
    }
    let home =
        dirs::home_dir().ok_or_else(|| AppError::other("home_dir_unavailable", json!({})))?;
    read_key_file_capped(&home.join(".ssh").join(&name), &format!("~/.ssh/{name}"))
}

/// Read a key file's text. Split out from the command so the cap / not-found
/// logic is unit-testable without a real `~/.ssh`. `display` is the user-facing
/// path shown on not_found (`~/.ssh/id_rsa`), kept separate from the real fs
/// path so the error message stays the friendly relative form.
fn read_key_file_capped(path: &std::path::Path, display: &str) -> AppResult<String> {
    // Cap before slurping into memory. metadata() follows symlinks, matching
    // read_to_string; a stat failure (missing / permission) yields 0 here and
    // falls through to the read below, which maps NotFound → key_file_not_found
    // and other IO → io_error, so the not-found path keeps its friendly message.
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size > MAX_KEY_FILE_BYTES {
        return Err(AppError::other(
            "key_file_too_large",
            json!({ "size": size }),
        ));
    }
    match std::fs::read_to_string(path) {
        Ok(c) => Ok(c),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(AppError::not_found(
            "key_file_not_found",
            json!({ "path": display }),
        )),
        Err(e) => Err(e.into()),
    }
}

/// A parsed `Host` block from `~/.ssh/config`.
#[derive(Clone, Serialize)]
pub struct SshConfigHost {
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identity_file: String,
    pub proxy_jump: Option<String>,
    /// 该别名在 rssh 里已存在同名连接。
    pub already_exists: bool,
}

#[derive(Clone, Serialize)]
pub struct SshImportResult {
    pub alias: String,
    /// imported | skipped_exists | skipped_no_host | skipped_no_key | error
    pub status: String,
    pub message: String,
}

fn ssh_config_path() -> AppResult<std::path::PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| AppError::other("home_dir_unavailable", json!({})))?;
    Ok(home.join(".ssh").join("config"))
}

/// Minimal OpenSSH `config` parser: top-level `Host <aliases>` blocks with
/// indented keys. Wildcard aliases (`Host *` / `Host ?`) are skipped; a blank
/// line ends the current block so dangling key-only segments (no `Host` head)
/// don't leak into the previous block.
fn parse_ssh_config(content: &str) -> Vec<SshConfigHost> {
    let mut blocks: Vec<SshConfigHost> = Vec::new();
    let mut cur: Option<SshConfigHost> = None;
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() {
            if let Some(b) = cur.take() {
                blocks.push(b);
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let (key, val) = match line.split_once(char::is_whitespace) {
            Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim().to_string()),
            None => (line.to_ascii_lowercase(), String::new()),
        };
        if key == "host" {
            if let Some(b) = cur.take() {
                blocks.push(b);
            }
            let aliases: Vec<String> = val
                .split_whitespace()
                .filter(|a| !a.is_empty() && !a.contains('*') && !a.contains('?'))
                .map(String::from)
                .collect();
            if aliases.is_empty() {
                continue;
            }
            cur = Some(SshConfigHost {
                alias: aliases[0].clone(),
                host: String::new(),
                port: 22,
                user: String::new(),
                identity_file: String::new(),
                proxy_jump: None,
                already_exists: false,
            });
        } else if let Some(b) = cur.as_mut() {
            match key.as_str() {
                "hostname" => b.host = val,
                "port" => {
                    if let Ok(p) = val.parse::<u16>() {
                        b.port = p;
                    }
                }
                "user" => b.user = val,
                "identityfile" => b.identity_file = val,
                "proxyjump" | "proxycommand" => b.proxy_jump = Some(val),
                _ => {}
            }
        }
    }
    if let Some(b) = cur.take() {
        blocks.push(b);
    }
    blocks
}

/// Expand `~` / relative paths against `$HOME/.ssh` and require the resolved
/// key to stay under `$HOME/.ssh` (webview-adjacent surface: never read
/// arbitrary files). Returns Ok(None) when the key file doesn't exist.
fn resolve_identity_key(home: &std::path::Path, spec: &str) -> AppResult<Option<String>> {
    let ssh_dir = home.join(".ssh");
    let path = if let Some(rest) = spec.strip_prefix("~/") {
        home.join(rest)
    } else if std::path::Path::new(spec).is_absolute() {
        // 支持 Windows 盘符绝对路径（C:/Users/.../.ssh/id_rsa 等）
        std::path::PathBuf::from(spec)
    } else {
        // relative to ~/.ssh, per OpenSSH config semantics
        ssh_dir.join(spec)
    };
    if !path.starts_with(&ssh_dir) {
        return Err(AppError::other(
            "key_outside_ssh_dir",
            json!({ "path": path.display().to_string() }),
        ));
    }
    match std::fs::read_to_string(&path) {
        Ok(c) => Ok(Some(c)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Scan `~/.ssh/config` and return the importable host list (host blocks that
/// have a HostName), marking aliases that already exist as rssh connections.
#[tauri::command]
pub fn ssh_config_scan(state: State<AppState>) -> AppResult<Vec<SshConfigHost>> {
    let path = ssh_config_path()?;
    let content = std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::not_found("ssh_config_not_found", json!({ "path": path.display().to_string() }))
        } else {
            AppError::from(e)
        }
    })?;
    let existing: std::collections::HashSet<String> = crate::db::profile::list(&state.db)?
        .into_iter()
        .map(|p| p.name)
        .collect();
    Ok(parse_ssh_config(&content)
        .into_iter()
        .filter(|h| !h.host.is_empty())
        .map(|mut h| {
            h.already_exists = existing.contains(&h.alias);
            h
        })
        .collect())
}

/// Import the named aliases from `~/.ssh/config` as rssh connections: one
/// key credential (private key read from `IdentityFile`) + one profile per
/// alias. Aliases that already exist or lack a host/key are skipped.
#[tauri::command]
pub fn ssh_config_import(
    state: State<AppState>,
    aliases: Vec<String>,
) -> AppResult<Vec<SshImportResult>> {
    let path = ssh_config_path()?;
    let content = std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::not_found("ssh_config_not_found", json!({ "path": path.display().to_string() }))
        } else {
            AppError::from(e)
        }
    })?;
    let hosts = parse_ssh_config(&content);
    let existing: std::collections::HashSet<String> = crate::db::profile::list(&state.db)?
        .into_iter()
        .map(|p| p.name)
        .collect();
    let home =
        dirs::home_dir().ok_or_else(|| AppError::other("home_dir_unavailable", json!({})))?;

    let mut results = Vec::new();
    for alias in aliases {
        let alias = alias.trim().to_string();
        let Some(h) = hosts.iter().find(|h| h.alias == alias) else {
            results.push(SshImportResult {
                alias,
                status: "error".into(),
                message: "未在 ~/.ssh/config 找到该别名".into(),
            });
            continue;
        };
        if existing.contains(&alias) {
            results.push(SshImportResult {
                alias,
                status: "skipped_exists".into(),
                message: "rssh 里已有同名连接".into(),
            });
            continue;
        }
        if h.host.is_empty() {
            results.push(SshImportResult {
                alias,
                status: "skipped_no_host".into(),
                message: "缺少 HostName，跳过".into(),
            });
            continue;
        }
        // 私钥读不到就跳过，避免导入一个连不上的空 key 凭据
        let secret = match resolve_identity_key(&home, &h.identity_file) {
            Ok(Some(s)) => Some(s),
            Ok(None) => {
                results.push(SshImportResult {
                    alias,
                    status: "skipped_no_key".into(),
                    message: format!("找不到密钥文件（{}），可先复制到 ~/.ssh 再试", h.identity_file),
                });
                continue;
            }
            Err(e) => {
                results.push(SshImportResult {
                    alias,
                    status: "error".into(),
                    message: e.to_string(),
                });
                continue;
            }
        };

        let cred_id = format!("cred-{}", Uuid::new_v4().simple());
        let cred = Credential {
            id: cred_id.clone(),
            name: format!("{} (from ssh config)", alias),
            username: h.user.clone(),
            credential_type: CredentialType::Key,
            secret,
            save_to_remote: false,
        };
        if let Err(e) = crate::db::credential::insert(&state.db, &cred) {
            results.push(SshImportResult {
                alias,
                status: "error".into(),
                message: e.to_string(),
            });
            continue;
        }
        if let Some(s) = &cred.secret {
            let _ = state.secret_store.set(&cred_secret_key(&cred_id), s);
        }

        // ProxyJump 别名若已是 rssh 连接则挂为跳板，否则忽略（不阻断导入）
        let bastion_profile_id = h
            .proxy_jump
            .as_ref()
            .and_then(|pj| {
                let first = pj.split_whitespace().next()?;
                if existing.contains(first) { Some(first.to_string()) } else { None }
            });
        let prof = Profile {
            id: format!("prof-{}", Uuid::new_v4().simple()),
            name: alias.clone(),
            host: h.host.clone(),
            port: h.port,
            credential_id: cred_id,
            bastion_profile_id,
            init_command: None,
            group_id: None,
            algorithms: SshAlgorithms::default(),
        };
        if let Err(e) = crate::db::profile::insert(&state.db, &prof) {
            results.push(SshImportResult {
                alias,
                status: "error".into(),
                message: e.to_string(),
            });
            continue;
        }
        results.push(SshImportResult {
            alias,
            status: "imported".into(),
            message: format!("已导入 {}:{} ({}@)", h.host, h.port, h.user),
        });
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    //! Key-file reads for the default-key quick fill: capped at 1 MiB,
    //! NotFound maps to the friendly key_file_not_found code.
    use super::*;

    #[test]
    fn read_key_file_capped_reads_a_normal_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id_rsa");
        std::fs::write(&path, "PEM-CONTENT").unwrap();
        assert_eq!(
            read_key_file_capped(&path, "~/.ssh/id_rsa").unwrap(),
            "PEM-CONTENT"
        );
    }

    #[test]
    fn read_key_file_capped_rejects_oversized_file() {
        // One byte past the 1 MiB cap — too big to be a private key, so it must
        // be rejected before it is slurped into memory. Mirrors the webview
        // pick-file path's guard so both key-import routes behave identically.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id_rsa");
        std::fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        let err = read_key_file_capped(&path, "~/.ssh/id_rsa").unwrap_err();
        assert_eq!(err.code(), "key_file_too_large");
    }

    #[test]
    fn read_key_file_capped_reports_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id_rsa");
        let err = read_key_file_capped(&path, "~/.ssh/id_rsa").unwrap_err();
        assert_eq!(err.code(), "key_file_not_found");
    }
}
