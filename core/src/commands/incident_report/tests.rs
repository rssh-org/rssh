use super::*;

fn add(data_dir: &Path, target: CaptureTarget, session: &str, text: &str) -> String {
    let excerpt = Excerpt {
        session_id: session.into(),
        session_name: "gateway-01".into(),
        text: text.into(),
    };
    capture(data_dir, &excerpt, target, "2026-11-21 gateway-01")
        .unwrap()
        .into_iter()
        .find(|r| r.is_current)
        .unwrap()
        .id
}

#[test]
fn capture_keeps_the_title_on_one_line_and_appends_in_call_order() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path();
    let mut excerpt = Excerpt {
        text: "first request".into(),
        session_id: "gw".into(),
        session_name: "gateway".into(),
    };
    let reports = capture(data_dir, &excerpt, CaptureTarget::New, "date\r\ngateway").unwrap();
    let id = &reports[0].id;
    excerpt.text = "second request".into();
    capture(data_dir, &excerpt, CaptureTarget::Current, "unused").unwrap();
    let report = load(data_dir, id).unwrap();
    assert_eq!(report.title, "date gateway");
    assert_eq!(report.content.lines().next(), Some(report.title.as_str()));
    assert!(report.content.contains("first request\n\nsecond request"));
}

#[test]
fn capture_and_edit_preserve_exact_markdown_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path();
    let a = add(data_dir, CaptureTarget::Current, "s1", "  request  \nbody");
    add(data_dir, CaptureTarget::Current, "s1", "second");
    let b = add(data_dir, CaptureTarget::New, "s3", "other");
    assert_ne!(a, b);
    add(
        data_dir,
        CaptureTarget::Existing(a.clone()),
        "s2",
        "response",
    );
    add(data_dir, CaptureTarget::Current, "s1", "repeat");
    let original = load(data_dir, &a).unwrap().content;
    assert!(original.starts_with("2026-11-21 gateway-01\n\n## gateway-01"));
    assert_eq!(original.matches("## gateway-01").count(), 3);
    assert_eq!(original,
        "2026-11-21 gateway-01\n\n## gateway-01\n\n```\n  request  \nbody\n\nsecond\n```\n\n## gateway-01\n\n```\nresponse\n```\n\n## gateway-01\n\n```\nrepeat\n```");
    let revised = original
        .replacen("2026-11-21 gateway-01", "  # My title  \r\n\n判断", 1)
        .replace("  request  \nbody", "edited request");
    update(data_dir, &a, &revised, &original).unwrap();
    assert_eq!(
        update(data_dir, &a, "\nblank first line", &revised)
            .unwrap_err()
            .code(),
        "incident_report_empty_title"
    );
    add(data_dir, CaptureTarget::Current, "s1", "again");
    let after = load(data_dir, &a).unwrap();
    assert_eq!(after.title, "# My title");
    assert_eq!(
        after.content,
        revised.replace("repeat\n```", "repeat\n\nagain\n```")
    );
    assert_eq!(after.content.matches("## gateway-01").count(), 3);
    let fresh = "用户修改\n\n删掉了旧会话标题";
    update(data_dir, &a, fresh, &after.content).unwrap();
    add(data_dir, CaptureTarget::Current, "s1", "```\nend  \n");
    let expected = format!("{fresh}\n\n## gateway-01\n\n````\n```\nend  \n\n````");
    select(data_dir, &b).unwrap();
    assert!(select(data_dir, "missing").is_err());
    let reopened = dir.path();
    assert_eq!(
        list(reopened)
            .unwrap()
            .into_iter()
            .find(|r| r.is_current)
            .unwrap()
            .id,
        b
    );
    assert_eq!(load(reopened, &a).unwrap().content, expected);
}

