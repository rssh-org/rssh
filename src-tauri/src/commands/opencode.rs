//! OpenCode coding-agent integration commands.
//!
//! Unlike pi (stdio RPC over an SSH exec channel), opencode is an HTTP
//! server. rssh starts a headless `opencode serve` on the remote host
//! (loopback + Basic Auth), creates a local port-forward to it, and talks
//! to the OpenAPI (https://opencode.ai/docs/server) over HTTP:
//!   - GET    /session                     list sessions
//!   - POST   /session                     create a session
//!   - DELETE /session/:id                 delete a session
//!   - GET    /session/:id/message         message history
//!   - POST   /session/:id/prompt_async    send a prompt (events stream via /event)
//!   - GET    /event                       global SSE event stream
//! Sessions persist server-side in opencode.db, so another machine connecting
//! to the same host continues the same conversation (TRAE / openchamber style).
//!
//! Auth: `opencode serve` reads `OPENCODE_SERVER_PASSWORD`; the username is
//! fixed to `opencode` (HTTP Basic). rssh persists the password on the remote
//! host at `~/.config/rssh-opencode/pass` (0600) so a serve that is already
//! running (e.g. started by a previous rssh instance) can be reused.

use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, State, Window};

use crate::emitter::Host;
use crate::error::{locked, AppError, AppResult};
use crate::ssh::client::{exec_once, SessionHandle};
use crate::state::{AppState, SessionKind, SessionOwner};

/// Fixed remote port for the rssh-managed opencode serve. 39801 is outside the
/// TRAE range (39797) so both can coexist on the same host.
pub const OPENCODE_REMOTE_PORT: u16 = 39801;

#[derive(Clone)]
pub struct OpencodeRuntime {
    pub forward_active_id: String,
    pub forward_id: String,
    pub local_port: u16,
    pub password: String,
    pub ssh_session_id: String,
    pub profile_id: String,
}

#[derive(Clone, Serialize)]
pub struct OpencodeInfo {
    pub id: String,
    pub local_port: u16,
    pub password: String,
}

#[derive(Clone, Serialize)]
pub struct OpencodeSessionMeta {
    pub id: String,
    pub title: String,
    pub directory: String,
    pub time: String,
}

/// Message history entry as returned by `GET /session/:id/message`.
/// Forwarded to the frontend almost verbatim so rendering stays in one place.
#[derive(Clone, Serialize)]
pub struct OpencodeMessage {
    pub info: Value,
    pub parts: Vec<Value>,
}

fn get_ssh(state: &State<'_, AppState>, session_id: &str) -> AppResult<SessionHandle> {
    locked(&state.sessions)?
        .get(session_id)
        .cloned()
        .ok_or_else(|| AppError::not_found("session_not_found", json!({})))
}

fn get_oc(state: &State<'_, AppState>, opencode_id: &str) -> AppResult<OpencodeRuntime> {
    locked(&state.opencode_sessions)?
        .get(opencode_id)
        .cloned()
        .ok_or_else(|| AppError::not_found("opencode_session_not_found", json!({})))
}

/// Validate an opencode session id that is interpolated into a URL path.
/// `ses_` + base62/uuid chars only — a session id can never escape the path.
fn check_session_id(id: &str) -> AppResult<()> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(AppError::other("invalid_opencode_session_id", json!({})))
    }
}

async fn oc_get(rt: &OpencodeRuntime, path: &str) -> AppResult<Value> {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}{}", rt.local_port, path);
    let resp = client
        .get(&url)
        .basic_auth("opencode", Some(&rt.password))
        .send()
        .await
        .map_err(|e| AppError::other("opencode_http_failed", json!({ "err": e.to_string() })))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::other(
            "opencode_http_status",
            json!({ "status": status.as_u16() }),
        ));
    }
    resp.json()
        .await
        .map_err(|e| AppError::other("opencode_http_parse", json!({ "err": e.to_string() })))
}

