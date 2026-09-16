use tauri::{AppHandle, Emitter, State};

use crate::error::{locked, AppError, AppResult};
use crate::models::SerialProfile;
use crate::state::{AppState, SessionKind, SessionOwner};
use crate::terminal::serial;

#[tauri::command]
pub async fn serial_list_ports() -> AppResult<Vec<String>> {
    #[cfg(target_env = "ohos")]
    {
        serial::available_ports().await
    }
    #[cfg(not(target_env = "ohos"))]
    {
        Ok(serial::available_ports())
    }
}

#[tauri::command]
pub async fn serial_open(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, AppState>,
    port: String,
    config: serial::SerialConfig,
    session_id: Option<String>,
) -> AppResult<String> {
    let session_id = crate::commands::lifecycle::resolve_session_id(session_id)?;
    let reservation = crate::commands::lifecycle::reserve_resource(
        &state,
        &session_id,
        SessionKind::Serial,
        SessionOwner::Window(window.label().to_owned()),
    )?;
    // Turn transport-agnostic serial output into Tauri events. The headless ws
    // server builds a different sink over the same `serial::open`.
    let sink: serial::SerialSink =
        std::sync::Arc::new(move |id: &str, out: serial::SerialOut| match out {
            serial::SerialOut::Data(b) => {
                let _ = app.emit(&format!("serial:data:{id}"), b);
            }
            serial::SerialOut::Close => {
                let _ = app.emit(&format!("serial:close:{id}"), ());
            }
        });
    #[cfg(target_env = "ohos")]
    let (id, handle) = serial::open(session_id, &port, config, sink).await?;
    #[cfg(not(target_env = "ohos"))]
    let (id, handle) = serial::open(session_id, &port, config, sink)?;
    reservation.activate_returned(
        &id,
        crate::commands::lifecycle::ReadySession::Serial(handle),
    )?;
    Ok(id)
}

/// Look up an open serial session's handle (cloned — `SerialHandle` is Arc-backed).
fn serial_handle(state: &State<'_, AppState>, session_id: &str) -> AppResult<serial::SerialHandle> {
    locked(&state.serial_sessions)?
        .get(session_id)
        .cloned()
        .ok_or_else(|| AppError::not_found("serial_not_found", serde_json::json!({})))
}

#[tauri::command]
pub async fn serial_write(
    state: State<'_, AppState>,
    session_id: String,
    data: Vec<u8>,
) -> AppResult<()> {
    let handle = serial_handle(&state, &session_id)?;
    #[cfg(target_env = "ohos")]
    {
        handle.write(&data).await
    }
    #[cfg(not(target_env = "ohos"))]
    {
        handle.write(&data)
    }
}

/// Drive the DTR control line (`true` = asserted). Manual line control for
/// MCU reset / bootloader entry / modem signalling.
#[tauri::command]
pub async fn serial_set_dtr(
    state: State<'_, AppState>,
    session_id: String,
    level: bool,
) -> AppResult<()> {
    let handle = serial_handle(&state, &session_id)?;
    #[cfg(target_env = "ohos")]
    {
        handle.set_dtr(level).await
    }
    #[cfg(not(target_env = "ohos"))]
    {
        handle.set_dtr(level)
    }
}

/// Drive the RTS control line (`true` = asserted).
#[tauri::command]
pub async fn serial_set_rts(
    state: State<'_, AppState>,
    session_id: String,
    level: bool,
) -> AppResult<()> {
    let handle = serial_handle(&state, &session_id)?;
    #[cfg(target_env = "ohos")]
    {
        handle.set_rts(level).await
    }
    #[cfg(not(target_env = "ohos"))]
    {
        handle.set_rts(level)
    }
}

/// Send a serial BREAK pulse — attention/interrupt signal for U-Boot,
/// kernel SysRq-over-serial, telco gear. Desktop holds it for ~250ms;
/// HarmonyOS uses the system serial service's pulse duration.
#[tauri::command]
pub async fn serial_send_break(state: State<'_, AppState>, session_id: String) -> AppResult<()> {
    let handle = serial_handle(&state, &session_id)?;
    #[cfg(target_env = "ohos")]
    {
        handle.send_break().await
    }
    #[cfg(not(target_env = "ohos"))]
    {
        handle.send_break()
    }
}

// No serial_resize: a serial line has no rows/cols. The frontend's transport
// table maps serial's resize entry to null, so it simply never calls it.

#[tauri::command]
pub fn serial_close(
    window: tauri::Window,
    state: State<'_, AppState>,
    session_id: String,
) -> AppResult<()> {
    crate::commands::lifecycle::close_resource(
        &state,
        &session_id,
        SessionKind::Serial,
        &SessionOwner::Window(window.label().to_owned()),
    )
}

// ── Saved serial profiles (peer of profile/forward; SQLite-persisted CRUD) ──

#[tauri::command]
pub fn list_serial_profiles(state: State<'_, AppState>) -> AppResult<Vec<SerialProfile>> {
    crate::db::serial_profile::list(&state.db)
}

#[tauri::command]
pub fn get_serial_profile(state: State<'_, AppState>, id: String) -> AppResult<SerialProfile> {
    crate::db::serial_profile::get(&state.db, &id)
}

#[tauri::command]
pub fn create_serial_profile(state: State<'_, AppState>, profile: SerialProfile) -> AppResult<()> {
    crate::db::serial_profile::insert(&state.db, &profile)
}

#[tauri::command]
pub fn update_serial_profile(state: State<'_, AppState>, profile: SerialProfile) -> AppResult<()> {
    crate::db::serial_profile::update(&state.db, &profile)
}

#[tauri::command]
pub fn delete_serial_profile(state: State<'_, AppState>, id: String) -> AppResult<()> {
    crate::db::serial_profile::delete(&state.db, &id)
}