#[test]
fn markdown_edit_merges_a_concurrent_append_or_reports_a_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path();
    let id = add(data_dir, CaptureTarget::Current, "中文", "first");
    let base = load(data_dir, &id).unwrap().content;
    add(data_dir, CaptureTarget::Current, "中文", "second");
    let current = load(data_dir, &id).unwrap().content;
    let rewritten_group = base.replace("## gateway-01", "分析结论");
    assert_eq!(
        update(data_dir, &id, &rewritten_group, &base)
            .unwrap_err()
            .code(),
        "incident_report_edit_conflict"
    );
    assert_eq!(load(data_dir, &id).unwrap().content, current);
    let draft = format!("判断\n\n{base}");
    update(data_dir, &id, &draft, &base).unwrap();
    let merged = load(data_dir, &id).unwrap().content;
    assert_eq!(merged, format!("判断\n\n{current}"));
    let user_notes = format!("{merged}\n\nMore observations");
    add(data_dir, CaptureTarget::Current, "中文", "third");
    update(data_dir, &id, &user_notes, &merged).unwrap();
    let saved = load(data_dir, &id).unwrap().content;
    assert!(saved.contains("third"));
    assert!(saved.ends_with("More observations"));
    assert_eq!(
        update(data_dir, &id, "different", &base)
            .unwrap_err()
            .code(),
        "incident_report_edit_conflict"
    );
}

#[test]
fn markdown_subsequent_capture_extends_merged_section_but_manual_edits_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path();
    let id = add(data_dir, CaptureTarget::Current, "s1", "first");
    let initial = load(data_dir, &id).unwrap().content;
    update(data_dir, &id, "A", &initial).unwrap();
    update(data_dir, &id, "AB", "A").unwrap();
    assert_eq!(
        update(data_dir, &id, "AC", "A").unwrap_err().code(),
        "incident_report_edit_conflict"
    );
    assert_eq!(load(data_dir, &id).unwrap().title, "AB");
    add(data_dir, CaptureTarget::Current, "s2", "second");
    update(data_dir, &id, "Edited", "AB").unwrap();
    add(data_dir, CaptureTarget::Current, "s2", "third");
    let merged = load(data_dir, &id).unwrap();
    assert_eq!(merged.title, "Edited");
    assert_eq!(merged.content.matches("## gateway-01").count(), 1);
    assert!(merged.content.starts_with("Edited\n\n## gateway-01"));
    assert!(merged.content.contains("second"));
    assert!(merged.content.contains("third"));
}

#[test]
fn same_session_preserves_edited_code_and_widens_both_fences() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path();
    let id = add(data_dir, CaptureTarget::Current, "gateway", "old text");
    let base = load(data_dir, &id).unwrap().content;
    let revised = base
        .replacen("2026-11-21 gateway-01", "new title", 1)
        .replace("old text", "user edited  \nlog with inline ```  ")
        + "\n";
    update(data_dir, &id, &revised, &base).unwrap();
    let long = format!("{}\n`````\n tail  ", "多行数据".repeat(10000));
    add(data_dir, CaptureTarget::Current, "gateway", &long);
    let expected = format!("new title\n\n## gateway-01\n\n``````\nuser edited  \nlog with inline ```  \n\n{long}\n``````\n");
    assert_eq!(load(data_dir, &id).unwrap().content, expected);
    let with_note = format!("{expected}\n判断");
    update(data_dir, &id, &with_note, &expected).unwrap();
    add(data_dir, CaptureTarget::Current, "gateway", "after note");
    assert_eq!(
        load(data_dir, &id).unwrap().content,
        format!("{with_note}\n\n## gateway-01\n\n```\nafter note\n```")
    );
}

