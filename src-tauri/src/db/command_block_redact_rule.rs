//! Command-block copy redaction persistence. Schema v26 creates this as a snapshot
//! of `ai_redact_rules`; subsequent edits are deliberately independent.

use rusqlite::params;

use crate::error::AppResult;

use super::Db;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RedactRuleRow {
    pub id: String,
    pub pattern: String,
    pub replacement: String,
}

pub fn list(db: &Db) -> AppResult<Vec<RedactRuleRow>> {
    let conn = db.lock()?;
    let mut stmt = conn.prepare(
        "SELECT id, pattern, replacement
         FROM command_block_redact_rules
         ORDER BY created_at, id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(RedactRuleRow {
                id: row.get(0)?,
                pattern: row.get(1)?,
                replacement: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn upsert(db: &Db, rule: &RedactRuleRow) -> AppResult<()> {
    let conn = db.lock()?;
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO command_block_redact_rules
             (id, pattern, replacement, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(id) DO UPDATE SET
             pattern = excluded.pattern,
             replacement = excluded.replacement,
             updated_at = excluded.updated_at",
        params![rule.id, rule.pattern, rule.replacement, now],
    )?;
    Ok(())
}

pub fn delete(db: &Db, id: &str) -> AppResult<()> {
    let conn = db.lock()?;
    conn.execute("DELETE FROM command_block_redact_rules WHERE id = ?1", [id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_independent_from_ai_rules() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(list(&db).unwrap().len(), 8);

        delete(&db, "ip-10").unwrap();
        upsert(
            &db,
            &RedactRuleRow {
                id: "custom".into(),
                pattern: "secret".into(),
                replacement: "<X>".into(),
            },
        )
        .unwrap();

        let image_rules = list(&db).unwrap();
        assert!(!image_rules.iter().any(|r| r.id == "ip-10"));
        assert!(image_rules.iter().any(|r| r.id == "custom"));

        let ai_rules = crate::db::ai_redact_rule::list(&db).unwrap();
        assert!(ai_rules.iter().any(|r| r.id == "ip-10"));
        assert!(!ai_rules.iter().any(|r| r.id == "custom"));
    }
}
