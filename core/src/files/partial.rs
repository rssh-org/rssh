//! Local half of a resumable SFTP download. The queue retains this ownership
//! token between attempts, after the writer has been flushed and closed.
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::json;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

#[derive(Debug, PartialEq, Eq)]
pub struct FileId {
    volume: u64,
    index: u128,
    created: Option<SystemTime>,
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    id: FileId,
    len: u64,
    modified: SystemTime,
}

#[cfg(any(macos, ios, linux, android, ohos))]
fn file_id(meta: &Metadata) -> FileId {
    use std::os::unix::fs::MetadataExt;
    FileId {
        volume: meta.dev(),
        index: u128::from(meta.ino()),
        created: meta.created().ok(),
    }
}

#[cfg(windows)]
fn windows_id(file: &File, meta: &Metadata) -> io::Result<FileId> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
    };
    let mut info = FILE_ID_INFO::default();
    // The buffer has the exact FILE_ID_INFO layout requested by FileIdInfo and
    // the borrowed handle remains valid for the duration of the call.
    let valid = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&raw mut info).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    };
    if valid == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(FileId {
        volume: info.VolumeSerialNumber,
        index: u128::from_ne_bytes(info.FileId.Identifier),
        created: meta.created().ok(),
    })
}

fn open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(any(macos, ios, linux, android, ohos))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options
}

pub fn local_file_id(file: &File) -> io::Result<FileId> {
    let meta = file.metadata()?;
    check_regular(&meta)?;
    #[cfg(any(macos, ios, linux, android, ohos))]
    return Ok(file_id(&meta));
    #[cfg(windows)]
    return windows_id(file, &meta);
}

fn check_regular(meta: &Metadata) -> io::Result<()> {
    if !meta.is_file() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "Expected a regular file",
        ));
    }
    Ok(())
}

fn snapshot(path: &Path) -> io::Result<Option<Snapshot>> {
    let meta = match fs::symlink_metadata(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        result => result?,
    };
    check_regular(&meta)?;
    #[cfg(any(macos, ios, linux, android, ohos))]
    let id = file_id(&meta);
    #[cfg(windows)]
    let id = {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES;
        let file = open_options()
            .access_mode(FILE_READ_ATTRIBUTES)
            .open(path)?;
        local_file_id(&file)?
    };
    Ok(Some(Snapshot {
        id,
        len: meta.len(),
        modified: meta.modified()?,
    }))
}

pub fn normalized_destination(path: &Path) -> AppResult<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "Missing file name"))?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    Ok(fs::canonicalize(parent)?.join(name))
}

fn changed(code: &'static str, path: &Path) -> AppError {
    AppError::sftp(code, json!({ "path": path }))
}

#[cfg(any(macos, ios, linux, android, ohos))]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE)?;
    Ok(())
}