async fn oc_send(
    rt: &OpencodeRuntime,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> AppResult<Value> {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{}{}", rt.local_port, path);
    let mut req = client
        .request(method, &url)
        .basic_auth("opencode", Some(&rt.password));
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| AppError::other("opencode_http_failed", json!({ "err": e.to_string() })))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .unwrap_or_default();
    if !status.is_success() {
        return Err(AppError::other(
            "opencode_http_status",
            json!({ "status": status.as_u16(), "body": text.chars().take(300).collect::<String>() }),
        ));
    }
    if text.trim().is_empty() {
        Ok(Value::Null)
    } else {
        serde_json::from_str(&text)
            .map_err(|e| AppError::other("opencode_http_parse", json!({ "err": e.to_string() })))
    }
}

/// Parse one SSE block and forward it to the frontend as `oc:event:{id}`
/// with `{ type, data }`. opencode emits bare `data:` lines whose JSON
/// carries the event type (`{"type":"message.part.updated",...}`); the SSE
/// `event:` line is usually absent, so fall back to `data.type`.
fn emit_sse(host: &Host, oc_id: &str, raw: &str) {
    let mut ev_type = String::new();
    let mut data = String::new();
    for line in raw.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("event:") {
            ev_type = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            data = v.trim().to_string();
        }
    }
    if data.is_empty() {
        return;
    }
    let parsed: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
    if ev_type.is_empty() {
        if let Some(t) = parsed.get("type").and_then(|v| v.as_str()) {
            ev_type = t.to_string();
        }
    }
    let _ = host.emit(
        &format!("oc:event:{oc_id}"),
        json!({ "type": ev_type, "data": parsed }),
    );
}

/// Stream `GET /event` in the background and forward parsed SSE blocks.
/// One reader per opencode tab; the frontend filters by its current session id.
fn spawn_event_reader(host: Host, oc_id: String, local_port: u16, password: String) {
    tauri::async_runtime::spawn(async move {
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{local_port}/event");
        let resp = match client
            .get(&url)
            .basic_auth("opencode", Some(&password))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            _ => return,
        };
        let mut buf = String::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let Ok(bytes) = chunk else { break };
            buf.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(pos) = buf.find("\n\n") {
                let raw = buf[..pos].to_string();
                buf = buf[pos + 2..].to_string();
                emit_sse(&host, &oc_id, &raw);
            }
        }
    });
}

