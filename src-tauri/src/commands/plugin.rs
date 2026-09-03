//! Third-party plugin packages: install/list/uninstall/enable + the one-shot
//! exec channel that is a plugin's ONLY way to interact with the host.
//!
//! A package is a zip of `{ manifest.json, index.html, assets/** }`. The UI is
//! hosted in a sandboxed iframe (see frontend PluginFrame); this module only
//! owns registration, safe on-disk extraction, and `plugin_exec`.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde_json::json;
use tauri::State;

use crate::error::{locked, AppError, AppResult};
use crate::models::{Plugin, PluginExecResult};
use crate::ssh::client::{self, SshHandle};
use crate::state::{AppState, SessionKind, SessionOwner, SessionPhase};

// ── Package limits ──────────────────────────────────────────────────────────
// Generous for UI bundles, tight enough to make zip bombs boring.

/// Compressed payload accepted from the webview (base64-decoded length).
const MAX_ZIP_BYTES: usize = 10 * 1024 * 1024;
/// Entry count cap — a UI bundle is dozens of files, not thousands.
const MAX_ENTRIES: usize = 500;
/// Per-file uncompressed cap; enforced via `take()` on the reader, not the
/// (spoofable) declared size in the zip header.
const MAX_FILE_UNCOMPRESSED: u64 = 32 * 1024 * 1024;
/// Total uncompressed cap across the archive.
const MAX_TOTAL_UNCOMPRESSED: u64 = 64 * 1024 * 1024;

// ── Exec limits ─────────────────────────────────────────────────────────────

const MAX_COMMAND_LEN: usize = 4096;
const DEFAULT_TIMEOUT_MS: u64 = 10_000;
const MIN_TIMEOUT_MS: u64 = 1_000;
const MAX_TIMEOUT_MS: u64 = 60_000;

/// Absolute path of the on-disk plugin store: `<data_dir>/plugins`.
pub fn plugins_dir(state: &AppState) -> PathBuf {
    state.data_dir.join("plugins")
}

/// Serializes plugin installs/uninstalls across entrypoints (Tauri commands
/// and the ws server): the directory swap and the registry upsert must not
/// interleave for the same id. Plain mutex — no path holds it across an await.
static PLUGIN_OPS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_plugin_ops() -> std::sync::MutexGuard<'static, ()> {
    // A poisoned lock only means an earlier operation panicked mid-way; the
    // next one should still run rather than brick plugin management.
    PLUGIN_OPS.lock().unwrap_or_else(|e| e.into_inner())
}

// ── Manifest ────────────────────────────────────────────────────────────────

/// `manifest.json` inside a plugin zip, before it becomes a DB `Plugin` row.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    /// Host region: "side" | "strip".
    pub area: String,
    /// Optional package-relative path to a preview document shown on the
    /// manager page (e.g. "preview.html").
    #[serde(default)]
    pub preview: String,
    /// Bridge protocol version the plugin speaks.
    pub api: u32,
}

