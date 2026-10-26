use rssh_core::error::AppResult;
use rssh_core::models::{
    DynamicDiscoveryContext, DynamicDiscoveryPlatform, DynamicDiscoverySnapshot,
    DynamicDiscoverySource, DynamicDiscoveryToolStatus,
};
use rssh_core::state::AppState;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub fn list_dynamic_discovery_sources(
    state: State<'_, Arc<AppState>>,
) -> AppResult<Vec<DynamicDiscoverySource>> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::discovery::list_dynamic_discovery_sources(state.inner().as_ref())
}

#[tauri::command]
pub fn save_dynamic_discovery_sources(
    state: State<'_, Arc<AppState>>,
    sources: Vec<DynamicDiscoverySource>,
) -> AppResult<()> {
    let runtime = tauri::async_runtime::handle();
    let _guard = runtime.inner().enter();
    rssh_core::commands::discovery::save_dynamic_discovery_sources(state.inner().as_ref(), sources)
}

#[tauri::command]
pub async fn dynamic_discovery_tool_status(
    platform: DynamicDiscoveryPlatform,
) -> AppResult<DynamicDiscoveryToolStatus> {
    rssh_core::commands::discovery::dynamic_discovery_tool_status(platform).await
}

#[tauri::command]
pub async fn list_dynamic_discovery_contexts(
    platform: DynamicDiscoveryPlatform,
) -> AppResult<Vec<DynamicDiscoveryContext>> {
    rssh_core::commands::discovery::list_dynamic_discovery_contexts(platform).await
}

#[tauri::command]
pub async fn discover_dynamic_targets(
    state: State<'_, Arc<AppState>>,
) -> AppResult<DynamicDiscoverySnapshot> {
    rssh_core::commands::discovery::discover_dynamic_targets(state.inner().as_ref()).await
}
