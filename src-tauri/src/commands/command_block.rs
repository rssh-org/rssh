use tauri::State;

use crate::db::command_block_redact_rule::{self, RedactRuleRow};
use crate::error::AppResult;
use crate::state::AppState;

pub fn list_redact_rules(db: &crate::db::Db) -> AppResult<Vec<RedactRuleRow>> {
    command_block_redact_rule::list(db)
}

pub fn save_redact_rule(db: &crate::db::Db, rule: &RedactRuleRow) -> AppResult<()> {
    crate::redaction::validate_pattern(&rule.pattern)?;
    command_block_redact_rule::upsert(db, rule)
}

pub fn delete_redact_rule(db: &crate::db::Db, id: &str) -> AppResult<()> {
    command_block_redact_rule::delete(db, id)
}

#[tauri::command]
pub fn command_block_list_redact_rules(
    state: State<'_, AppState>,
) -> AppResult<Vec<RedactRuleRow>> {
    list_redact_rules(&state.db)
}

#[tauri::command]
pub fn command_block_save_redact_rule(
    state: State<'_, AppState>,
    id: String,
    pattern: String,
    replacement: String,
) -> AppResult<()> {
    save_redact_rule(
        &state.db,
        &RedactRuleRow {
            id,
            pattern,
            replacement,
        },
    )
}

#[tauri::command]
pub fn command_block_delete_redact_rule(state: State<'_, AppState>, id: String) -> AppResult<()> {
    delete_redact_rule(&state.db, &id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_rejects_invalid_and_zero_width_regex() {
        let db = crate::db::Db::open_in_memory().unwrap();
        for (id, pattern, code) in [
            ("invalid", "(", "redact_invalid_regex"),
            ("empty", "", "redact_zero_width_pattern"),
        ] {
            let err = save_redact_rule(
                &db,
                &RedactRuleRow {
                    id: id.into(),
                    pattern: pattern.into(),
                    replacement: "<X>".into(),
                },
            )
            .unwrap_err();
            assert_eq!(err.code(), code);
            assert!(!list_redact_rules(&db).unwrap().iter().any(|r| r.id == id));
        }
    }
}