/// Plugin id doubles as the install directory name — a strict lowercase slug
/// is the whole path-safety story for `plugins/<id>`.
pub fn valid_plugin_id(id: &str) -> bool {
    (2..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

/// Field-level validation; every rejection names the offending field.
pub fn validate_manifest(m: &PluginManifest) -> AppResult<()> {
    let bad = |field: &str, reason: &str| {
        AppError::config(
            "plugin_manifest_invalid",
            json!({ "field": field, "reason": reason }),
        )
    };
    if !valid_plugin_id(&m.id) {
        return Err(bad(
            "id",
            "expected 2-64 chars of [a-z0-9-], no leading/trailing '-'",
        ));
    }
    let text_field = |v: &str, field: &'static str, max: usize| -> AppResult<()> {
        let t = v.trim();
        if t.is_empty() {
            return Err(bad(field, "must not be empty"));
        }
        if t.chars().count() > max {
            return Err(bad(field, &format!("too long (max {max} chars)")));
        }
        Ok(())
    };
    text_field(&m.name, "name", 100)?;
    text_field(&m.version, "version", 32)?;
    if m.description.chars().count() > 2000 {
        return Err(bad("description", "too long (max 2000 chars)"));
    }
    if m.author.chars().count() > 100 {
        return Err(bad("author", "too long (max 100 chars)"));
    }
    if m.area != "side" && m.area != "strip" {
        return Err(bad("area", "expected \"side\" or \"strip\""));
    }
    if !m.preview.is_empty() && (m.preview.chars().count() > 200 || !safe_entry_name(&m.preview)) {
        return Err(bad(
            "preview",
            "expected a relative path inside the package",
        ));
    }
    if m.api != 1 {
        return Err(bad(
            "api",
            "unsupported protocol version (this host speaks 1)",
        ));
    }
    Ok(())
}

// ── Safe extraction ─────────────────────────────────────────────────────────

/// Entry names must be relative, backslash-free, and free of `..`/`.`/prefix
/// components — anything else is a zip-slip attempt.
fn safe_entry_name(name: &str) -> bool {
    if name.is_empty() || name.contains('\\') {
        return false;
    }
    let path = Path::new(name);
    path.is_relative()
        && !path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::CurDir | Component::Prefix(_)
            )
        })
}

/// A symlink inside a plugin dir could point anywhere on disk; the iframe only
/// reads today, but future capabilities may not. zip's own S_IFLNK detection.
fn entry_is_symlink(entry: &zip::read::ZipFile<'_>) -> bool {
    entry.is_symlink()
}

fn read_entry(
    archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    name: &str,
) -> AppResult<Vec<u8>> {
    let entry = archive
        .by_name(name)
        .map_err(|e| zip_err("read_entry", e))?;
    let mut buf = Vec::new();
    entry
        .take(MAX_FILE_UNCOMPRESSED + 1)
        .read_to_end(&mut buf)?;
    Ok(buf)
}

/// macOS Finder zips carry `__MACOSX/` metadata and `.DS_Store` noise; skip.
fn is_junk_entry(name: &str) -> bool {
    name.starts_with("__MACOSX/") || name.ends_with("/.DS_Store") || name == ".DS_Store"
}

fn zip_err(op: &'static str, e: zip::result::ZipError) -> AppError {
    AppError::config(
        "plugin_zip_invalid",
        json!({ "op": op, "err": e.to_string() }),
    )
}

/// Extract `archive` into `dest` with all caps enforced. Rejecting one bad
/// entry rejects the whole install — a half-trusted bundle is worse than none.
pub fn extract_plugin_zip(
    archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    dest: &Path,
) -> AppResult<()> {
    if archive.len() > MAX_ENTRIES {
        return Err(AppError::config(
            "plugin_zip_invalid",
            json!({ "op": "entries", "count": archive.len() }),
        ));
    }
    std::fs::create_dir_all(dest)?;
    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| zip_err("read_entry", e))?;
        let name = entry.name().to_owned();
        if is_junk_entry(&name) {
            continue;
        }
        if !safe_entry_name(&name) {
            return Err(AppError::config(
                "plugin_zip_invalid",
                json!({ "op": "entry_name", "name": name }),
            ));
        }
        if entry_is_symlink(&entry) {
            return Err(AppError::config(
                "plugin_zip_invalid",
                json!({ "op": "symlink", "name": name }),
            ));
        }
        let out_path = dest.join(&name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // `take` caps the actual bytes read; the header's declared size is
        // advisory from an attacker's point of view.
        let mut buf = Vec::new();
        entry
            .take(MAX_FILE_UNCOMPRESSED + 1)
            .read_to_end(&mut buf)?;
        if buf.len() as u64 > MAX_FILE_UNCOMPRESSED {
            return Err(AppError::config(
                "plugin_zip_invalid",
                json!({ "op": "file_too_large", "name": name }),
            ));
        }
        total += buf.len() as u64;
        if total > MAX_TOTAL_UNCOMPRESSED {
            return Err(AppError::config(
                "plugin_zip_invalid",
                json!({ "op": "total_too_large", "name": name }),
            ));
        }
        std::fs::write(&out_path, &buf)?;
    }
    Ok(())
}

