use rusqlite::params;

use super::Db;
use crate::error::AppResult;
use crate::models::Plugin;

const COLS: &str =
    "id, name, version, description, author, area, preview, enabled, installed_at, sort_order";

fn row_to_plugin(row: &rusqlite::Row<'_>) -> rusqlite::Result<Plugin> {
    Ok(Plugin {
        id: row.get(0)?,
        name: row.get(1)?,
        version: row.get(2)?,
        description: row.get(3)?,
        author: row.get(4)?,
        area: row.get(5)?,
        preview: row.get(6)?,
        enabled: row.get::<_, i64>(7)? != 0,
        installed_at: row.get(8)?,
        sort_order: row.get(9)?,
    })
}

pub fn list(db: &Db) -> AppResult<Vec<Plugin>> {
    let conn = db.lock()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM plugins ORDER BY area ASC, sort_order ASC, id ASC"
    ))?;
    let rows = stmt.query_map([], |row| row_to_plugin(row))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get(db: &Db, id: &str) -> AppResult<Option<Plugin>> {
    let conn = db.lock()?;
    let result = conn.query_row(
        &format!("SELECT {COLS} FROM plugins WHERE id = ?1"),
        params![id],
        |row| row_to_plugin(row),
    );
    match result {
        Ok(plugin) => Ok(Some(plugin)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Insert or replace: installing the same id again is an upgrade (dir contents
/// were already swapped on disk by the caller before this runs). `enabled`
/// and `sort_order` are deliberately NOT in the update set — an upgrade keeps
/// the user's toggle state and position.
pub fn upsert(db: &Db, plugin: &Plugin) -> AppResult<()> {
    let conn = db.lock()?;
    conn.execute(
        "INSERT INTO plugins (id, name, version, description, author, area, preview, enabled, installed_at, sort_order)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
            name = ?2, version = ?3, description = ?4, author = ?5, area = ?6,
            preview = ?7, installed_at = ?9",
        params![
            plugin.id,
            plugin.name,
            plugin.version,
            plugin.description,
            plugin.author,
            plugin.area,
            plugin.preview,
            plugin.enabled as i64,
            plugin.installed_at,
            plugin.sort_order,
        ],
    )?;
    Ok(())
}

/// Remove a row. Preserves nothing — `enabled` state dies with the install;
/// a reinstall comes back enabled (fresh trust decision by the user).
pub fn delete(db: &Db, id: &str) -> AppResult<()> {
    let conn = db.lock()?;
    conn.execute("DELETE FROM plugins WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn set_enabled(db: &Db, id: &str, enabled: bool) -> AppResult<()> {
    let conn = db.lock()?;
    conn.execute(
        "UPDATE plugins SET enabled = ?2 WHERE id = ?1",
        params![id, enabled as i64],
    )?;
    Ok(())
}

/// One past the current maximum, so each new install appends to its area.
pub fn next_sort_order(db: &Db) -> AppResult<i64> {
    let conn = db.lock()?;
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM plugins",
        [],
        |row| row.get(0),
    )?)
}

/// Rewrite the order within one area from a full ordered id list (the manager
/// page computes the new sequence after a move; unknown ids are ignored).
pub fn set_order(db: &Db, ids: &[String]) -> AppResult<()> {
    let conn = db.lock()?;
    for (idx, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE plugins SET sort_order = ?2 WHERE id = ?1",
            params![id, idx as i64],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, sort_order: i64) -> Plugin {
        Plugin {
            id: id.to_owned(),
            name: "Monitor".into(),
            version: "1.0.0".into(),
            description: "desc".into(),
            author: "a".into(),
            area: "side".into(),
            preview: "preview.html".into(),
            enabled: true,
            installed_at: 42,
            sort_order,
        }
    }

    #[test]
    fn empty_db_lists_nothing() {
        let db = Db::open_in_memory().unwrap();
        assert!(list(&db).unwrap().is_empty());
    }

    #[test]
    fn upsert_then_get_round_trip() {
        let db = Db::open_in_memory().unwrap();
        upsert(&db, &sample("mon", 0)).unwrap();
        let p = get(&db, "mon").unwrap().unwrap();
        assert_eq!(p.name, "Monitor");
        assert_eq!(p.area, "side");
        assert_eq!(p.preview, "preview.html");
        assert!(p.enabled);
    }

    #[test]
    fn reinstall_upgrades_but_keeps_enabled_and_order() {
        // ON CONFLICT keeps the old `enabled`/`sort_order` — an upgrade must
        // not silently re-enable a plugin the user turned off, or teleport it
        // to the end of the list.
        let db = Db::open_in_memory().unwrap();
        upsert(&db, &sample("mon", 3)).unwrap();
        set_enabled(&db, "mon", false).unwrap();
        let mut upgraded = sample("mon", 0);
        upgraded.version = "2.0.0".into();
        upsert(&db, &upgraded).unwrap();
        let got = get(&db, "mon").unwrap().unwrap();
        assert_eq!(got.version, "2.0.0");
        assert!(!got.enabled);
        assert_eq!(got.sort_order, 3);
    }

    #[test]
    fn set_order_rewrites_sequence() {
        let db = Db::open_in_memory().unwrap();
        for (i, id) in ["a", "b", "c"].into_iter().enumerate() {
            upsert(&db, &sample(id, i as i64)).unwrap();
        }
        set_order(&db, &["c".into(), "a".into(), "b".into()]).unwrap();
        let ids: Vec<String> = list(&db)
            .unwrap()
            .into_iter()
            .filter(|p| p.area == "side")
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec!["c", "a", "b"]);
    }

    #[test]
    fn next_sort_order_appends() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(next_sort_order(&db).unwrap(), 0);
        upsert(&db, &sample("a", 0)).unwrap();
        upsert(&db, &sample("b", 5)).unwrap();
        assert_eq!(next_sort_order(&db).unwrap(), 6);
    }

    #[test]
    fn delete_removes_row() {
        let db = Db::open_in_memory().unwrap();
        upsert(&db, &sample("mon", 0)).unwrap();
        delete(&db, "mon").unwrap();
        assert!(get(&db, "mon").unwrap().is_none());
    }
}
