use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::Db;
use crate::error::{AppError, AppResult};

const EXCERPT_SEPARATOR: &str = "\n\n";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSummary {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub is_current: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncidentReport {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub content: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Excerpt {
    pub text: String,
    pub session_id: String,
    pub session_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureTarget {
    Current,
    New,
    Existing(String),
}

fn not_found() -> AppError {
    AppError::not_found("incident_report_not_found", serde_json::json!({}))
}

fn title(content: &str) -> AppResult<&str> {
    let title = content.lines().next().unwrap_or_default().trim();
    if title.is_empty() {
        return Err(AppError::config(
            "incident_report_empty_title",
            serde_json::json!({}),
        ));
    }
    Ok(title)
}

fn single_line_title(title: &str) -> String {
    title.trim().replace("\r\n", " ").replace(['\r', '\n'], " ")
}

fn fence_width(text: &str) -> usize {
    let longest = text.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
    3.max(longest + 1)
}

fn is_fence(text: &str) -> bool {
    text.len() >= 3 && text.bytes().all(|ch| ch == b'`')
}

// Read only the trailing block RSSH generates. Other user-authored Markdown
// (including a judgment below the block) remains opaque.
struct CaptureBlock<'a> {
    header: &'a str, // The complete document through the session heading.
    heading: &'a str,
    heading_offset: usize,
    fence: &'a str,
    text: &'a str,
    after_fence: &'a str,
}

impl<'a> CaptureBlock<'a> {
    fn trailing(document: &'a str) -> Option<Self> {
        let block = document.trim_end_matches([' ', '\t', '\r', '\n']);
        let after_fence = &document[block.len()..];
        let (before_close, fence) = block.rsplit_once('\n')?;
        if !is_fence(fence) {
            return None;
        }
        let (header, text) = before_close.rsplit_once(&format!("\n\n{fence}\n"))?;
        let (above, heading) = header.rsplit_once("\n\n")?;
        // A standalone fence closes the block. Text below it belongs to the
        // user even when the document ends in another fence-like line.
        let closed_early = text.lines().any(|line| {
            let indent = line.len() - line.trim_start_matches(' ').len();
            let token = line.trim_start_matches(' ').trim_end_matches([' ', '\t']);
            indent <= 3 && token.len() >= fence.len() && is_fence(token)
        });
        if !heading.starts_with("## ") || heading.contains('\n') || closed_early {
            return None;
        }
        Some(Self {
            header,
            heading,
            heading_offset: above.len() + 2,
            fence,
            text,
            after_fence,
        })
    }

    fn append(&self, text: &str) -> String {
        let fence = "`".repeat(
            self.fence
                .len()
                .max(fence_width(self.text))
                .max(fence_width(text)),
        );
        format!(
            "{}\n\n{fence}\n{}{EXCERPT_SEPARATOR}{text}\n{fence}{}",
            self.header, self.text, self.after_fence
        )
    }

    fn retained_in(&self, edited: &Self) -> bool {
        // Titles may be renamed while the last log is being edited. The rest
        // of the header still identifies the same position in the document.
        let same_position = self
            .header
            .lines()
            .skip(1)
            .eq(edited.header.lines().skip(1));
        if self.heading != edited.heading {
            return false;
        }
        // When a session name recurs, deleting its final group can expose an
        // earlier session with identical text. Renaming the title at the same
        // time must not transfer the deleted group's identity to that one.
        let occurrences = |block: &Self| {
            block
                .header
                .lines()
                .skip(1)
                .filter(|line| *line == block.heading)
                .count()
        };
        same_position || (self.text == edited.text && occurrences(self) == occurrences(edited))
    }
}

fn append_new_group(content: &mut String, excerpt: &Excerpt) -> usize {
    content.push_str("\n\n");
    let offset = content.len();
    content.push_str("## ");
    for ch in excerpt.session_name.chars() {
        if matches!(
            ch,
            '\\' | '[' | ']' | '<' | '>' | '*' | '_' | '#' | '`' | '~' | '&'
        ) {
            content.push('\\');
        }
        content.push(if matches!(ch, '\r' | '\n') { ' ' } else { ch });
    }
    let fence = "`".repeat(fence_width(&excerpt.text));
    content.push_str("\n\n");
    content.push_str(&fence);
    content.push('\n');
    content.push_str(&excerpt.text);
    content.push('\n');
    content.push_str(&fence);
    offset
}

fn list_on(conn: &Connection) -> AppResult<Vec<ReportSummary>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, created_at, is_current FROM incident_reports ORDER BY created_at DESC, rowid DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ReportSummary {
                id: r.get(0)?,
                title: r.get(1)?,
                created_at: r.get(2)?,
                is_current: r.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

pub fn list(db: &Db) -> AppResult<Vec<ReportSummary>> {
    let conn = db.lock()?;
    list_on(&conn)
}

pub fn load(db: &Db, id: &str) -> AppResult<IncidentReport> {
    db.lock()?
        .query_row(
            "SELECT id, title, created_at, content FROM incident_reports WHERE id=?1",
            [id],
            |r| {
                Ok(IncidentReport {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    created_at: r.get(2)?,
                    content: r.get(3)?,
                })
            },
        )
        .optional()?
        .ok_or_else(not_found)
}

fn make_current(conn: &Connection, id: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE incident_reports SET is_current = 0 WHERE is_current = 1 AND id <> ?1",
        [id],
    )?;
    if conn.execute(
        "UPDATE incident_reports SET is_current = 1 WHERE id = ?1",
        [id],
    )? == 0
    {
        return Err(not_found());
    }
    Ok(())
}

pub fn select(db: &Db, id: &str) -> AppResult<Vec<ReportSummary>> {
    db.with_transaction(|tx| {
        make_current(tx, id)?;
        list_on(tx)
    })
}

pub fn delete(db: &Db, id: &str) -> AppResult<Vec<ReportSummary>> {
    db.with_transaction(|tx| {
        let removed_current = tx
            .query_row(
                "DELETE FROM incident_reports WHERE id = ?1 RETURNING is_current",
                [id],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
        let mut reports = list_on(tx)?;
        if removed_current {
            if let Some(next) = reports.first_mut() {
                make_current(tx, &next.id)?;
                next.is_current = true;
            }
        }
        Ok(reports)
    })
}

pub fn capture(
    db: &Db,
    excerpt: &Excerpt,
    target: CaptureTarget,
    default_title: &str,
) -> AppResult<Vec<ReportSummary>> {
    // Reserve the writer before choosing a destination and inspecting its tail.
    let mut conn = db.lock()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let id = match target {
        CaptureTarget::Current => tx
            .query_row(
                "SELECT id FROM incident_reports WHERE is_current = 1",
                [],
                |r| r.get(0),
            )
            .optional()?,
        CaptureTarget::New => None,
        CaptureTarget::Existing(id) => Some(id),
    };
    let id = match id {
        Some(id) => id,
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            let name = single_line_title(default_title);
            let title = title(&name)?;
            tx.execute("INSERT INTO incident_reports(id, title, created_at, content) VALUES (?1, ?2, ?3, ?2)",
                params![id, title, chrono::Utc::now().timestamp_millis()])?;
            id
        }
    };
    make_current(&tx, &id)?;
    let (mut content, last_session_id, offset): (String, Option<String>, Option<usize>) = tx
        .query_row(
        "SELECT content, last_session_id, last_heading_offset FROM incident_reports WHERE id=?1",
        [&id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let block = CaptureBlock::trailing(&content).filter(|block| {
        Some(block.heading_offset) == offset
            && last_session_id.as_ref() == Some(&excerpt.session_id)
    });
    let heading_offset = if let Some(block) = block {
        let heading_offset = block.heading_offset;
        content = block.append(&excerpt.text);
        heading_offset
    } else {
        append_new_group(&mut content, excerpt)
    };
    tx.execute(
        "UPDATE incident_reports SET content=?2, last_session_id=?3, last_heading_offset=?4 WHERE id=?1",
        params![id, content, excerpt.session_id, heading_offset],
    )?;
    let reports = list_on(&tx)?;
    tx.commit()?;
    Ok(reports)
}

fn retained_heading(before: &str, after: &str, offset: Option<usize>) -> Option<usize> {
    let before = CaptureBlock::trailing(before)?;
    let after = CaptureBlock::trailing(after)?;
    (before.heading_offset == offset? && before.retained_in(&after)).then_some(after.heading_offset)
}

// Reapply only terminal captures that arrived after the editor took its
// snapshot. A same-session capture can change both fences as well as the body.
fn merge_captures(base: &str, draft: &str, stored: &str) -> Option<String> {
    if let Some(groups) = stored
        .strip_prefix(base)
        .filter(|tail| tail.starts_with("\n\n## "))
    {
        return Some(format!("{draft}{groups}"));
    }
    let original = CaptureBlock::trailing(base)?;
    let (new_fence, body) = stored
        .strip_prefix(original.header)?
        .strip_prefix("\n\n")?
        .split_once('\n')?;
    if !is_fence(new_fence) || new_fence.len() < original.fence.len() {
        return None;
    }
    let continuation = body
        .strip_prefix(original.text)?
        .strip_prefix(EXCERPT_SEPARATOR)?;
    let (added_text, later_groups) = continuation.split_once(&format!("\n{new_fence}"))?;
    let later_groups = later_groups.strip_prefix(original.after_fence)?;
    if !later_groups.is_empty() && !later_groups.starts_with("\n\n## ") {
        return None;
    }
    let (_, original_body) = base.split_once('\n')?;
    let (draft_title, draft_body) = draft.split_once('\n')?;
    if let Some(notes) = draft_body.strip_prefix(original_body) {
        // Title edits and notes below the original block do not overlap the
        // terminal's changes to the block itself, even when its fence widened.
        let (_, stored_body) = stored.split_once('\n')?;
        return Some(format!("{draft_title}\n{stored_body}{notes}"));
    }
    let edited = CaptureBlock::trailing(draft)?;
    if !original.retained_in(&edited) {
        return None;
    }
    let mut merged = edited.append(added_text);
    merged.push_str(later_groups);
    Some(merged)
}

pub fn update(
    db: &Db,
    id: &str,
    content: &str,
    previous_content: &str,
) -> AppResult<Vec<ReportSummary>> {
    title(content)?;
    let mut conn = db.lock()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (stored, heading, last_edit_bytes): (String, Option<usize>, usize) = tx
        .query_row(
            "SELECT content, last_heading_offset, last_edit_bytes FROM incident_reports WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or_else(not_found)?;
    let conflict = || AppError::other("incident_report_edit_conflict", serde_json::json!({}));
    if stored == content || content == previous_content {
        let reports = list_on(&tx)?;
        tx.commit()?;
        return Ok(reports);
    }
    let merged = if stored == previous_content {
        content.to_owned()
    } else {
        // Only a capture may extend a stale draft automatically. A manual
        // edit occupies the entire prefix through last_edit_bytes, even when
        // two users happen to type at the end of the same title or sentence.
        if previous_content.len() < last_edit_bytes {
            return Err(conflict());
        }
        merge_captures(previous_content, content, &stored).ok_or_else(conflict)?
    };
    let offset = retained_heading(&stored, &merged, heading);
    tx.execute(
        "UPDATE incident_reports SET title=?2, content=?3, last_heading_offset=?4, last_edit_bytes=?5,
          last_session_id=CASE WHEN ?4 IS NULL THEN NULL ELSE last_session_id END WHERE id=?1",
        params![id, title(&merged)?, merged, offset, merged.len()],
    )?;
    let reports = list_on(&tx)?;
    tx.commit()?;
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(db: &Db, target: CaptureTarget, session: &str, text: &str) -> String {
        let excerpt = Excerpt {
            session_id: session.into(),
            session_name: "gateway-01".into(),
            text: text.into(),
        };
        capture(db, &excerpt, target, "2026-11-21 gateway-01")
            .unwrap()
            .into_iter()
            .find(|r| r.is_current)
            .unwrap()
            .id
    }

    #[test]
    fn capture_keeps_the_title_on_one_line_and_appends_in_call_order() {
        let db = Db::open_in_memory().unwrap();
        let mut excerpt = Excerpt {
            text: "first request".into(),
            session_id: "gw".into(),
            session_name: "gateway".into(),
        };
        let reports = capture(&db, &excerpt, CaptureTarget::New, "date\r\ngateway").unwrap();
        let id = &reports[0].id;
        excerpt.text = "second request".into();
        capture(&db, &excerpt, CaptureTarget::Current, "unused").unwrap();
        let report = load(&db, id).unwrap();
        assert_eq!(report.title, "date gateway");
        assert_eq!(report.content.lines().next(), Some(report.title.as_str()));
        assert!(report.content.contains("first request\n\nsecond request"));
    }

    #[test]
    fn capture_and_edit_preserve_exact_markdown_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let a = add(&db, CaptureTarget::Current, "s1", "  request  \nbody");
        add(&db, CaptureTarget::Current, "s1", "second");
        let b = add(&db, CaptureTarget::New, "s3", "other");
        assert_ne!(a, b);
        add(&db, CaptureTarget::Existing(a.clone()), "s2", "response");
        add(&db, CaptureTarget::Current, "s1", "repeat");
        let original = load(&db, &a).unwrap().content;
        assert!(original.starts_with("2026-11-21 gateway-01\n\n## gateway-01"));
        assert_eq!(original.matches("## gateway-01").count(), 3);
        assert_eq!(original,
            "2026-11-21 gateway-01\n\n## gateway-01\n\n```\n  request  \nbody\n\nsecond\n```\n\n## gateway-01\n\n```\nresponse\n```\n\n## gateway-01\n\n```\nrepeat\n```");
        let revised = original
            .replacen("2026-11-21 gateway-01", "  # My title  \r\n\n判断", 1)
            .replace("  request  \nbody", "edited request");
        update(&db, &a, &revised, &original).unwrap();
        assert_eq!(
            update(&db, &a, "\nblank first line", &revised)
                .unwrap_err()
                .code(),
            "incident_report_empty_title"
        );
        add(&db, CaptureTarget::Current, "s1", "again");
        let after = load(&db, &a).unwrap();
        assert_eq!(after.title, "# My title");
        assert_eq!(
            after.content,
            revised.replace("repeat\n```", "repeat\n\nagain\n```")
        );
        assert_eq!(after.content.matches("## gateway-01").count(), 3);
        let fresh = "用户修改\n\n删掉了旧会话标题";
        update(&db, &a, fresh, &after.content).unwrap();
        add(&db, CaptureTarget::Current, "s1", "```\nend  \n");
        let expected = format!("{fresh}\n\n## gateway-01\n\n````\n```\nend  \n\n````");
        select(&db, &b).unwrap();
        assert!(select(&db, "missing").is_err());
        drop(db);
        let reopened = Db::open(dir.path()).unwrap();
        assert_eq!(
            list(&reopened)
                .unwrap()
                .into_iter()
                .find(|r| r.is_current)
                .unwrap()
                .id,
            b
        );
        assert_eq!(load(&reopened, &a).unwrap().content, expected);
    }

    #[test]
    fn markdown_edit_merges_a_concurrent_append_or_reports_a_conflict() {
        let db = Db::open_in_memory().unwrap();
        let id = add(&db, CaptureTarget::Current, "中文", "first");
        let base = load(&db, &id).unwrap().content;
        add(&db, CaptureTarget::Current, "中文", "second");
        let current = load(&db, &id).unwrap().content;
        let rewritten_group = base.replace("## gateway-01", "分析结论");
        assert_eq!(
            update(&db, &id, &rewritten_group, &base)
                .unwrap_err()
                .code(),
            "incident_report_edit_conflict"
        );
        assert_eq!(load(&db, &id).unwrap().content, current);
        let draft = format!("判断\n\n{base}");
        update(&db, &id, &draft, &base).unwrap();
        let merged = load(&db, &id).unwrap().content;
        assert_eq!(merged, format!("判断\n\n{current}"));
        let user_notes = format!("{merged}\n\nMore observations");
        add(&db, CaptureTarget::Current, "中文", "third");
        update(&db, &id, &user_notes, &merged).unwrap();
        let saved = load(&db, &id).unwrap().content;
        assert!(saved.contains("third"));
        assert!(saved.ends_with("More observations"));
        assert_eq!(
            update(&db, &id, "different", &base).unwrap_err().code(),
            "incident_report_edit_conflict"
        );
    }

    #[test]
    fn markdown_subsequent_capture_extends_merged_section_but_manual_edits_conflict() {
        let db = Db::open_in_memory().unwrap();
        let id = add(&db, CaptureTarget::Current, "s1", "first");
        let initial = load(&db, &id).unwrap().content;
        update(&db, &id, "A", &initial).unwrap();
        update(&db, &id, "AB", "A").unwrap();
        assert_eq!(
            update(&db, &id, "AC", "A").unwrap_err().code(),
            "incident_report_edit_conflict"
        );
        assert_eq!(load(&db, &id).unwrap().title, "AB");
        add(&db, CaptureTarget::Current, "s2", "second");
        update(&db, &id, "Edited", "AB").unwrap();
        add(&db, CaptureTarget::Current, "s2", "third");
        let merged = load(&db, &id).unwrap();
        assert_eq!(merged.title, "Edited");
        assert_eq!(merged.content.matches("## gateway-01").count(), 1);
        assert!(merged.content.starts_with("Edited\n\n## gateway-01"));
        assert!(merged.content.contains("second"));
        assert!(merged.content.contains("third"));
    }

    #[test]
    fn same_session_preserves_edited_code_and_widens_both_fences() {
        let db = Db::open_in_memory().unwrap();
        let id = add(&db, CaptureTarget::Current, "gateway", "old text");
        let base = load(&db, &id).unwrap().content;
        let revised = base
            .replacen("2026-11-21 gateway-01", "new title", 1)
            .replace("old text", "user edited  \nlog with inline ```  ")
            + "\n";
        update(&db, &id, &revised, &base).unwrap();
        let long = format!("{}\n`````\n tail  ", "多行数据".repeat(10000));
        add(&db, CaptureTarget::Current, "gateway", &long);
        let expected = format!("new title\n\n## gateway-01\n\n``````\nuser edited  \nlog with inline ```  \n\n{long}\n``````\n");
        assert_eq!(load(&db, &id).unwrap().content, expected);
        let with_note = format!("{expected}\n判断");
        update(&db, &id, &with_note, &expected).unwrap();
        add(&db, CaptureTarget::Current, "gateway", "after note");
        assert_eq!(
            load(&db, &id).unwrap().content,
            format!("{with_note}\n\n## gateway-01\n\n```\nafter note\n```")
        );
    }

    #[test]
    fn stale_title_edit_survives_widening_and_later_session_switches() {
        let dir = tempfile::tempdir().unwrap();
        let editor = Db::open(dir.path()).unwrap();
        let terminal = Db::open(dir.path()).unwrap();
        let id = add(&editor, CaptureTarget::Current, "gateway", "first");
        let base = load(&editor, &id).unwrap().content;
        add(
            &terminal,
            CaptureTarget::Current,
            "gateway",
            " ```second\nline ",
        );
        add(
            &terminal,
            CaptureTarget::Current,
            "gateway",
            "\nnext\n\nparagraph",
        );
        add(&terminal, CaptureTarget::Current, "app", "third");
        let stored = load(&terminal, &id).unwrap().content;
        assert!(stored.contains("first\n\n ```second\nline \n\n\nnext\n\nparagraph"));
        let draft = base.replacen("2026-11-21 gateway-01", "new title", 1);
        update(&editor, &id, &draft, &base).unwrap();
        let report = load(&editor, &id).unwrap();
        assert_eq!(report.title, "new title");
        assert_eq!(
            report.content,
            stored.replacen("2026-11-21 gateway-01", "new title", 1)
        );
        add(&terminal, CaptureTarget::Current, "app", "fourth");
        assert!(load(&editor, &id)
            .unwrap()
            .content
            .ends_with("\nthird\n\nfourth\n```"));
    }

    #[test]
    fn editing_out_an_identical_session_group_does_not_assign_its_identity_to_the_previous_one() {
        let db = Db::open_in_memory().unwrap();
        let id = add(&db, CaptureTarget::Current, "session-one", "同一行");
        let first_group = load(&db, &id).unwrap().content;
        add(&db, CaptureTarget::Current, "session-two", "同一行");
        let previous = load(&db, &id).unwrap().content;
        let edited = first_group.replacen("2026-11-21 gateway-01", "新标题", 1);
        update(&db, &id, &edited, &previous).unwrap();
        add(&db, CaptureTarget::Current, "session-two", "from two");
        let final_report = load(&db, &id).unwrap().content;
        assert_eq!(
            final_report,
            format!("{edited}\n\n## gateway-01\n\n```\nfrom two\n```")
        );
    }

    #[test]
    fn concurrent_capture_can_merge_a_renamed_title_and_a_judgment_below_the_original_block() {
        let db = Db::open_in_memory().unwrap();
        let id = add(&db, CaptureTarget::Current, "gateway", "before");
        let previous = load(&db, &id).unwrap().content;
        let old_title = "2026-11-21 gateway-01";
        let draft = previous.replacen(old_title, "判断", 1) + "\n\n人工补充";
        add(&db, CaptureTarget::Current, "gateway", "after ```");
        let stored = load(&db, &id).unwrap().content;
        update(&db, &id, &draft, &previous).unwrap();
        assert_eq!(
            load(&db, &id).unwrap().content,
            stored.replacen(old_title, "判断", 1) + "\n\n人工补充"
        );
    }

    #[test]
    fn upgrading_a_v37_database_adds_empty_local_reports_and_preserves_existing_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rssh.db");
        let old = Connection::open(&path).unwrap();
        old.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings VALUES ('existing', 'kept'); PRAGMA user_version = 37;",
        )
        .unwrap();
        drop(old);
        let upgraded = Db::open(dir.path()).unwrap();
        assert!(list(&upgraded).unwrap().is_empty());
        assert_eq!(
            crate::db::settings::get(&upgraded, "existing")
                .unwrap()
                .as_deref(),
            Some("kept")
        );
        let id = add(&upgraded, CaptureTarget::Current, "机房", "迁移后收录");
        drop(upgraded);
        assert!(load(&Db::open(dir.path()).unwrap(), &id)
            .unwrap()
            .content
            .contains("迁移后收录"));
    }

    #[test]
    fn remote_config_sync_keeps_local_report_text_private() {
        use crate::sync::config::{build_payload, merge_import, read_sync_prefs, ExportMode};
        let dir = tempfile::tempdir().unwrap();
        let db = std::sync::Arc::new(Db::open(dir.path()).unwrap());
        let secrets = crate::secret::DbStore::new(db.clone());
        let id = add(
            &db,
            CaptureTarget::Current,
            "gateway",
            "private request payload",
        );
        let mode = ExportMode::RemotePush(read_sync_prefs(&db).unwrap());
        let payload = build_payload(&db, &secrets, dir.path(), &mode).unwrap();
        let serialized = payload.to_string();
        assert!(!serialized.contains("private request payload"));
        assert!(!serialized.contains(&id));
        merge_import(&db, &secrets, dir.path(), &payload).unwrap();
        assert!(load(&db, &id)
            .unwrap()
            .content
            .contains("private request payload"));
    }

    #[test]
    fn deleting_by_id_keeps_or_replaces_the_current_report_in_list_order() {
        let db = Db::open_in_memory().unwrap();
        let oldest = add(&db, CaptureTarget::New, "gw", "one");
        let current = add(&db, CaptureTarget::New, "gw", "two");
        let other = add(&db, CaptureTarget::New, "gw", "three");
        let newest = add(&db, CaptureTarget::New, "gw", "four");
        db.lock()
            .unwrap()
            .execute("UPDATE incident_reports SET created_at = 42", [])
            .unwrap();
        // Identical titles: deleting one must always target its ID alone.
        select(&db, &current).unwrap();
        for _ in 0..2 {
            let reports = delete(&db, &oldest).unwrap();
            assert_eq!(reports.len(), 3);
            assert_eq!(reports.iter().find(|r| r.is_current).unwrap().id, current);
            assert_eq!(reports[0].id, newest);
        }
        let reports = delete(&db, &current).unwrap();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].id, newest);
        assert!(reports[0].is_current);
        assert_eq!(add(&db, CaptureTarget::Current, "gw", "continued"), newest);
        let last = delete(&db, &newest).unwrap();
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].id, other);
        assert!(last[0].is_current);
        assert!(delete(&db, &other).unwrap().is_empty());
        assert!(delete(&db, &other).unwrap().is_empty());
        let fresh = add(&db, CaptureTarget::Current, "gw", "fresh");
        assert!(![oldest, current, other, newest].contains(&fresh));
        assert_eq!(list(&db).unwrap().len(), 1);
    }

    #[test]
    fn deleting_current_while_another_window_appends_never_retargets_the_excerpt() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let other = Db::open(dir.path()).unwrap();
        let keep = add(&db, CaptureTarget::New, "gw", "keep this");
        let remove = add(&db, CaptureTarget::New, "gw", "delete this");
        let original = load(&db, &keep).unwrap().content;
        let start = std::sync::Arc::new(std::sync::Barrier::new(2));
        let ready = start.clone();
        let target = remove.clone();
        let writer = std::thread::spawn(move || {
            ready.wait();
            capture(
                &other,
                &Excerpt {
                    session_id: "other".into(),
                    session_name: "other".into(),
                    text: "late capture".into(),
                },
                CaptureTarget::Existing(target),
                "unused",
            )
        });
        start.wait();
        let remaining = delete(&db, &remove).unwrap();
        if let Err(error) = writer.join().unwrap() {
            assert_eq!(error.code(), "incident_report_not_found");
        }
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, keep);
        assert!(remaining[0].is_current);
        assert_eq!(
            load(&db, &remove).unwrap_err().code(),
            "incident_report_not_found"
        );
        assert_eq!(load(&db, &keep).unwrap().content, original);
        assert_eq!(add(&db, CaptureTarget::Current, "gw", "confirmed"), keep);
    }

    #[test]
    fn concurrent_first_captures_share_one_report_without_losing_entries() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let other = Db::open(dir.path()).unwrap();
        let start = std::sync::Arc::new(std::sync::Barrier::new(2));
        let ready = start.clone();
        let writer = std::thread::spawn(move || {
            ready.wait();
            add(&other, CaptureTarget::Current, "1", "one")
        });
        start.wait();
        let id = add(&db, CaptureTarget::Current, "2", "two");
        assert_eq!(writer.join().unwrap(), id);
        assert_eq!(list(&db).unwrap().len(), 1);
        let report = load(&db, &id).unwrap();
        assert!(report.content.contains("\none\n"));
        assert!(report.content.contains("\ntwo\n"));
        assert_eq!(report.content.matches("## gateway-01").count(), 2);
    }
}