/// Install (or upgrade) a plugin from raw zip bytes. `expected_area` is the
/// region the install was triggered from ("side"/"strip"): a plugin whose
/// manifest declares the other area is rejected before anything touches disk.
/// Extraction goes to a staging dir first; only a fully valid package
/// replaces the install dir.
pub fn install_impl(
    state: &AppState,
    zip_bytes: &[u8],
    expected_area: Option<&str>,
) -> AppResult<Plugin> {
    let _ops = lock_plugin_ops();
    if zip_bytes.len() > MAX_ZIP_BYTES {
        return Err(AppError::config(
            "plugin_zip_invalid",
            json!({ "op": "zip_too_large", "size": zip_bytes.len() }),
        ));
    }
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| zip_err("open", e))?;

    let manifest_bytes = read_entry(&mut archive, "manifest.json")?;
    let manifest: PluginManifest = serde_json::from_slice(&manifest_bytes).map_err(|e| {
        AppError::config(
            "plugin_manifest_invalid",
            json!({ "field": "manifest.json", "reason": e.to_string() }),
        )
    })?;
    validate_manifest(&manifest)?;
    if let Some(expected) = expected_area {
        if manifest.area != expected {
            return Err(AppError::config(
                "plugin_area_mismatch",
                json!({ "expected": expected, "actual": manifest.area }),
            ));
        }
    }
    if !entry_exists(&mut archive, "index.html") {
        return Err(AppError::config(
            "plugin_manifest_invalid",
            json!({ "field": "index.html", "reason": "missing from package" }),
        ));
    }
    if !manifest.preview.is_empty() && !entry_exists(&mut archive, &manifest.preview) {
        return Err(AppError::config(
            "plugin_manifest_invalid",
            json!({ "field": "preview", "reason": "declared but missing from package" }),
        ));
    }

    let root = plugins_dir(state);
    std::fs::create_dir_all(&root)?;
    let staging = root.join(format!(".staging-{}", uuid::Uuid::new_v4()));
    if let Err(e) = extract_plugin_zip(&mut archive, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    let final_dir = root.join(&manifest.id);
    if final_dir.exists() {
        std::fs::remove_dir_all(&final_dir)?;
    }
    std::fs::rename(&staging, &final_dir)?;

    let plugin = Plugin {
        id: manifest.id,
        name: manifest.name.trim().to_owned(),
        version: manifest.version.trim().to_owned(),
        description: manifest.description.trim().to_owned(),
        author: manifest.author.trim().to_owned(),
        area: manifest.area,
        preview: manifest.preview.trim().to_owned(),
        enabled: true,
        installed_at: chrono::Utc::now().timestamp(),
        // New installs append to the end of their area; upgrades keep the
        // stored sort_order (upsert does not touch it).
        sort_order: crate::db::plugin::next_sort_order(&state.db)?,
    };
    crate::db::plugin::upsert(&state.db, &plugin)?;
    // Return the DB row, not the local struct: an upgrade preserves the old
    // `enabled` (upsert semantics), and the caller must see that truth.
    crate::db::plugin::get(&state.db, &plugin.id)?.ok_or_else(|| {
        AppError::other(
            "session_registry_inconsistent",
            json!({ "op": "plugin_upsert_vanished", "id": plugin.id }),
        )
    })
}

fn entry_exists(archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>, name: &str) -> bool {
    archive.by_name(name).is_ok()
}

// ── Session lookup for exec ─────────────────────────────────────────────────

/// Where a plugin command runs: an SSH exec channel on the tab's connection,
/// or a local child process for local-shell tabs. Telnet/serial tabs have
/// neither — the plugin capability contract covers SSH + local only.
enum ExecTransport {
    Ssh(SshHandle),
    #[cfg(desktop)]
    Local,
}

