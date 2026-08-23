//! User-defined AI provider rows. A provider is (name, protocol, endpoint,
//! model) + an api_key in the SecretStore keyed by row id — it replaced the
//! old fixed four-vendor settings keys (`ai_{name}_model` etc., migrated away
//! in `migration/v3_ai_provider_entities`).

use rusqlite::params;

use crate::error::AppResult;

use super::Db;

/// The three protocol types a provider can speak. Wire + DB values.
pub const PROTOCOLS: &[&str] = &[
    "deepseek-thinking",
    "openai-completions",
    "anthropic-messages",
];

pub fn protocol_valid(p: &str) -> bool {
    PROTOCOLS.contains(&p)
}

#[derive(Debug, Clone)]
pub struct ProviderRow {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub model: String,
    pub endpoint: String,
}

impl ProviderRow {
    /// Fresh row with a generated id. `provider-xxxxx` — the id is never
    /// user-typed; uniqueness is re-checked by the caller against existing ids.
    pub fn generate_id() -> String {
        format!(
            "provider-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..5]
        )
    }
}

pub fn list(db: &Db) -> AppResult<Vec<ProviderRow>> {
    let conn = db.lock()?;
    let mut stmt = conn.prepare(
        "SELECT id, name, protocol, model, endpoint FROM ai_providers ORDER BY created_at ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ProviderRow {
                id: r.get(0)?,
                name: r.get(1)?,
                protocol: r.get(2)?,
                model: r.get(3)?,
                endpoint: r.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get(db: &Db, id: &str) -> AppResult<Option<ProviderRow>> {
    let conn = db.lock()?;
    let res = conn
        .query_row(
            "SELECT id, name, protocol, model, endpoint FROM ai_providers WHERE id = ?1",
            [id],
            |r| {
                Ok(ProviderRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    protocol: r.get(2)?,
                    model: r.get(3)?,
                    endpoint: r.get(4)?,
                })
            },
        )
        .ok();
    Ok(res)
}

pub fn upsert_tx(conn: &rusqlite::Connection, row: &ProviderRow) -> AppResult<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO ai_providers (id, name, protocol, model, endpoint, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            protocol = excluded.protocol,
            model = excluded.model,
            endpoint = excluded.endpoint,
            updated_at = excluded.updated_at",
        params![row.id, row.name, row.protocol, row.model, row.endpoint, now],
    )?;
    Ok(())
}

pub fn upsert(db: &Db, row: &ProviderRow) -> AppResult<()> {
    let conn = db.lock()?;
    upsert_tx(&conn, row)
}

pub fn delete(db: &Db, id: &str) -> AppResult<()> {
    let conn = db.lock()?;
    conn.execute("DELETE FROM ai_providers WHERE id = ?1", [id])?;
    Ok(())
}

pub fn clear_all_tx(conn: &rusqlite::Connection) -> AppResult<()> {
    conn.execute("DELETE FROM ai_providers", [])?;
    Ok(())
}

/// Wipe every provider row. Sync-pull mirror semantics — only called when the
/// payload carries a validated `ai.providers` array (import validates before
/// any mutation, so this can never run against a rejected payload).
pub fn clear_all(db: &Db) -> AppResult<()> {
    let conn = db.lock()?;
    clear_all_tx(&conn)
}

/// Every provider id currently in the table — for generated-id collision
/// checks (create + migration).
pub fn ids(db: &Db) -> AppResult<Vec<String>> {
    let conn = db.lock()?;
    let mut stmt = conn.prepare("SELECT id FROM ai_providers")?;
    let rows = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(rows)
}
