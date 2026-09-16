//! Transport-agnostic host context.
//!
//! The engine reaches the outside world through a threaded handle that does two
//! things: emit events (`ssh:data:<id>`, auth prompts, AI deltas …) and reach
//! `AppState` (the auth path fetches waiter maps via `app.state::<AppState>()`).
//! To run that same engine outside Tauri we replace the threaded
//! `tauri::AppHandle` with this `Host`: the Tauri app wraps its handle (so
//! `emit`/`state` behave exactly as before — INV-2), the headless ws server
//! wraps a sink closure + an `Arc<AppState>`. Engine call sites change only
//! `app.state::<AppState>()` → `app.state()`; `emit`/`clone` are untouched.

use std::ops::Deref;
use std::sync::Arc;

use serde::Serialize;

use crate::state::AppState;

/// Returns `true` if the event was handed off (ws send queued), `false` once the
/// connection is gone — lets `Host::emit` surface failure to prompt paths.
type Sink = Arc<dyn Fn(&str, serde_json::Value) -> bool + Send + Sync>;

#[derive(Clone)]
pub enum Host {
    /// Desktop: delegate straight to Tauri.
    Tauri(tauri::AppHandle),
    /// Headless: emit via a sink (ws push), reach state via a shared Arc.
    Headless { sink: Sink, state: Arc<AppState> },
}

/// Uniform `Deref<Target = AppState>` over either a Tauri `State` guard or a
/// plain borrow, so `let s = host.state(); &s.some_map` reads identically and
/// can be held across `.await` in both worlds.
pub enum StateRef<'a> {
    Tauri(tauri::State<'a, AppState>),
    Borrowed(&'a AppState),
}

impl Deref for StateRef<'_> {
    type Target = AppState;
    fn deref(&self) -> &AppState {
        match self {
            StateRef::Tauri(s) => s,
            StateRef::Borrowed(s) => s,
        }
    }
}

impl Host {
    /// Mirror of `tauri::Emitter::emit` (same `Serialize + Clone` bound).
    pub fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> tauri::Result<()> {
        match self {
            Host::Tauri(app) => {
                use tauri::Emitter as _;
                app.emit(event, payload)
            }
            Host::Headless { sink, .. } => {
                // The sink reports delivery. Once the connection is closed it returns
                // false → Err, so emit-then-await prompt paths (auth / passphrase /
                // host-key) bail at the emit step instead of parking on a waiter no
                // client can ever answer. See the WS-close handler in server.rs.
                if sink(
                    event,
                    serde_json::to_value(payload).unwrap_or(serde_json::Value::Null),
                ) {
                    Ok(())
                } else {
                    Err(tauri::Error::FailedToReceiveMessage)
                }
            }
        }
    }

    /// Replaces `app.state::<AppState>()`.
    pub fn state(&self) -> StateRef<'_> {
        match self {
            Host::Tauri(app) => {
                use tauri::Manager as _;
                StateRef::Tauri(app.state::<AppState>())
            }
            Host::Headless { state, .. } => StateRef::Borrowed(state),
        }
    }

    /// Check before proposing approval; unsupported hosts must not leave a
    /// useless approval card waiting for user input.
    pub async fn ensure_local_analysis_available(&self) -> Result<(), String> {
        match self {
            #[cfg(any(desktop, target_env = "ohos"))]
            Host::Tauri(_) => {
                crate::commands::window::ensure_app_window_available(
                    crate::commands::window::AppWindowPurpose::LocalAnalysis,
                )
                .await
            }
            #[cfg(not(any(desktop, target_env = "ohos")))]
            Host::Tauri(_) => Err("Additional windows are unavailable on this device".into()),
            Host::Headless { .. } => {
                Err("Local analysis windows are unavailable in headless mode".into())
            }
        }
    }

    /// Spawn a standalone analysis window (the `analyze_locally` AI tool).
    /// The native boundary rechecks capabilities after approval, so OHOS
    /// phone/tablet builds cannot bypass it by invoking the tool directly.
    pub async fn open_app_window(
        &self,
        label: &str,
        title: &str,
        init_script: &str,
    ) -> Result<(), String> {
        match self {
            #[cfg(any(desktop, target_env = "ohos"))]
            Host::Tauri(app) => {
                crate::commands::window::open_app_window(
                    app,
                    label,
                    title,
                    init_script,
                    crate::commands::window::AppWindowPurpose::LocalAnalysis,
                )
                .await
            }
            #[cfg(not(any(desktop, target_env = "ohos")))]
            Host::Tauri(_) => {
                let _ = (label, title, init_script);
                Err("Additional windows are unavailable on this device".into())
            }
            Host::Headless { .. } => Err("multi-window unavailable in headless mode".to_string()),
        }
    }
}