fn exec_transport(
    state: &AppState,
    session_id: &str,
    requester: &SessionOwner,
) -> AppResult<ExecTransport> {
    let registry = locked(&state.lifecycle_sessions)?;
    let record = registry
        .get(session_id)
        .ok_or_else(|| AppError::not_found("plugin_no_exec", json!({ "id": session_id })))?;
    if record.phase != SessionPhase::Ready {
        return Err(AppError::not_found(
            "plugin_no_exec",
            json!({ "id": session_id, "kind": format!("{:?}", record.kind) }),
        ));
    }
    if &record.owner != requester {
        return Err(AppError::config(
            "session_owner_mismatch",
            json!({ "id": session_id }),
        ));
    }
    match record.kind {
        SessionKind::Ssh => {
            let handle = locked(&state.sessions)?
                .get(session_id)
                .map(|h| h.ssh_handle().clone())
                .ok_or_else(|| {
                    AppError::other("session_registry_inconsistent", json!({ "id": session_id }))
                })?;
            Ok(ExecTransport::Ssh(handle))
        }
        // Local shell tab: run on this machine. The PTY handle itself is not
        // needed — a fresh child process per call, same one-shot contract.
        #[cfg(desktop)]
        SessionKind::Pty => Ok(ExecTransport::Local),
        kind => Err(AppError::not_found(
            "plugin_no_exec",
            json!({ "id": session_id, "kind": format!("{kind:?}") }),
        )),
    }
}

/// One-shot local command, mirroring `ssh::client::exec_once` semantics
/// (timeout, 256 KB per stream). The timeout path kills the WHOLE process
/// group: `kill_on_drop` takes out the shell and a group sweep takes out its
/// children — no orphaned `sh -c "cat /dev/zero & wait"` burners.
#[cfg(desktop)]
async fn local_exec(command: &str, timeout: std::time::Duration) -> AppResult<PluginExecResult> {
    const CAP: u64 = 256 * 1024;

    #[cfg(unix)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("/bin/sh");
        c.arg("-c");
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C");
        c
    };
    // Own process group: the timeout kill must reach the shell's children,
    // not just the shell. Windows TerminateProcess reaches only the direct
    // child — grandchildren are left to the OS (job objects are overkill
    // for v1).
    #[cfg(unix)]
    cmd.process_group(0);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .arg(command);
    let mut child = cmd.spawn().map_err(|e| {
        AppError::other("plugin_exec_spawn_failed", json!({ "err": e.to_string() }))
    })?;
    // The child leads its own group, so the group id equals the child pid.
    #[cfg(unix)]
    let group_id = child.id();
    let out_pipe = child.stdout.take();
    let err_pipe = child.stderr.take();

    async fn read_capped<R: tokio::io::AsyncRead + Unpin>(pipe: Option<R>) -> Vec<u8> {
        use tokio::io::AsyncReadExt as _;
        let mut buf = Vec::new();
        if let Some(mut r) = pipe {
            // Retain the first CAP bytes, then keep DRAINING to EOF: closing
            // the read end early would break the pipe and the child would die
            // to SIGPIPE mid-write, corrupting its exit code — the SSH path
            // consumes the channel to EOF for the same reason.
            let mut chunk = [0u8; 8192];
            loop {
                match r.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let room = (CAP as usize).saturating_sub(buf.len());
                        if room > 0 {
                            buf.extend_from_slice(&chunk[..n.min(room)]);
                        }
                    }
                }
            }
        }
        buf
    }

    let collect = async {
        let (out, err) = tokio::join!(read_capped(out_pipe), read_capped(err_pipe));
        let status = child.wait().await;
        (out, err, status)
    };
    match tokio::time::timeout(timeout, collect).await {
        Ok((out, err, status)) => Ok(PluginExecResult {
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
            exit_code: status.ok().and_then(|s| s.code()),
        }),
        Err(_) => {
            // Dropping `collect` already killed the shell (kill_on_drop);
            // the group sweep finishes off its children. ESRCH here just
            // means the group is already gone.
            #[cfg(unix)]
            if let Some(pid) = group_id {
                unsafe {
                    let _ = libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                }
            }
            Err(AppError::other(
                "plugin_exec_timeout",
                json!({ "millis": timeout.as_millis() as u64 }),
            ))
        }
    }
}