#[test]
fn stale_title_edit_survives_widening_and_later_session_switches() {
    let dir = tempfile::tempdir().unwrap();
    let editor = dir.path();
    let terminal = dir.path();
    let id = add(editor, CaptureTarget::Current, "gateway", "first");
    let base = load(editor, &id).unwrap().content;
    add(
        terminal,
        CaptureTarget::Current,
        "gateway",
        " ```second\nline ",
    );
    add(
        terminal,
        CaptureTarget::Current,
        "gateway",
        "\nnext\n\nparagraph",
    );
    add(terminal, CaptureTarget::Current, "app", "third");
    let stored = load(terminal, &id).unwrap().content;
    assert!(stored.contains("first\n\n ```second\nline \n\n\nnext\n\nparagraph"));
    let draft = base.replacen("2026-11-21 gateway-01", "new title", 1);
    update(editor, &id, &draft, &base).unwrap();
    let report = load(editor, &id).unwrap();
    assert_eq!(report.title, "new title");
    assert_eq!(
        report.content,
        stored.replacen("2026-11-21 gateway-01", "new title", 1)
    );
    add(terminal, CaptureTarget::Current, "app", "fourth");
    assert!(load(editor, &id)
        .unwrap()
        .content
        .ends_with("\nthird\n\nfourth\n```"));
}

#[test]
fn editing_out_an_identical_session_group_does_not_assign_its_identity_to_the_previous_one() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path();
    let id = add(data_dir, CaptureTarget::Current, "session-one", "同一行");
    let first_group = load(data_dir, &id).unwrap().content;
    add(data_dir, CaptureTarget::Current, "session-two", "同一行");
    let previous = load(data_dir, &id).unwrap().content;
    let edited = first_group.replacen("2026-11-21 gateway-01", "新标题", 1);
    update(data_dir, &id, &edited, &previous).unwrap();
    add(data_dir, CaptureTarget::Current, "session-two", "from two");
    let final_report = load(data_dir, &id).unwrap().content;
    assert_eq!(
        final_report,
        format!("{edited}\n\n## gateway-01\n\n```\nfrom two\n```")
    );
}

#[test]
fn concurrent_capture_can_merge_a_renamed_title_and_a_judgment_below_the_original_block() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path();
    let id = add(data_dir, CaptureTarget::Current, "gateway", "before");
    let previous = load(data_dir, &id).unwrap().content;
    let old_title = "2026-11-21 gateway-01";
    let draft = previous.replacen(old_title, "判断", 1) + "\n\n人工补充";
    add(data_dir, CaptureTarget::Current, "gateway", "after ```");
    let stored = load(data_dir, &id).unwrap().content;
    update(data_dir, &id, &draft, &previous).unwrap();
    assert_eq!(
        load(data_dir, &id).unwrap().content,
        stored.replacen(old_title, "判断", 1) + "\n\n人工补充"
    );
}

#[test]
fn deleting_by_id_keeps_or_replaces_the_current_report_in_list_order() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path();
    let oldest = add(data_dir, CaptureTarget::New, "gw", "one");
    let current = add(data_dir, CaptureTarget::New, "gw", "two");
    let other = add(data_dir, CaptureTarget::New, "gw", "three");
    let newest = add(data_dir, CaptureTarget::New, "gw", "four");
    {
        let mut store = Store::open(data_dir).unwrap();
        for record in &mut store.index.reports {
            record.created_at = 42;
        }
        store.commit().unwrap();
    }
    // Identical titles: deleting one must always target its ID alone.
    select(data_dir, &current).unwrap();
    for _ in 0..2 {
        let reports = delete(data_dir, &oldest).unwrap();
        assert_eq!(reports.len(), 3);
        assert_eq!(reports.iter().find(|r| r.is_current).unwrap().id, current);
        assert_eq!(reports[0].id, newest);
    }
    let reports = delete(data_dir, &current).unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0].id, newest);
    assert!(reports[0].is_current);
    assert_eq!(
        add(data_dir, CaptureTarget::Current, "gw", "continued"),
        newest
    );
    let last = delete(data_dir, &newest).unwrap();
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].id, other);
    assert!(last[0].is_current);
    assert!(delete(data_dir, &other).unwrap().is_empty());
    assert!(delete(data_dir, &other).unwrap().is_empty());
    let fresh = add(data_dir, CaptureTarget::Current, "gw", "fresh");
    assert!(![oldest, current, other, newest].contains(&fresh));
    assert_eq!(list(data_dir).unwrap().len(), 1);
}

