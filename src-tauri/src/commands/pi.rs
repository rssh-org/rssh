//! Pi coding-agent integration commands.
//!
//! The frontend opens a "Pi 会话" tab attached to an active SSH session; the
//! backend spawns a persistent `pi --mode rpc` on that connection and streams
//! its JSONL events back to the tab. Sessions live server-side under
//! `~/.pi/agent/sessions/`, so the same session continues from any machine that
//! connects to the same host.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, State};

use crate::error::{locked, AppError, AppResult};
use crate::emitter::Host;
use crate::ssh::client::{exec_once, SessionHandle};
use crate::ssh::pi::{self, PiHandle};
use crate::state::AppState;

fn get_ssh(state: &State<'_, AppState>, session_id: &str) -> AppResult<SessionHandle> {
    locked(&state.sessions)?
        .get(session_id)
        .cloned()
        .ok_or_else(|| AppError::not_found("session_not_found", json!({})))
}

fn get_pi(state: &State<'_, AppState>, pi_id: &str) -> AppResult<Arc<PiHandle>> {
    locked(&state.pi_sessions)?
        .get(pi_id)
        .cloned()
        .ok_or_else(|| AppError::not_found("pi_session_not_found", json!({})))
}

/// Start a persistent `pi --mode rpc` on an already-active SSH session.
///
/// `session_path` resumes a *specific* server-side session file (most reliable
/// pick); `continue_session` resumes the most recent one for the remote working
/// directory; otherwise a fresh session is started. Returns the pi session id
/// used for `pi_session_send` / `pi_session_stop` and for the
/// `pi:line:{id}` / `pi:status:{id}` event channels.
#[tauri::command]
pub async fn pi_session_start(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    continue_session: Option<bool>,
    name: Option<String>,
    session_path: Option<String>,
) -> AppResult<String> {
    let ssh = get_ssh(&state, &session_id)?;
    let pi_id = format!("pi-{}", uuid::Uuid::new_v4().simple());

    let mut extra = Vec::new();
    if let Some(p) = session_path {
        let p = p.trim();
        if !p.is_empty() {
            extra.push("--session".to_string());
            extra.push(p.to_string());
        }
    } else if continue_session.unwrap_or(false) {
        extra.push("--continue".to_string());
    }
    if let Some(n) = name {
        let n = n.trim();
        if !n.is_empty() {
            extra.push("--name".to_string());
            extra.push(n.to_string());
        }
    }

    let handle = pi::spawn(
        Host::Tauri(app),
        pi_id.clone(),
        ssh.ssh_handle(),
        ssh.profile_id().to_owned(),
        session_id,
        extra,
    )
    .await?;

    locked(&state.pi_sessions)?.insert(pi_id.clone(), Arc::new(handle));
    Ok(pi_id)
}

/// Send one JSONL command line to the pi subprocess stdin.
#[tauri::command]
pub async fn pi_session_send(
    state: State<'_, AppState>,
    pi_id: String,
    line: String,
) -> AppResult<()> {
    get_pi(&state, &pi_id)?.send_line(&line)
}

/// Stop the pi subprocess and remove it from the registry.
#[tauri::command]
pub async fn pi_session_stop(state: State<'_, AppState>, pi_id: String) -> AppResult<()> {
    if let Some(h) = locked(&state.pi_sessions)?.remove(&pi_id) {
        h.stop();
    }
    Ok(())
}

#[derive(Clone, Serialize)]
pub struct PiSessionInfo {
    pub path: String,
    /// 会话文件时间戳（无 .jsonl 后缀）
    pub name: String,
    /// 会话的工作目录（从会话文件 header 的 cwd 字段读，可能为空）
    pub cwd: String,
    /// 会话第一条用户消息（截断后），用作列表里的"标题"帮助区分
    pub title: String,
}

/// List server-side pi sessions (most recent first). Each entry carries the
/// session file path, its timestamp, the working directory the session ran in,
/// and the session's first user message (as a short title) so the user can
/// pick which one to resume. `\x1f` is the field separator (survives `|` in
/// messages).
#[tauri::command]
pub async fn pi_session_list(
    state: State<'_, AppState>,
    session_id: String,
) -> AppResult<Vec<PiSessionInfo>> {
    let ssh = get_ssh(&state, &session_id)?;
    let script = r#"for f in $(ls -t ~/.pi/agent/sessions/*/*.jsonl 2>/dev/null | head -80); do c=$(head -1 "$f" | grep -m1 -oE '"cwd":"[^"]*"' | head -1); t=$(head -80 "$f" | grep -m1 '"role":"user"' | grep -m1 -oP '(?<="text":")[^"]*' | head -1); echo -e "$f\x1f$c\x1f$t"; done"#;
    let out = exec_once(
        ssh.ssh_handle(),
        script,
        Duration::from_secs(20),
    )
    .await?;

    let mut list = Vec::new();
    for line in out.stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\x1f').collect();
        let path = parts[0].trim();
        if path.is_empty() || !path.ends_with(".jsonl") {
            continue;
        }
        let cwd = parts
            .get(1)
            .unwrap_or(&"")
            .trim()
            .trim_start_matches("\"cwd\":\"")
            .trim_end_matches('"')
            .to_string();
        let mut title = parts.get(2).unwrap_or(&"").trim().to_string();
        title = title
            .replace("\\n", " ")
            .replace("\\r", " ")
            .replace("\\t", " ")
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
        if title.chars().count() > 60 {
            title = title.chars().take(60).collect::<String>() + "…";
        }
        let base = path.rsplit('/').next().unwrap_or(path);
        let name = base.trim_end_matches(".jsonl").to_string();
        list.push(PiSessionInfo {
            path: path.to_string(),
            name,
            cwd,
            title,
        });
    }
    Ok(list)
}

/// Delete a server-side pi session file. The path is validated against
/// `~/.pi/agent/sessions/` + `.jsonl` and stripped of shell metacharacters
/// before it reaches the remote, so a session file can never turn into an
/// arbitrary remote command.
#[tauri::command]
pub async fn pi_session_delete(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> AppResult<()> {
    let p = path.trim();
    if !p.starts_with("~/.pi/agent/sessions/") || !p.ends_with(".jsonl") {
        return Err(AppError::other("invalid_session_path", json!({ "path": p })));
    }
    if p.contains(';')
        || p.contains('|')
        || p.contains('&')
        || p.contains('$')
        || p.contains('`')
        || p.contains('\'')
        || p.contains('"')
        || p.contains(' ')
    {
        return Err(AppError::other("invalid_session_path", json!({ "path": p })));
    }
    let ssh = get_ssh(&state, &session_id)?;
    exec_once(
        ssh.ssh_handle(),
        &format!("rm -f '{p}'"),
        Duration::from_secs(10),
    )
    .await?;
    Ok(())
}