/// Shared exec core (Tauri command + headless server dispatch both call this).
pub async fn plugin_exec_impl(
    state: &AppState,
    owner: &SessionOwner,
    session_id: String,
    command: String,
    timeout_ms: Option<u64>,
) -> AppResult<PluginExecResult> {
    if command.len() > MAX_COMMAND_LEN {
        return Err(AppError::config(
            "plugin_exec_command_too_long",
            json!({ "len": command.len() }),
        ));
    }
    let timeout = std::time::Duration::from_millis(
        timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS),
    );
    match exec_transport(state, &session_id, owner)? {
        ExecTransport::Ssh(handle) => {
            let ssh = handle.clone();
            let cmd = command;
            client::run_blocking_ssh(
                move || async move { client::exec_once(&ssh, &cmd, timeout).await },
            )
            .await
        }
        #[cfg(desktop)]
        ExecTransport::Local => local_exec(&command, timeout).await,
    }
}

// ── Tauri commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn plugins_root(state: State<'_, AppState>) -> AppResult<String> {
    Ok(plugins_dir(&state).to_string_lossy().into_owned())
}

/// Reject encoded input that could decode beyond MAX_ZIP_BYTES — decoding
/// first would allocate the whole payload in memory. Call before decode.
pub fn ensure_zip_b64_within_cap(encoded: &str) -> AppResult<()> {
    // Decoded upper bound: 3 bytes per full 4-char group plus the 1-2 bytes
    // of a trailing partial group (padding decodes to nothing). Exact, so it
    // agrees with install_impl's check on the decoded bytes.
    let len = encoded.trim().len();
    let decoded = len / 4 * 3
        + match len % 4 {
            2 => 1,
            3 => 2,
            _ => 0,
        };
    if decoded > MAX_ZIP_BYTES {
        return Err(AppError::config(
            "plugin_zip_invalid",
            json!({ "op": "zip_too_large", "size": len }),
        ));
    }
    Ok(())
}

#[tauri::command]
pub fn install_plugin(
    state: State<'_, AppState>,
    base64_zip: String,
    area: Option<String>,
) -> AppResult<Plugin> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    ensure_zip_b64_within_cap(&base64_zip)?;
    let bytes = STANDARD.decode(base64_zip.trim()).map_err(|e| {
        AppError::config(
            "crypto_base64_decode_failed",
            json!({ "err": e.to_string() }),
        )
    })?;
    install_impl(&state, &bytes, area.as_deref())
}

#[tauri::command]
pub fn list_plugins(state: State<'_, AppState>) -> AppResult<Vec<Plugin>> {
    crate::db::plugin::list(&state.db)
}

#[tauri::command]
pub fn set_plugin_enabled(state: State<'_, AppState>, id: String, enabled: bool) -> AppResult<()> {
    crate::db::plugin::set_enabled(&state.db, &id, enabled)
}

/// Rewrite one area's order from the manager page (full ordered id list).
#[tauri::command]
pub fn set_plugin_order(state: State<'_, AppState>, ids: Vec<String>) -> AppResult<()> {
    for id in &ids {
        if !valid_plugin_id(id) {
            return Err(AppError::config(
                "plugin_manifest_invalid",
                json!({ "field": "id", "reason": "not a plugin id" }),
            ));
        }
    }
    crate::db::plugin::set_order(&state.db, &ids)
}

/// Uninstall shared by the Tauri command and the ws server.
pub fn uninstall_impl(state: &AppState, id: &str) -> AppResult<()> {
    if !valid_plugin_id(id) {
        return Err(AppError::config(
            "plugin_manifest_invalid",
            json!({ "field": "id", "reason": "not a plugin id" }),
        ));
    }
    let _ops = lock_plugin_ops();
    // Registry row first, files second: a failure after the DB delete leaves
    // an orphan directory (harmless — reinstall overwrites it), while the
    // reverse order can leave a row pointing at removed files.
    crate::db::plugin::delete(&state.db, id)?;
    // Remove unconditionally — an exists() probe would swallow real errors
    // and races another uninstall's removal; a missing dir is plain success.
    if let Err(e) = std::fs::remove_dir_all(plugins_dir(state).join(id)) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e.into());
        }
    }
    Ok(())
}

