use std::sync::Arc;

use rssh_core::commands::incident_report::{
    self, CaptureTarget, Excerpt, IncidentReport, ReportSummary,
};
use rssh_core::commands::run_data_dir_task;
use rssh_core::error::AppResult;
use rssh_core::state::AppState;
use tauri::State;

#[tauri::command]
pub async fn list_incident_reports(
    state: State<'_, Arc<AppState>>,
) -> AppResult<Vec<ReportSummary>> {
    run_data_dir_task(&state, incident_report::list).await?
}

#[tauri::command]
pub async fn load_incident_report(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> AppResult<IncidentReport> {
    run_data_dir_task(&state, move |dir| incident_report::load(dir, &id)).await?
}

#[tauri::command]
pub async fn select_incident_report(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> AppResult<Vec<ReportSummary>> {
    run_data_dir_task(&state, move |dir| incident_report::select(dir, &id)).await?
}

#[tauri::command]
pub async fn delete_incident_report(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> AppResult<Vec<ReportSummary>> {
    run_data_dir_task(&state, move |dir| incident_report::delete(dir, &id)).await?
}

#[tauri::command]
pub async fn capture_incident_excerpt(
    state: State<'_, Arc<AppState>>,
    excerpt: Excerpt,
    target: CaptureTarget,
    default_title: String,
) -> AppResult<Vec<ReportSummary>> {
    run_data_dir_task(&state, move |dir| {
        incident_report::capture(dir, &excerpt, target, &default_title)
    })
    .await?
}

#[tauri::command]
pub async fn update_incident_report(
    state: State<'_, Arc<AppState>>,
    id: String,
    content: String,
    previous_content: String,
) -> AppResult<Vec<ReportSummary>> {
    run_data_dir_task(&state, move |dir| {
        incident_report::update(dir, &id, &content, &previous_content)
    })
    .await?
}
