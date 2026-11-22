use crate::db::Db;
use crate::error::{AppError, AppResult};
use serde_json::json;

pub use crate::db::incident_report::{
    delete, list, load, select, update, CaptureTarget, Excerpt, IncidentReport, ReportSummary,
};

pub fn capture(
    db: &Db,
    excerpt: &Excerpt,
    target: CaptureTarget,
    default_title: &str,
) -> AppResult<Vec<ReportSummary>> {
    if excerpt.text.is_empty() {
        return Err(AppError::config("incident_report_empty_excerpt", json!({})));
    }
    if default_title.trim().is_empty() {
        return Err(AppError::config("incident_report_empty_title", json!({})));
    }
    crate::db::incident_report::capture(db, excerpt, target, default_title)
}