#[tauri::command]
pub fn uninstall_plugin(state: State<'_, AppState>, id: String) -> AppResult<()> {
    uninstall_impl(&state, &id)
}

#[tauri::command]
pub async fn plugin_exec(
    window: tauri::Window,
    state: State<'_, AppState>,
    session_id: String,
    command: String,
    timeout_ms: Option<u64>,
) -> AppResult<PluginExecResult> {
    plugin_exec_impl(
        &state,
        &SessionOwner::Window(window.label().to_owned()),
        session_id,
        command,
        timeout_ms,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn build_zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in files {
            w.start_file((*name).to_owned(), zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        let cursor = w.finish().unwrap();
        cursor.into_inner()
    }

    /// zip 2.x's writer masks permissions to 0o777, so a symlink entry can
    /// only be produced through the dedicated API — same as a hostile packager.
    fn build_zip_with_symlink() -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        w.add_symlink(
            "link",
            "/etc/passwd",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        let cursor = w.finish().unwrap();
        cursor.into_inner()
    }

    fn manifest_json(id: &str, area: &str) -> String {
        format!(
            r#"{{"id":"{id}","name":"Mon","version":"1.0.0","description":"d","author":"a","area":"{area}","api":1}}"#
        )
    }

    fn open(bytes: &[u8]) -> zip::ZipArchive<std::io::Cursor<&[u8]>> {
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap()
    }

    #[cfg(desktop)]
    #[tokio::test]
    async fn local_exec_runs_command_and_captures_output() {
        // Both shells must chain stdout, stderr and a nonzero exit code.
        let command = if cfg!(windows) {
            "echo hi& echo oops 1>&2 & exit 7"
        } else {
            "echo hi; echo oops 1>&2; exit 7"
        };
        let result = local_exec(command, std::time::Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(result.stdout.trim(), "hi");
        assert_eq!(result.stderr.trim(), "oops");
        assert_eq!(result.exit_code, Some(7));
    }

    #[cfg(desktop)]
    #[tokio::test]
    async fn local_exec_times_out_and_kills() {
        // `sleep` doesn't exist on Windows; ping-to-localhost is the standard
        // blocking stand-in (~1s per -n unit).
        let command = if cfg!(windows) {
            "ping -n 31 127.0.0.1 >nul"
        } else {
            "sleep 30"
        };
        let start = std::time::Instant::now();
        let err = local_exec(command, std::time::Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "plugin_exec_timeout");
        // kill_on_drop fired — we must not have waited for the sleeper.
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    #[cfg(all(desktop, unix))]
    #[tokio::test]
    async fn local_exec_timeout_kills_the_process_group() {
        // `sh -c "sleep N & wait"` parks the shell behind a background child;
        // the timeout kill must reap the whole group, not just the shell.
        // 9876s is a unique duration so the ps scan can't false-positive.
        let command = "sleep 9876 & wait";
        let err = local_exec(command, std::time::Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "plugin_exec_timeout");
        // SIGKILL delivery is asynchronous — give the sweep a beat to land.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let out = std::process::Command::new("ps")
            .args(["-eo", "args"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!text.contains("sleep 9876"), "orphan survived:\n{text}");
    }

    #[cfg(desktop)]
    #[tokio::test]
    async fn local_exec_caps_output_without_breaking_the_pipe() {
        // >256 KB of stdout: the retained buffer is capped, but the pipe keeps
        // draining so the writer finishes normally — a broken read end would
        // kill it with SIGPIPE (exit 141) instead of a clean truncation.
        let command = if cfg!(windows) {
            "for /L %i in (1,1,30000) do @echo 0123456789012345678901234567890"
        } else {
            "head -c 300000 /dev/zero"
        };
        let result = local_exec(command, std::time::Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(result.stdout.len(), 256 * 1024);
        assert_eq!(result.exit_code, Some(0));
    }

    #[test]
    fn preview_declared_but_missing_is_rejected() {
        let state = test_state();
        let manifest = manifest_json("mon", "side")
            .replace("\"api\":1", "\"preview\":\"preview.html\",\"api\":1");
        let bytes = build_zip(&[("manifest.json", &manifest), ("index.html", "x")]);
        let err = install_impl(&state, &bytes, None).unwrap_err();
        assert_eq!(err.code(), "plugin_manifest_invalid");

        // Declared AND present → install succeeds and the row carries it.
        let bytes = build_zip(&[
            ("manifest.json", &manifest),
            ("index.html", "x"),
            ("preview.html", "<html></html>"),
        ]);
        let plugin = install_impl(&state, &bytes, None).unwrap();
        assert_eq!(plugin.preview, "preview.html");
    }

    #[test]
    fn plugin_id_rules() {
        assert!(valid_plugin_id("mon"));
        assert!(valid_plugin_id("rssh-plugin-monitor"));
        assert!(!valid_plugin_id("a"));
        assert!(!valid_plugin_id("-mon"));
        assert!(!valid_plugin_id("Mon"));
        assert!(!valid_plugin_id("mon_x"));
        assert!(!valid_plugin_id("../evil"));
        assert!(!valid_plugin_id(""));
    }

    #[test]
    fn manifest_validation_rejects_bad_area_and_api() {
        let parse = |s: &str| serde_json::from_str::<PluginManifest>(s).unwrap();
        let ok = parse(&manifest_json("mon", "side"));
        assert!(validate_manifest(&ok).is_ok());

        let bad_area = parse(&manifest_json("mon", "corner"));
        assert!(validate_manifest(&bad_area).is_err());

        let bad_api = parse(&manifest_json("mon", "side").replace("\"api\":1", "\"api\":2"));
        assert!(validate_manifest(&bad_api).is_err());

        let no_name = serde_json::from_str::<PluginManifest>(
            r#"{"id":"mon","name":"","version":"1","area":"side","api":1}"#,
        )
        .unwrap();
        assert!(validate_manifest(&no_name).is_err());
    }

    #[test]
    fn extraction_copies_files() {
        let bytes = build_zip(&[
            ("manifest.json", &manifest_json("mon", "side")),
            ("index.html", "<html></html>"),
            ("assets/app.js", "console.log(1)"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        extract_plugin_zip(&mut open(&bytes), dir.path()).unwrap();
        assert!(dir.path().join("index.html").exists());
        assert!(dir.path().join("assets/app.js").exists());
    }

    #[test]
    fn extraction_rejects_parent_dir_escape() {
        for name in ["../evil.txt", "a/../../evil.txt"] {
            let bytes = build_zip(&[(name, "x")]);
            let dir = tempfile::tempdir().unwrap();
            let err = extract_plugin_zip(&mut open(&bytes), dir.path()).unwrap_err();
            assert_eq!(err.code(), "plugin_zip_invalid");
            // Nothing may land outside dest — or inside it either.
            assert!(!dir.path().parent().unwrap().join("evil.txt").exists());
        }
    }

    #[test]
    fn extraction_rejects_symlink_entries() {
        let bytes = build_zip_with_symlink();
        let dir = tempfile::tempdir().unwrap();
        let err = extract_plugin_zip(&mut open(&bytes), dir.path()).unwrap_err();
        assert_eq!(err.code(), "plugin_zip_invalid");
    }

    #[test]
    fn extraction_skips_macos_junk() {
        let bytes = build_zip(&[("__MACOSX/meta", "junk"), ("index.html", "ok")]);
        let dir = tempfile::tempdir().unwrap();
        extract_plugin_zip(&mut open(&bytes), dir.path()).unwrap();
        assert!(!dir.path().join("__MACOSX").exists());
        assert!(dir.path().join("index.html").exists());
    }

    #[test]
    fn install_impl_rejects_zip_without_manifest() {
        let state = test_state();
        let bytes = build_zip(&[("index.html", "<html></html>")]);
        let err = install_impl(&state, &bytes, None).unwrap_err();
        assert_eq!(err.code(), "plugin_zip_invalid");
    }

    #[test]
    fn install_impl_round_trip_and_uninstall_dir() {
        let state = test_state();
        let bytes = build_zip(&[
            ("manifest.json", &manifest_json("demo-mon", "side")),
            ("index.html", "<html>ok</html>"),
        ]);
        let plugin = install_impl(&state, &bytes, None).unwrap();
        assert_eq!(plugin.id, "demo-mon");
        assert_eq!(plugin.area, "side");
        let dir = plugins_dir(&state).join("demo-mon");
        assert!(dir.join("index.html").exists());

        let listed = crate::db::plugin::list(&state.db).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "demo-mon");

        std::fs::remove_dir_all(&dir).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn install_impl_upgrades_same_id() {
        let state = test_state();
        let v1 = build_zip(&[
            ("manifest.json", &manifest_json("demo-mon", "side")),
            ("index.html", "v1"),
        ]);
        install_impl(&state, &v1, None).unwrap();
        crate::db::plugin::set_enabled(&state.db, "demo-mon", false).unwrap();

        let v2 = build_zip(&[
            (
                "manifest.json",
                &manifest_json("demo-mon", "side").replace("1.0.0", "2.0.0"),
            ),
            ("index.html", "v2"),
        ]);
        let upgraded = install_impl(&state, &v2, None).unwrap();
        assert_eq!(upgraded.version, "2.0.0");
        // Upgrade keeps the user's disabled state (db upsert semantics).
        assert!(!upgraded.enabled);
        let dir = plugins_dir(&state).join("demo-mon");
        let html = std::fs::read_to_string(dir.join("index.html")).unwrap();
        assert_eq!(html, "v2");
    }

    #[test]
    fn install_impl_rejects_area_mismatch() {
        let state = test_state();
        // A side package offered to the strip region's install entry: rejected
        // before extraction — nothing lands on disk or in the DB.
        let bytes = build_zip(&[
            ("manifest.json", &manifest_json("demo-mon", "side")),
            ("index.html", "x"),
        ]);
        let err = install_impl(&state, &bytes, Some("strip")).unwrap_err();
        assert_eq!(err.code(), "plugin_area_mismatch");
        assert!(crate::db::plugin::list(&state.db).unwrap().is_empty());
        assert!(!plugins_dir(&state).join("demo-mon").exists());

        // Matching area installs fine.
        install_impl(&state, &bytes, Some("side")).unwrap();
        assert_eq!(crate::db::plugin::list(&state.db).unwrap().len(), 1);
    }

    /// AppState without a real Tauri app: enough fields for install_impl.
    /// Same construction as lifecycle.rs's `empty_state`, but with a real
    /// tempdir so extracted files land somewhere inspectable.
    fn test_state() -> AppState {
        let db = std::sync::Arc::new(crate::db::Db::open_in_memory().unwrap());
        let secret_store: std::sync::Arc<dyn crate::secret::SecretStore> =
            std::sync::Arc::new(crate::secret::DbStore::new(db.clone()));
        AppState {
            db,
            secret_store,
            lifecycle_sessions: Default::default(),
            sessions: Default::default(),
            #[cfg(desktop)]
            pty_sessions: Default::default(),
            #[cfg(desktop)]
            serial_sessions: Default::default(),
            telnet_sessions: Default::default(),
            sftp_sessions: Default::default(),
            transfer_cancels: Default::default(),
            active_forwards: Default::default(),
            auth_waiters: Default::default(),
            passphrase_waiters: Default::default(),
            host_key_waiters: Default::default(),
            passphrase_cache: Default::default(),
            ai_sessions: Default::default(),
            ai_session_owners: Default::default(),
            ai_remote_shell_cache: Default::default(),
            data_dir: tempfile::tempdir().unwrap().keep(),
        }
    }
}