#[test]
fn deleting_current_while_another_window_appends_never_retargets_the_excerpt() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path();
    let other = dir.path().to_owned();
    let keep = add(data_dir, CaptureTarget::New, "gw", "keep this");
    let remove = add(data_dir, CaptureTarget::New, "gw", "delete this");
    let original = load(data_dir, &keep).unwrap().content;
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
    let remaining = delete(data_dir, &remove).unwrap();
    if let Err(error) = writer.join().unwrap() {
        assert_eq!(error.code(), "incident_report_not_found");
    }
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, keep);
    assert!(remaining[0].is_current);
    assert_eq!(
        load(data_dir, &remove).unwrap_err().code(),
        "incident_report_not_found"
    );
    assert_eq!(load(data_dir, &keep).unwrap().content, original);
    assert_eq!(
        add(data_dir, CaptureTarget::Current, "gw", "confirmed"),
        keep
    );
}

#[test]
fn concurrent_first_captures_share_one_report_without_losing_entries() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path();
    let other = dir.path().to_owned();
    let start = std::sync::Arc::new(std::sync::Barrier::new(2));
    let ready = start.clone();
    let writer = std::thread::spawn(move || {
        ready.wait();
        add(&other, CaptureTarget::Current, "1", "one")
    });
    start.wait();
    let id = add(data_dir, CaptureTarget::Current, "2", "two");
    assert_eq!(writer.join().unwrap(), id);
    assert_eq!(list(data_dir).unwrap().len(), 1);
    let report = load(data_dir, &id).unwrap();
    assert!(report.content.contains("\none\n"));
    assert!(report.content.contains("\ntwo\n"));
    assert_eq!(report.content.matches("## gateway-01").count(), 2);
}

#[test]
fn markdown_is_a_separate_private_file_and_listing_only_reads_the_index() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let id = add(dir.path(), CaptureTarget::New, "s1", "初始正文");
    let original = load(dir.path(), &id).unwrap().content;
    let content = "   新标题 \r\n\n外部可读的 UTF-8 正文\n尾空格  \n";
    update(dir.path(), &id, content, &original).unwrap();
    assert!(!dir.path().join("rssh.db").exists());
    let folder = dir.path().join("incident-reports");
    let files: Vec<_> = fs::read_dir(&folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 3);
    let markdown = files
        .iter()
        .find(|p| p.extension().is_some_and(|ext| ext == "md"))
        .unwrap();
    assert_eq!(fs::read(markdown).unwrap(), content.as_bytes());
    assert!(!fs::read_to_string(folder.join("index.json"))
        .unwrap()
        .contains("外部可读"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&folder).unwrap().permissions().mode() & 0o077,
            0
        );
        for file in &files {
            assert_eq!(fs::metadata(file).unwrap().permissions().mode() & 0o077, 0);
        }
    }
    // Invalid UTF-8 prevents reading the body. Listing and selecting other
    // reports still work using metadata alone.
    fs::write(markdown, [0xff]).unwrap();
    assert_eq!(list(dir.path()).unwrap()[0].title, "新标题");
    assert!(load(dir.path(), &id).is_err());
    let other = add(dir.path(), CaptureTarget::New, "s2", "second body");
    assert!(load(dir.path(), "../index.json").is_err());
    assert_eq!(delete(dir.path(), "../index.json").unwrap().len(), 2);
    assert_eq!(select(dir.path(), &other).unwrap()[0].id, other);
}

