use serde_json::json;
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::models::{Credential, Profile, SshAlgorithmCatalog};
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