#[cfg(windows)]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileW;
    // Paths use canonical parent directories, including the extended-length
    // prefix provided by std on Windows. MoveFileW never replaces a file.
    let wide = |p: &Path| {
        p.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let from = wide(from);
    let to = wide(to);
    if unsafe { MoveFileW(from.as_ptr(), to.as_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[derive(Debug)]
pub struct LocalPartial {
    temporary: PathBuf,
    destination: PathBuf,
    id: FileId,
    original: Option<Snapshot>,
    backup: Option<PathBuf>,
    installed: bool,
}

impl LocalPartial {
    pub fn create(destination: &Path) -> AppResult<(Self, tokio::fs::File)> {
        let destination = normalized_destination(destination)?;
        let original = snapshot(&destination)?;
        // Generate names independently of the final name and user-supplied
        // transfer ID. CREATE_NEW is authoritative even for a UUID collision.
        let temporary =
            destination.with_file_name(format!(".rssh-download-{}.part", Uuid::new_v4()));
        let file = open_options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let id = local_file_id(&file)?;
        Ok((
            Self {
                temporary,
                destination,
                id,
                original,
                backup: None,
                installed: false,
            },
            tokio::fs::File::from_std(file),
        ))
    }

    pub fn path(&self) -> &Path {
        &self.temporary
    }

    pub(crate) fn backup_path(&self) -> PathBuf {
        self.temporary.with_extension("bak")
    }

    fn backup_error(&self, backup: &Path, error: impl std::fmt::Display) -> AppError {
        let code = if self.installed {
            "sftp_backup_cleanup_failed"
        } else {
            "sftp_replace_failed"
        };
        AppError::sftp(
            code,
            json!({ "target": self.destination, "backup": backup, "err": error.to_string() }),
        )
    }

    pub fn reopen(&self) -> AppResult<tokio::fs::File> {
        if let Some(backup) = &self.backup {
            // Once the old file has moved, keep the recovery path visible on
            // every retry. A new occupant must never become another backup.
            if self.installed || snapshot(&self.destination)?.is_some() {
                return Err(self.backup_error(backup, "Destination changed during replacement"));
            }
        } else if snapshot(&self.destination)? != self.original {
            return Err(changed("sftp_target_changed", &self.destination));
        }
        let file = open_options()
            .read(true)
            .write(true)
            .open(&self.temporary)?;
        if local_file_id(&file)? != self.id {
            return Err(changed("sftp_part_changed", &self.temporary));
        }
        Ok(tokio::fs::File::from_std(file))
    }

    fn verify_file(&self, path: &Path) -> io::Result<()> {
        if snapshot(path)?.is_some_and(|file| file.id == self.id) {
            Ok(())
        } else {
            Err(io::Error::other("File identity changed during replacement"))
        }
    }

    pub fn commit(&mut self) -> AppResult<()> {
        self.commit_with(rename_exclusive)
    }

    fn commit_with(&mut self, rename: impl Fn(&Path, &Path) -> io::Result<()>) -> AppResult<()> {
        if !self.installed {
            if !snapshot(&self.temporary)?.is_some_and(|part| part.id == self.id) {
                return Err(changed("sftp_part_changed", &self.temporary));
            }
            if self.backup.is_none() {
                if snapshot(&self.destination)? != self.original {
                    return Err(changed("sftp_target_changed", &self.destination));
                }
                match rename(&self.temporary, &self.destination) {
                    Ok(()) => {
                        self.verify_file(&self.destination).map_err(|e| {
                            AppError::sftp("sftp_publish_failed", json!({
                                "target": self.destination, "part": self.temporary, "err": e.to_string()
                            }))
                        })?;
                        self.installed = true;
                        return Ok(());
                    }
                    Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e.into()),
                }
                if self.original.is_none() || snapshot(&self.destination)? != self.original {
                    return Err(changed("sftp_target_changed", &self.destination));
                }
                // A transfer-specific backup also avoids using another queued
                // download's ordinary target (such as “report.bak”) as scratch.
                let backup = self.backup_path();
                rename(&self.destination, &backup)?;
                self.backup = Some(backup);
            }
            let backup = self.backup.as_ref().expect("original was moved");
            if snapshot(backup).map_err(|e| self.backup_error(backup, e))? != self.original {
                return Err(self.backup_error(backup, "Backup identity changed"));
            }
            self.verify_file(&self.temporary)
                .map_err(|e| self.backup_error(backup, e))?;
            rename(&self.temporary, &self.destination).map_err(|e| self.backup_error(backup, e))?;
        }
        if let Some(backup) = &self.backup {
            self.verify_file(&self.destination).map_err(|e| {
                AppError::sftp(
                    "sftp_replace_failed",
                    json!({
                        "target": self.destination, "backup": backup, "err": e.to_string()
                    }),
                )
            })?;
            self.installed = true;
            if snapshot(backup).map_err(|e| self.backup_error(backup, e))? != self.original {
                return Err(self.backup_error(backup, "Backup identity changed"));
            }
            fs::remove_file(backup).map_err(|e| self.backup_error(backup, e))?;
            self.backup = None;
        }
        Ok(())
    }
}

impl Drop for LocalPartial {
    fn drop(&mut self) {
        if !self.installed
            && matches!(snapshot(&self.temporary), Ok(Some(part)) if part.id == self.id)
        {
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    async fn downloaded(path: &Path, content: &[u8]) -> LocalPartial {
        let (partial, mut file) = LocalPartial::create(path).unwrap();
        file.write_all(content).await.unwrap();
        file.shutdown().await.unwrap();
        drop(file.into_std().await);
        partial
    }

    #[tokio::test]
    async fn commit_preserves_unrelated_temporary_and_backup_files() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("target");
        let bak = dir.path().join("target.bak");
        let part = dir.path().join("target.part");
        fs::write(&dest, b"original").unwrap();
        fs::write(&bak, b"private backup").unwrap();
        fs::write(&part, b"private partial").unwrap();
        let mut transfer = downloaded(&dest, b"replacement").await;
        transfer.commit().unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"replacement");
        assert_eq!(fs::read(bak).unwrap(), b"private backup");
        assert_eq!(fs::read(part).unwrap(), b"private partial");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    }

    #[tokio::test]
    async fn second_rename_failure_exposes_backup_and_can_retry_without_moving_another_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("target");
        fs::write(&dest, b"original").unwrap();
        let mut transfer = downloaded(&dest, b"replacement").await;
        let tmp = transfer.path().to_owned();
        let canonical_dest = transfer.destination.clone();
        let result = transfer.commit_with(|from, to| {
            if from == tmp && to == canonical_dest && !dest.exists() {
                return Err(ErrorKind::PermissionDenied.into());
            }
            rename_exclusive(from, to)
        });
        let AppError::Sftp(msg) = result.unwrap_err() else {
            panic!("missing recovery location")
        };
        let backup = PathBuf::from(msg.params["backup"].as_str().unwrap());
        assert_eq!(msg.code, "sftp_replace_failed");
        assert_eq!(fs::read(&backup).unwrap(), b"original");
        fs::write(&dest, b"concurrent arrival").unwrap();
        assert_eq!(transfer.commit().unwrap_err().code(), "sftp_replace_failed");
        assert_eq!(fs::read(&dest).unwrap(), b"concurrent arrival");
        assert_eq!(fs::read(&backup).unwrap(), b"original");
        fs::remove_file(&dest).unwrap();
        transfer.commit().unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"replacement");
        assert!(!backup.exists());
    }

    #[tokio::test]
    async fn changed_part_cannot_be_resumed_committed_or_unlinked() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("target");
        let mut transfer = downloaded(&dest, b"checkpoint").await;
        let path = transfer.path().to_owned();
        fs::rename(&path, dir.path().join("moved")).unwrap();
        fs::write(&path, b"unrelated").unwrap();
        assert_eq!(transfer.reopen().unwrap_err().code(), "sftp_part_changed");
        assert_eq!(transfer.commit().unwrap_err().code(), "sftp_part_changed");
        drop(transfer);
        assert_eq!(fs::read(path).unwrap(), b"unrelated");
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn file_swapped_during_replacement_preserves_the_original_backup() {
        for after_install in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let dest = dir.path().join("target");
            fs::write(&dest, b"original").unwrap();
            let mut transfer = downloaded(&dest, b"downloaded").await;
            let tmp = transfer.path().to_owned();
            let backup = transfer.backup_path();
            let outcome = transfer.commit_with(|from, to| {
                rename_exclusive(from, to)?;
                if (!after_install && to == backup) || (after_install && from == tmp) {
                    let replaced = if after_install { &dest } else { &tmp };
                    fs::rename(replaced, dir.path().join("moved"))?;
                    fs::write(replaced, b"unrelated")?;
                }
                Ok(())
            });
            assert_eq!(outcome.unwrap_err().code(), "sftp_replace_failed");
            assert_eq!(fs::read(backup).unwrap(), b"original");
            drop(transfer);
            assert_eq!(
                fs::read(if after_install { dest } else { tmp }).unwrap(),
                b"unrelated"
            );
        }
    }

    #[tokio::test]
    async fn concurrent_downloads_cannot_replace_a_new_arrival() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("target");
        let mut first = downloaded(&dest, b"first").await;
        let mut second = downloaded(&dest, b"second").await;
        first.commit().unwrap();
        assert_eq!(second.commit().unwrap_err().code(), "sftp_target_changed");
        assert_eq!(fs::read(&dest).unwrap(), b"first");
    }
}
