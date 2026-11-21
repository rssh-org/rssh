//! Completed files live directly under the mobile RSSH downloads directory.
//! The directory itself is the index, including backups left by failed commits.
use std::fs::{self, File, Metadata};
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedDownload {
    name: String,
    local_path: PathBuf,
    size: u64,
    identity: String,
    is_backup: bool,
}

pub(super) fn invalid_path() -> AppError {
    AppError::other(
        "file_path_invalid",
        json!({"err": "Expected a file in RSSH downloads"}),
    )
}

fn changed(path: &Path) -> AppError {
    AppError::sftp("sftp_target_changed", json!({ "path": path }))
}

fn transfer_suffix(name: &str) -> Option<&str> {
    let (id, suffix) = name.strip_prefix(".rssh-download-")?.rsplit_once('.')?;
    // Only the hyphenated UUID format emitted by LocalPartial is reserved.
    if id.len() == 36 && Uuid::try_parse(id).is_ok() {
        Some(suffix)
    } else {
        None
    }
}

pub(super) fn destination_name(name: &str) -> bool {
    super::valid_display_name(name) && !matches!(transfer_suffix(name), Some("part" | "bak"))
}

fn identity(meta: &Metadata) -> String {
    let birth = meta
        .created()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|time| time.as_nanos())
        .unwrap_or_default();
    format!(
        "{}:{}:{}:{}:{}:{}",
        meta.dev(),
        meta.ino(),
        birth,
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec()
    )
}

pub(super) fn child(root: &Path, path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    if !super::valid_display_name(name) {
        return None;
    }
    let parent = path.parent()?;
    // UIKit may use /var while the normalized destination starts /private/var.
    if parent != root && parent != root.canonicalize().ok()? {
        return None;
    }
    Some(root.join(name))
}

pub(super) fn list(root: &Path) -> AppResult<Vec<SavedDownload>> {
    match fs::symlink_metadata(root) {
        Ok(meta) if !meta.is_dir() => return Err(invalid_path()),
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
        _ => {}
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !super::valid_display_name(&name) || transfer_suffix(&name) == Some("part") {
            continue;
        }
        let path = entry.path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_file() => meta,
            Ok(_) => continue,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        files.push(SavedDownload {
            is_backup: transfer_suffix(&name) == Some("bak"),
            name,
            local_path: path,
            size: meta.len(),
            identity: identity(&meta),
        });
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

pub(super) fn open_verified(root: &Path, path: &Path, expected: &str) -> AppResult<File> {
    let path = child(root, path).ok_or_else(invalid_path)?;
    if !fs::symlink_metadata(root)?.is_dir()
        || path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(transfer_suffix)
            == Some("part")
    {
        return Err(invalid_path());
    }
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_file() {
        return Err(invalid_path());
    }
    if identity(&meta) != expected {
        return Err(changed(&path));
    }
    // Export the verified open descriptor, even if a later download replaces
    // the path while the user is choosing a document provider.
    let file = File::open(&path)?;
    if identity(&file.metadata()?) != expected {
        return Err(changed(&path));
    }
    Ok(file)
}

// UIKit reads the URL asynchronously, after the picker has been presented.
#[cfg(any(ios, test))]
#[derive(Debug)]
pub(super) struct PickerFile {
    pub path: PathBuf,
}

#[cfg(any(ios, test))]
impl PickerFile {
    pub fn create(source: &Path, mut file: File, temporary_root: &Path) -> AppResult<Self> {
        let folder = temporary_root.join(format!("rssh-export-{}", Uuid::new_v4()));
        fs::create_dir(&folder)?;
        let name = source.file_name().ok_or_else(invalid_path)?;
        let snapshot = Self {
            path: folder.join(name),
        };
        let expected = identity(&file.metadata()?);
        if fs::hard_link(source, &snapshot.path).is_ok() {
            let linked = fs::symlink_metadata(&snapshot.path)?;
            if linked.is_file() && identity(&linked) == expected {
                return Ok(snapshot);
            }
            // A concurrent replacement won the link race. Copy the pinned
            // descriptor instead of presenting the new occupant's contents.
            fs::remove_file(&snapshot.path)?;
        }
        let mut output = File::create_new(&snapshot.path)?;
        std::io::copy(&mut file, &mut output)?;
        if identity(&file.metadata()?) != expected {
            return Err(changed(source));
        }
        Ok(snapshot)
    }
}

#[cfg(any(ios, test))]
impl Drop for PickerFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        if let Some(folder) = self.path.parent() {
            let _ = fs::remove_dir(folder);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relisted_files_include_backups_and_ordinary_part_names() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("downloads");
        assert!(list(&missing).unwrap().is_empty());
        fs::create_dir(&missing).unwrap();
        let token = Uuid::new_v4();
        let part = format!(".rssh-download-{token}.part");
        let backup = format!(".rssh-download-{token}.bak");
        let ordinary_part = "document.part";
        for name in [&part[..], &backup, ordinary_part] {
            fs::write(missing.join(name), b"content").unwrap();
        }
        let files = list(&missing).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].name, backup);
        assert!(files[0].is_backup);
        assert_eq!(files[1].name, ordinary_part);
        assert!(!destination_name(&part) && !destination_name(&backup));
        assert!(destination_name(ordinary_part));
        assert!(open_verified(&missing, &missing.join(part), "unused").is_err());
        assert!(open_verified(&missing, &files[0].local_path, &files[0].identity).is_ok());
    }

    #[test]
    fn symlinks_and_paths_outside_the_library_cannot_be_exported() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("downloads");
        fs::create_dir(&root).unwrap();
        let other = parent.path().join("outside");
        fs::write(&other, b"outside").unwrap();
        std::os::unix::fs::symlink(&other, root.join("link")).unwrap();
        fs::create_dir(root.join("directory")).unwrap();
        assert!(list(&root).unwrap().is_empty());
        let unrelated = identity(&fs::metadata(&other).unwrap());
        for path in [&other, &root.join("link"), &root.join("../outside")] {
            assert!(open_verified(&root, path, &unrelated).is_err());
        }
        let alias = parent.path().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        assert!(list(&alias).is_err());
    }

    #[test]
    fn a_stale_entry_cannot_export_a_replacement_and_a_picker_keeps_its_original() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("report.txt");
        fs::write(&path, b"original").unwrap();
        let entry = list(root.path()).unwrap().pop().unwrap();
        let open = open_verified(root.path(), &path, &entry.identity).unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let snapshot = PickerFile::create(&path, open, temporary.path()).unwrap();
        let next = root.path().join("new");
        fs::write(&next, b"replaced").unwrap();
        fs::rename(next, &path).unwrap();
        assert_eq!(
            open_verified(root.path(), &path, &entry.identity)
                .unwrap_err()
                .code(),
            "sftp_target_changed"
        );
        assert_eq!(fs::read(&snapshot.path).unwrap(), b"original");
        drop(snapshot);
        assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 0);
    }
}
