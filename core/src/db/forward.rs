use std::collections::HashSet;

use rusqlite::params;

use super::Db;
use crate::error::AppError;
use crate::error::AppResult;
use crate::models::{validate_name, Forward, ForwardRule, ForwardType};

fn parse_type(s: &str) -> ForwardType {
    match s {
        "remote" => ForwardType::Remote,
        "dynamic" => ForwardType::Dynamic,
        _ => ForwardType::Local,
    }
}
fn type_str(ft: ForwardType) -> &'static str {
    match ft {
        ForwardType::Local => "local",
        ForwardType::Remote => "remote",
        ForwardType::Dynamic => "dynamic",
    }
}

fn rules(conn: &rusqlite::Connection, id: &str) -> AppResult<Vec<ForwardRule>> {
    let mut stmt = conn.prepare(
        "SELECT type, local_port, remote_host, remote_port FROM forward_rules WHERE forward_id = ?1 ORDER BY position",
    )?;
    let rows = stmt.query_map(params![id], |row| {
        Ok(ForwardRule {
            forward_type: parse_type(&row.get::<_, String>(0)?),
            local_port: row.get::<_, u32>(1)? as u16,
            remote_host: row.get(2)?,
            remote_port: row.get::<_, u32>(3)? as u16,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn validate_rules(rules: &[ForwardRule]) -> AppResult<()> {
    if rules.is_empty() {
        return Err(AppError::config("fwd_rules_empty", serde_json::json!({})));
    }

    let mut local_ports = HashSet::new();
    let mut remote_ports = HashSet::new();
    for rule in rules {
        if rule.forward_type != ForwardType::Dynamic && rule.remote_host.trim().is_empty() {
            return Err(AppError::config(
                "fwd_remote_host_empty",
                serde_json::json!({}),
            ));
        }
        let invalid_target = match rule.forward_type {
            ForwardType::Local => rule.remote_port == 0,
            ForwardType::Remote => rule.local_port == 0,
            ForwardType::Dynamic => false,
        };
        if invalid_target {
            return Err(AppError::config("fwd_invalid_port", serde_json::json!({})));
        }
        let (ports, port, error_code) = match rule.forward_type {
            ForwardType::Remote => (
                &mut remote_ports,
                rule.remote_port,
                "fwd_duplicate_remote_port",
            ),
            _ => (
                &mut local_ports,
                rule.local_port,
                "fwd_duplicate_listen_port",
            ),
        };
        if port != 0 && !ports.insert(port) {
            return Err(AppError::config(
                error_code,
                serde_json::json!({ "port": port }),
            ));
        }
    }
    Ok(())
}

pub fn get(db: &Db, id: &str) -> AppResult<Forward> {
    let conn = db.lock()?;
    let mut forward = conn
        .query_row(
            "SELECT id, name, profile_id, group_id FROM forwards WHERE id = ?1",
            params![id],
            |row| {
                Ok(Forward {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    profile_id: row.get(2)?,
                    group_id: row.get(3)?,
                    rules: Vec::new(),
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                crate::error::AppError::not_found("fwd_not_found", serde_json::json!({}))
            }
            other => other.into(),
        })?;
    forward.rules = rules(&conn, id)?;
    Ok(forward)
}

pub fn list(db: &Db) -> AppResult<Vec<Forward>> {
    let conn = db.lock()?;
    let mut stmt =
        conn.prepare("SELECT id, name, profile_id, group_id FROM forwards ORDER BY name ASC")?;
    let rows = stmt.query_map([], |row| {
        Ok(Forward {
            id: row.get(0)?,
            name: row.get(1)?,
            profile_id: row.get(2)?,
            group_id: row.get(3)?,
            rules: Vec::new(),
        })
    })?;
    let mut forwards = rows.collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    for forward in &mut forwards {
        forward.rules = rules(&conn, &forward.id)?;
    }
    Ok(forwards)
}

fn write(conn: &rusqlite::Connection, f: &Forward, update_only: bool) -> AppResult<()> {
    validate_name(&f.name)?;
    validate_rules(&f.rules)?;

    if update_only {
        conn.execute(
            "UPDATE forwards SET name=?1, profile_id=?2, group_id=?3 WHERE id=?4",
            params![f.name, f.profile_id, f.group_id, f.id],
        )?;
    } else {
        conn.execute(
            "INSERT INTO forwards (id, name, profile_id, group_id) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(id) DO UPDATE SET name=excluded.name, profile_id=excluded.profile_id, group_id=excluded.group_id",
            params![f.id, f.name, f.profile_id, f.group_id],
        )?;
    }
    conn.execute(
        "DELETE FROM forward_rules WHERE forward_id = ?1",
        params![f.id],
    )?;
    for (position, rule) in f.rules.iter().enumerate() {
        conn.execute(
            "INSERT INTO forward_rules (forward_id, position, type, local_port, remote_host, remote_port) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![f.id, position as u32, type_str(rule.forward_type), rule.local_port as u32, rule.remote_host, rule.remote_port as u32],
        )?;
    }
    Ok(())
}

pub fn insert(db: &Db, f: &Forward) -> AppResult<()> {
    let mut conn = db.lock()?;
    let tx = conn.transaction()?;
    write(&tx, f, false)?;
    tx.commit()?;
    Ok(())
}

pub fn update(db: &Db, f: &Forward) -> AppResult<()> {
    let mut conn = db.lock()?;
    let tx = conn.transaction()?;
    write(&tx, f, true)?;
    tx.commit()?;
    Ok(())
}

pub fn delete(db: &Db, id: &str) -> AppResult<()> {
    let conn = db.lock()?;
    conn.execute("DELETE FROM forwards WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn clear_all_tx(conn: &rusqlite::Connection) -> AppResult<()> {
    conn.execute("DELETE FROM forwards", [])?;
    Ok(())
}

pub fn clear_all(db: &Db) -> AppResult<()> {
    let conn = db.lock()?;
    clear_all_tx(&conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn mk(id: &str, name: &str, ft: ForwardType) -> Forward {
        Forward {
            id: id.into(),
            name: name.into(),
            profile_id: "p1".into(),
            group_id: None,
            rules: vec![ForwardRule {
                forward_type: ft,
                local_port: 8080,
                remote_host: "127.0.0.1".into(),
                remote_port: 80,
            }],
        }
    }

    #[test]
    fn group_id_roundtrips() {
        let db = Db::open_in_memory().unwrap();
        let mut f = mk("f1", "alpha", ForwardType::Local);
        f.group_id = Some("g_prod".into());
        insert(&db, &f).unwrap();
        assert_eq!(get(&db, "f1").unwrap().group_id.as_deref(), Some("g_prod"));
        // A row inserted without a group stays ungrouped (NULL → None).
        insert(&db, &mk("f2", "beta", ForwardType::Local)).unwrap();
        assert_eq!(get(&db, "f2").unwrap().group_id, None);
    }

    #[test]
    fn update_changes_group_id() {
        // Exercises the UPDATE path (distinct from INSERT's ON CONFLICT branch):
        // proves the group_id placeholder/column alignment in update() is right.
        let db = Db::open_in_memory().unwrap();
        insert(&db, &mk("f1", "alpha", ForwardType::Local)).unwrap();
        let mut f = mk("f1", "alpha", ForwardType::Local);
        f.group_id = Some("g9".into());
        update(&db, &f).unwrap();
        assert_eq!(get(&db, "f1").unwrap().group_id.as_deref(), Some("g9"));
    }

    #[test]
    fn insert_then_get_for_all_types() {
        let db = Db::open_in_memory().unwrap();
        for ft in [
            ForwardType::Local,
            ForwardType::Remote,
            ForwardType::Dynamic,
        ] {
            let id = format!("f-{}", type_str(ft));
            insert(&db, &mk(&id, &id, ft)).unwrap();
            assert_eq!(get(&db, &id).unwrap().rules[0].forward_type, ft);
        }
    }

    #[test]
    fn upsert_overwrites_ports() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, &mk("f1", "alpha", ForwardType::Local)).unwrap();
        let mut updated = mk("f1", "alpha", ForwardType::Remote);
        updated.rules[0].local_port = 9999;
        updated.rules[0].remote_port = 3306;
        insert(&db, &updated).unwrap();
        let got = get(&db, "f1").unwrap();
        assert_eq!(got.rules[0].forward_type, ForwardType::Remote);
        assert_eq!(got.rules[0].local_port, 9999);
        assert_eq!(got.rules[0].remote_port, 3306);
    }

    #[test]
    fn multiple_rules_roundtrip_in_order() {
        let db = Db::open_in_memory().unwrap();
        let mut f = mk("f1", "mixed", ForwardType::Local);
        f.rules.push(ForwardRule {
            forward_type: ForwardType::Remote,
            local_port: 3000,
            remote_host: "localhost".into(),
            remote_port: 9000,
        });
        insert(&db, &f).unwrap();
        assert_eq!(get(&db, "f1").unwrap().rules, f.rules);
    }

    #[test]
    fn update_replaces_the_complete_rule_set() {
        let db = Db::open_in_memory().unwrap();
        let mut original = mk("f1", "mixed", ForwardType::Local);
        original.rules.push(ForwardRule {
            forward_type: ForwardType::Dynamic,
            local_port: 1080,
            remote_host: "127.0.0.1".into(),
            remote_port: 0,
        });
        insert(&db, &original).unwrap();

        let mut replacement = mk("f1", "mixed", ForwardType::Remote);
        replacement.rules[0].local_port = 5432;
        replacement.rules[0].remote_port = 9000;
        update(&db, &replacement).unwrap();

        assert_eq!(get(&db, "f1").unwrap().rules, replacement.rules);
    }

    #[test]
    fn list_sorted_by_name() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, &mk("f1", "zebra", ForwardType::Local)).unwrap();
        insert(&db, &mk("f2", "apple", ForwardType::Local)).unwrap();
        let names: Vec<String> = list(&db).unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["apple", "zebra"]);
    }

    #[test]
    fn delete_removes_row() {
        let db = Db::open_in_memory().unwrap();
        insert(&db, &mk("f1", "alpha", ForwardType::Local)).unwrap();
        delete(&db, "f1").unwrap();
        assert_eq!(get(&db, "f1").unwrap_err().code(), "fwd_not_found");
    }

    /// 防御 schema 漂移：DB 里出现未知 type 字符串时不能 panic，应退回 Local。
    /// 通过 raw SQL 注入一个 type='garbage' 的行模拟。
    #[test]
    fn unknown_type_string_falls_back_to_local() {
        let db = Db::open_in_memory().unwrap();
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "INSERT INTO forwards (id, name, profile_id) VALUES (?1, ?2, ?3)",
                params!["fx", "weird", "p1"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO forward_rules
                     (forward_id, position, type, local_port, remote_host, remote_port)
                 VALUES (?1, 0, ?2, ?3, ?4, ?5)",
                params!["fx", "garbage_type", 1u32, "127.0.0.1", 80u32],
            )
            .unwrap();
        }
        assert_eq!(
            get(&db, "fx").unwrap().rules[0].forward_type,
            ForwardType::Local
        );
    }

    #[test]
    fn insert_rejects_name_with_control_char() {
        let db = Db::open_in_memory().unwrap();
        let bad = mk("f1", "bad\nname", ForwardType::Local);
        assert_eq!(
            insert(&db, &bad).unwrap_err().code(),
            "name_has_control_char"
        );
    }

    #[test]
    fn insert_rejects_zero_and_duplicate_listen_ports() {
        let db = Db::open_in_memory().unwrap();
        let mut invalid = mk("f1", "invalid", ForwardType::Local);
        invalid.rules[0].remote_port = 0;
        assert_eq!(
            insert(&db, &invalid).unwrap_err().code(),
            "fwd_invalid_port"
        );

        let mut duplicate = mk("f2", "duplicate", ForwardType::Local);
        duplicate.rules.push(ForwardRule {
            forward_type: ForwardType::Dynamic,
            local_port: 8080,
            remote_host: "127.0.0.1".into(),
            remote_port: 0,
        });
        assert_eq!(
            insert(&db, &duplicate).unwrap_err().code(),
            "fwd_duplicate_listen_port"
        );
    }

    #[test]
    fn insert_rejects_duplicate_remote_ports_with_remote_error() {
        let db = Db::open_in_memory().unwrap();
        let mut duplicate = mk("f1", "duplicate", ForwardType::Remote);
        duplicate.rules[0].local_port = 1000;
        duplicate.rules.push(ForwardRule {
            forward_type: ForwardType::Remote,
            local_port: 1001,
            remote_host: "127.0.0.1".into(),
            remote_port: duplicate.rules[0].remote_port,
        });

        assert_eq!(
            insert(&db, &duplicate).unwrap_err().code(),
            "fwd_duplicate_remote_port"
        );
    }

    #[test]
    fn insert_rejects_empty_remote_host() {
        let db = Db::open_in_memory().unwrap();
        let mut invalid = mk("f1", "invalid", ForwardType::Local);
        invalid.rules[0].remote_host = "  ".into();

        assert_eq!(
            insert(&db, &invalid).unwrap_err().code(),
            "fwd_remote_host_empty"
        );
    }
}
