use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;

use crate::error::{AppError, AppResult};
use crate::models::Snippet;

pub fn load(data_dir: &Path) -> AppResult<Vec<Snippet>> {
    let path = data_dir.join("snippets.json");
    let data = match std::fs::read_to_string(&path) {
        Ok(data) => data,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    // 文件存在但解析失败 = 用户数据可能损坏。早 fail 让用户察觉，比 silent 清空更好。
    let snippets: Vec<Snippet> = serde_json::from_str(&data).map_err(|e| {
        AppError::other(
            "snippet_parse_failed",
            serde_json::json!({
                "path": path.to_string_lossy(),
                "err": e.to_string(),
            }),
        )
    })?;
    validate_parameters(&snippets)?;
    Ok(snippets)
}

pub fn save(data_dir: &Path, snippets: &[Snippet]) -> AppResult<()> {
    let _lock = lock_for_write(data_dir)?;
    write_list(data_dir, snippets)
}

/// Apply one edit to the latest list. The previous row guards a draft against
/// reordering, deletion or edits from another window or configuration import.
pub fn update(
    data_dir: &Path,
    index: Option<usize>,
    previous: Option<Snippet>,
    snippet: Option<Snippet>,
) -> AppResult<Vec<Snippet>> {
    let _lock = lock_for_write(data_dir)?;
    let mut rows = load(data_dir)?;
    match (index, previous, snippet) {
        (None, None, Some(new)) => rows.push(new),
        (Some(i), Some(expected), replacement) => {
            if rows.get(i) != Some(&expected) {
                return Err(AppError::config("snippet_changed", serde_json::json!({})));
            }
            match replacement {
                Some(new) => rows[i] = new,
                None => {
                    rows.remove(i);
                }
            }
        }
        _ => {
            return Err(AppError::config(
                "snippet_update_invalid",
                serde_json::json!({}),
            ))
        }
    }
    write_list(data_dir, &rows)?;
    Ok(rows)
}

// Lock a stable sibling: replacing the JSON file changes its inode.
fn lock_for_write(data_dir: &Path) -> AppResult<File> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join("snippets.lock"))?;
    lock.lock()?;
    Ok(lock)
}

fn write_list(data_dir: &Path, snippets: &[Snippet]) -> AppResult<()> {
    validate_parameters(snippets)?;
    let path = data_dir.join("snippets.json");
    let data = serde_json::to_string_pretty(snippets).map_err(|e| {
        crate::error::AppError::other("serde_failed", serde_json::json!({ "err": e.to_string() }))
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(data_dir)?;
    temporary.write_all(data.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn validate_parameters(snippets: &[Snippet]) -> AppResult<()> {
    for snippet in snippets {
        let mut names = HashSet::new();
        for parameter in &snippet.parameters {
            if parameter.name.is_empty() || !names.insert(&parameter.name) {
                return Err(AppError::config(
                    "snippet_parameters_invalid",
                    serde_json::json!({ "name": snippet.name }),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plain(name: &str) -> Snippet {
        Snippet {
            name: name.into(),
            command: format!("echo {name}"),
            parameters: Vec::new(),
        }
    }

    #[test]
    fn literal_parameter_names_round_trip_and_invalid_edits_leave_the_file_intact() {
        let dir = tempfile::tempdir().unwrap();
        let valid: Snippet = serde_json::from_value(json!({
            "name": "test", "command": "echo ' a b ' ' ' '{x}'\r\necho 'first\r\nsecond'",
            "parameters": [
                {"name": " a b ", "defaultValue": " A B "},
                {"name": " "},
                {"name": "{x}", "defaultValue": null},
                {"name": "first\r\nsecond", "defaultValue": "one\ntwo\rthree"},
            ],
        }))
        .unwrap();
        assert_eq!(valid.parameters[1].default_value, None);
        save(dir.path(), std::slice::from_ref(&valid)).unwrap();
        for name in ["", "{x}", "first\r\nsecond"] {
            let mut invalid = valid.clone();
            invalid.parameters[0].name = name.into();
            let err = save(dir.path(), &[invalid]).unwrap_err();
            assert_eq!(err.code(), "snippet_parameters_invalid");
        }
        assert_eq!(load(dir.path()).unwrap(), [valid]);
    }

    #[test]
    fn concurrent_appends_do_not_overwrite_draft_edits_or_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let old = plain("old");
        save(path, std::slice::from_ref(&old)).unwrap();

        let barrier = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            for name in ["one", "two", "three"] {
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    update(path, None, None, Some(plain(name))).unwrap();
                });
            }
            barrier.wait();
            update(path, Some(0), Some(old.clone()), Some(plain("edited"))).unwrap();
        });

        let rows = load(path).unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0], plain("edited"));
        for name in ["one", "two", "three"] {
            assert!(rows.contains(&plain(name)));
        }
        // Editing/deleting an outdated row cannot overwrite the newer list.
        for replacement in [None, Some(plain("stale"))] {
            let err = update(path, Some(0), Some(old.clone()), replacement).unwrap_err();
            assert_eq!(err.code(), "snippet_changed");
        }
        assert_eq!(load(path).unwrap(), rows);
    }
}