/// Start (or reuse) `opencode serve` on the remote host, create a local
/// port-forward to it, and start streaming its event bus.
#[tauri::command]
pub async fn opencode_session_start(
    app: AppHandle,
    window: Window,
    state: State<'_, AppState>,
    session_id: String,
) -> AppResult<OpencodeInfo> {
    let ssh = get_ssh(&state, &session_id)?;
    let oc_id = format!("oc-{}", uuid::Uuid::new_v4().simple());
    let profile_id = ssh.profile_id().to_owned();

    // 1. Remote: ensure opencode serve is up on 39801, print the Basic Auth
    //    password on stdout. Idempotent: an already-running serve (same host,
    //    any machine that started it) is reused via the password file.
    let script = r#"
set -e
PASS_FILE=~/.config/rssh-opencode/pass
if ! command -v opencode >/dev/null 2>&1; then
  echo "__NO_OPENCODE__"
  exit 2
fi
health() {
  if [ -f "$PASS_FILE" ]; then
    curl -s -u "opencode:$(cat "$PASS_FILE")" -o /dev/null -w "%{http_code}" --max-time 2 http://127.0.0.1:39801/global/health 2>/dev/null || true
  else
    echo "000"
  fi
}
H=$(health)
if [ "$H" = "200" ]; then
  cat "$PASS_FILE"
else
  mkdir -p ~/.config/rssh-opencode
  PASS=$(cat /proc/sys/kernel/random/uuid 2>/dev/null || date +%s%N)
  printf '%s' "$PASS" > "$PASS_FILE"
  chmod 600 "$PASS_FILE"
  OPENCODE_SERVER_PASSWORD="$PASS" nohup opencode serve --hostname 127.0.0.1 --port 39801 > /tmp/rssh-opencode.log 2>&1 &
  sleep 1.5
  H2=$(health)
  if [ "$H2" != "200" ]; then
    echo "__SERVE_FAIL__"
    tail -5 /tmp/rssh-opencode.log 2>/dev/null || true
    exit 4
  fi
  printf '%s' "$PASS"
fi
"#;
    let out = exec_once(ssh.ssh_handle(), script, Duration::from_secs(40)).await?;
    let stdout = out.stdout.trim();
    if stdout.starts_with("__NO_OPENCODE__") {
        return Err(AppError::other(
            "opencode_not_installed",
            json!({ "hint": "服务器上未安装 opencode。安装：curl -fsSL https://opencode.ai/install | bash" }),
        ));
    }
    if stdout.starts_with("__SERVE_FAIL__") || stdout.starts_with("__NO_PASS__") {
        return Err(AppError::other(
            "opencode_serve_failed",
            json!({ "log": stdout }),
        ));
    }
    let password = stdout.to_string();

    // 2. Local port-forward to 127.0.0.1:39801 (ephemeral local port).
    let forward_id = format!("ocfwd-{}", uuid::Uuid::new_v4().simple());
    let f = crate::models::Forward {
        id: forward_id.clone(),
        name: format!("opencode-{}", &oc_id[3..9]),
        profile_id: profile_id.clone(),
        group_id: None,
        rules: vec![crate::models::ForwardRule {
            forward_type: crate::models::ForwardType::Local,
            local_port: 0,
            remote_host: "127.0.0.1".into(),
            remote_port: OPENCODE_REMOTE_PORT,
        }],
    };
    crate::db::forward::insert(&state.db, &f)?;
    let owner = SessionOwner::Window(window.label().to_owned());
    let active_id =
        crate::commands::forward::forward_start_impl(&state, owner.clone(), forward_id.clone())
            .await?;
    let stats =
        crate::commands::forward::forward_stats_impl(&state, &owner, active_id.clone())?;
    let local_port = stats
        .rules
        .iter()
        .find(|r| r.effective_port.is_some())
        .and_then(|r| r.effective_port)
        .ok_or_else(|| AppError::other("opencode_no_local_port", json!({})))?
        as u16;

    // 3. Stream the event bus for this tab.
    spawn_event_reader(
        Host::Tauri(app),
        oc_id.clone(),
        local_port,
        password.clone(),
    );

    // 4. Register.
    locked(&state.opencode_sessions)?.insert(
        oc_id.clone(),
        OpencodeRuntime {
            forward_active_id: active_id,
            forward_id,
            local_port,
            password: password.clone(),
            ssh_session_id: session_id,
            profile_id,
        },
    );
    Ok(OpencodeInfo {
        id: oc_id,
        local_port,
        password,
    })
}

/// Stop the port-forward (and drop the temp forward record). The remote
/// `opencode serve` keeps running so other machines can still reach it.
#[tauri::command]
pub async fn opencode_session_stop(
    state: State<'_, AppState>,
    opencode_id: String,
) -> AppResult<()> {
    if let Some(rt) = locked(&state.opencode_sessions)?.remove(&opencode_id) {
        let owner = SessionOwner::Headless(uuid::Uuid::new_v4());
        let _ = crate::commands::lifecycle::close_resource(
            &state,
            &rt.forward_active_id,
            SessionKind::Forward,
            &owner,
        );
        let _ = crate::db::forward::delete(&state.db, &rt.forward_id);
    }
    Ok(())
}

