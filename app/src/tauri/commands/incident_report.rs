use std::sync::Arc;

use rssh_core::commands::incident_report::{
    self, CaptureTarget, Excerpt, IncidentReport, ReportSummary,
};
use rssh_core::error::AppResult;
use rssh_core::state::AppState;
use tauri::State;

#[tauri::command(async)]
pub fn list_incident_reports(state: State<'_, Arc<AppState>>) -> AppResult<Vec<ReportSummary>> {
    incident_report::list(&state.db)
}

#[tauri::command(async)]
pub fn load_incident_report(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> AppResult<IncidentReport> {
    incident_report::load(&state.db, &id)
}

#[tauri::command(async)]
pub fn select_incident_report(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> AppResult<Vec<ReportSummary>> {
    incident_report::select(&state.db, &id)
}

#[tauri::command(async)]
pub fn delete_incident_report(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> AppResult<Vec<ReportSummary>> {
    incident_report::delete(&state.db, &id)
}

#[tauri::command(async)]
pub fn capture_incident_excerpt(
    state: State<'_, Arc<AppState>>,
    excerpt: Excerpt,
    target: CaptureTarget,
    default_title: String,
) -> AppResult<Vec<ReportSummary>> {
    incident_report::capture(&state.db, &excerpt, target, &default_title)
}

#[tauri::command(async)]
pub fn update_incident_report(
    state: State<'_, Arc<AppState>>,
    id: String,
    content: String,
    previous_content: String,
) -> AppResult<Vec<ReportSummary>> {
    incident_report::update(&state.db, &id, &content, &previous_content)
}