#[test]
fn interrupted_publication_keeps_the_previous_content_and_reclaims_only_owned_files() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let id = add(dir.path(), CaptureTarget::New, "gw", "committed");
    let folder = dir.path().join("incident-reports");
    let index = folder.join("index.json");
    let saved_index = folder.join("original.json");
    let foreign = folder.join("my-notes.md");
    fs::write(&foreign, "keep").unwrap();
    {
        let mut store = Store::open(dir.path()).unwrap();
        // Simulate a filesystem refusing replacement after the new .md has
        // reached disk, then put the last committed index back in place.
        fs::rename(&index, &saved_index).unwrap();
        fs::create_dir(&index).unwrap();
        assert!(store.write(0, "unpublished").is_err());
        fs::remove_dir(&index).unwrap();
        fs::rename(&saved_index, &index).unwrap();
        assert_eq!(
            fs::read_dir(&folder).unwrap().count(),
            4,
            "failed publication should reclaim both staged files immediately"
        );
    }
    assert!(load(dir.path(), &id).unwrap().content.contains("committed"));
    // The process could also have exited before it could run its error path.
    let orphan = folder.join(format!("{id}.{}.md", uuid::Uuid::new_v4()));
    let pending = folder.join(format!("index.{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&orphan, "uncommitted revision").unwrap();
    fs::write(&pending, "unpublished index").unwrap();
    // Preparation alone makes room for the next capture.
    drop(Store::open_for_write(dir.path()).unwrap());
    assert!(!orphan.exists());
    assert!(!pending.exists());
    let saved = fs::read(&index).unwrap();
    fs::write(&index, b"{truncated").unwrap();
    assert!(list(dir.path()).is_err());
    assert!(delete(dir.path(), &id).is_err());
    fs::write(&index, saved).unwrap();
    add(dir.path(), CaptureTarget::Current, "gw", "later");
    let final_content = load(dir.path(), &id).unwrap().content;
    assert!(final_content.contains("committed\n\nlater"));
    assert!(!final_content.contains("unpublished"));
    assert_eq!(fs::read_dir(folder).unwrap().count(), 4);
    assert_eq!(fs::read_to_string(foreign).unwrap(), "keep");
}

#[test]
fn configuration_import_and_export_leave_local_reports_private() {
    use crate::sync::config::{build_payload, merge_import, read_sync_prefs, ExportMode};
    let dir = tempfile::tempdir().unwrap();
    let db = std::sync::Arc::new(crate::db::Db::open(dir.path()).unwrap());
    let secrets = crate::secret::DbStore::new(db.clone());
    let id = add(
        dir.path(),
        CaptureTarget::Current,
        "gateway",
        "private request payload",
    );
    for mode in [
        ExportMode::LocalBackup,
        ExportMode::RemotePush(read_sync_prefs(&db).unwrap()),
    ] {
        let payload = build_payload(&db, &secrets, dir.path(), &mode).unwrap();
        let serialized = payload.to_string();
        assert!(!serialized.contains("private request payload"));
        assert!(!serialized.contains(&id));
        merge_import(&db, &secrets, dir.path(), &payload).unwrap();
        assert!(load(dir.path(), &id)
            .unwrap()
            .content
            .contains("private request payload"));
    }
}

#[test]
fn missing_index_preserves_existing_markdown_until_the_index_is_restored() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let id = add(dir.path(), CaptureTarget::New, "session", "saved body");
    let folder = dir.path().join("incident-reports");
    let index = folder.join("index.json");
    let backup = fs::read(&index).unwrap();
    fs::remove_file(&index).unwrap();
    assert_eq!(
        list(dir.path()).unwrap_err().code(),
        "incident_report_missing_index"
    );
    let excerpt = Excerpt {
        text: "new".into(),
        session_id: "s".into(),
        session_name: "host".into(),
    };
    assert!(capture(dir.path(), &excerpt, CaptureTarget::New, "new").is_err());
    assert!(!index.exists());
    fs::write(index, backup).unwrap();
    assert!(load(dir.path(), &id)
        .unwrap()
        .content
        .contains("saved body"));
}