/// List server-side opencode sessions (most recent first).
#[tauri::command]
pub async fn opencode_session_list(
    state: State<'_, AppState>,
    opencode_id: String,
) -> AppResult<Vec<OpencodeSessionMeta>> {
    let rt = get_oc(&state, &opencode_id)?;
    let v = oc_get(&rt, "/session").await?;
    let arr = v.as_array().cloned().unwrap_or_default();
    let mut out = Vec::with_capacity(arr.len());
    for s in arr {
        let time = s
            .get("time")
            .and_then(|t| {
                t.as_str()
                    .map(|x| x.to_string())
                    .or_else(|| {
                        t.get("created")
                            .and_then(|c| c.as_i64())
                            .map(|n| n.to_string())
                    })
            })
            .unwrap_or_default();
        out.push(OpencodeSessionMeta {
            id: s["id"].as_str().unwrap_or("").to_string(),
            title: s["title"].as_str().unwrap_or("").to_string(),
            directory: s["directory"].as_str().unwrap_or("").to_string(),
            time,
        });
    }
    Ok(out)
}

/// Create a new opencode session on the server.
#[tauri::command]
pub async fn opencode_session_new(
    state: State<'_, AppState>,
    opencode_id: String,
) -> AppResult<String> {
    let rt = get_oc(&state, &opencode_id)?;
    let v = oc_send(&rt, reqwest::Method::POST, "/session", Some(json!({}))).await?;
    v["id"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| AppError::other("opencode_new_session_no_id", json!({})))
}

/// Delete a server-side opencode session.
#[tauri::command]
pub async fn opencode_session_delete(
    state: State<'_, AppState>,
    opencode_id: String,
    oc_session_id: String,
) -> AppResult<()> {
    check_session_id(&oc_session_id)?;
    let rt = get_oc(&state, &opencode_id)?;
    let _ = oc_send(
        &rt,
        reqwest::Method::DELETE,
        &format!("/session/{oc_session_id}"),
        None,
    )
    .await?;
    Ok(())
}

/// Send a prompt and wait for the assistant's full response (synchronous
/// `/message`). Reliable across opencode versions; the event bus still feeds
/// session.idle / error state. Returns the assistant reply so the frontend can
/// render it immediately.
#[tauri::command]
pub async fn opencode_session_send(
    state: State<'_, AppState>,
    opencode_id: String,
    oc_session_id: String,
    text: String,
) -> AppResult<OpencodeMessage> {
    check_session_id(&oc_session_id)?;
    let rt = get_oc(&state, &opencode_id)?;
    let body = json!({
        "parts": [ { "type": "text", "text": text } ]
    });
    let v = oc_send(
        &rt,
        reqwest::Method::POST,
        &format!("/session/{oc_session_id}/message"),
        Some(body),
    )
    .await?;
    Ok(OpencodeMessage {
        info: v.get("info").cloned().unwrap_or(Value::Null),
        parts: v
            .get("parts")
            .and_then(|p| p.as_array().cloned())
            .unwrap_or_default(),
    })
}

/// Fetch message history for a session (most recent first).
#[tauri::command]
pub async fn opencode_session_messages(
    state: State<'_, AppState>,
    opencode_id: String,
    oc_session_id: String,
    limit: Option<u32>,
) -> AppResult<Vec<OpencodeMessage>> {
    check_session_id(&oc_session_id)?;
    let rt = get_oc(&state, &opencode_id)?;
    let n = limit.unwrap_or(50).min(200);
    let v = oc_get(&rt, &format!("/session/{oc_session_id}/message?limit={n}")).await?;
    let arr = v.as_array().cloned().unwrap_or_default();
    Ok(arr
        .into_iter()
        .map(|m| OpencodeMessage {
            info: m.get("info").cloned().unwrap_or(Value::Null),
            parts: m
                .get("parts")
                .and_then(|p| p.as_array().cloned())
                .unwrap_or_default(),
        })
        .collect())
}

/// Abort the currently running session.
#[tauri::command]
pub async fn opencode_session_abort(
    state: State<'_, AppState>,
    opencode_id: String,
    oc_session_id: String,
) -> AppResult<()> {
    check_session_id(&oc_session_id)?;
    let rt = get_oc(&state, &opencode_id)?;
    let _ = oc_send(
        &rt,
        reqwest::Method::POST,
        &format!("/session/{oc_session_id}/abort"),
        None,
    )
    .await?;
    Ok(())
}
