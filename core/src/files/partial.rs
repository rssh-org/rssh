//! Local half of a resumable SFTP download. The queue retains this ownership
//! token between attempts, after the writer has been flushed and closed.
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::json;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileId {
    volume: u64,
    index: u128,
    created: Option<SystemTime>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct FileSnapshot {
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

fn snapshot(path: &Path) -> io::Result<Option<FileSnapshot>> {
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
    Ok(Some(FileSnapshot {
        id,
        len: meta.len(),
        modified: meta.modified()?,
    }))
}

impl FileSnapshot {
    pub fn from_file(file: &File) -> io::Result<Self> {
        let meta = file.metadata()?;
        Ok(Self {
            id: local_file_id(file)?,
            len: meta.len(),
            modified: meta.modified()?,
        })
    }
}

/// Operations on siblings of a selected download destination. Filesystem paths
/// use the native defaults. A document provider interprets only the final name
/// within its authorized directory and returns an owned, seekable descriptor.
pub trait DownloadDirectory: Send + Sync + std::fmt::Debug {
    /// Document providers may identify a target separately from its display path.
    fn document_key(&self) -> Option<String> {
        None
    }
    fn snapshot(&self, path: &Path) -> io::Result<Option<FileSnapshot>> {
        snapshot(path)
    }
    fn create(&self, path: &Path) -> io::Result<File> {
        open_options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
    }
    fn open(&self, path: &Path) -> io::Result<File> {
        open_options().read(true).write(true).open(path)
    }
    /// Release any provider-owned writer after its Rust descriptor has closed.
    fn close_writer(&self, _path: &Path) -> io::Result<()> {
        Ok(())
    }
    /// Only the first publish may directly replace the verified old target on
    /// a provider that supports it. Backup and recovery moves cannot overwrite.
    fn rename(&self, from: &Path, to: &Path, allow_existing: bool) -> io::Result<()> {
        if allow_existing {
            fs::rename(from, to)
        } else {
            rename_exclusive(from, to)
        }
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
    fn discard(self: Arc<Self>, path: PathBuf, id: FileId) {
        if let Err(error) = discard_owned_file(&*self, &path, &id) {
            log::warn!(
                "Cannot clean up partial download {}: {error}",
                path.display()
            );
        }
    }
}

#[derive(Debug)]
pub struct NativeDownloadDirectory;
impl DownloadDirectory for NativeDownloadDirectory {}

#[derive(Debug)]
pub struct PartialWriter {
    file: Option<tokio::fs::File>,
    directory: Arc<dyn DownloadDirectory>,
    path: PathBuf,
    runtime: tokio::runtime::Handle,
}

impl PartialWriter {
    fn new(file: File, directory: Arc<dyn DownloadDirectory>, path: &Path) -> Self {
        Self {
            file: Some(tokio::fs::File::from_std(file)),
            directory,
            path: path.to_owned(),
            runtime: tokio::runtime::Handle::current(),
        }
    }

    pub fn stream(&mut self) -> &mut tokio::fs::File {
        self.file.as_mut().expect("partial writer is open")
    }

    fn start_close(&mut self) -> Option<tokio::task::JoinHandle<io::Result<()>>> {
        let mut file = self.file.take()?;
        let directory = self.directory.clone();
        let path = self.path.clone();
        // This task owns both handles even if its caller is cancelled while
        // Tokio is still writing. Some providers commit buffered data on close.
        Some(self.runtime.spawn(async move {
            use tokio::io::AsyncWriteExt;
            let flushed = file.shutdown().await;
            drop(file.into_std().await);
            let closed = directory.close_writer(&path);
            if let Err(error) = &closed {
                log::warn!(
                    "Could not close partial download {}: {error}",
                    path.display()
                );
            }
            flushed.and(closed)
        }))
    }

    pub async fn close(mut self) -> io::Result<()> {
        self.start_close().expect("partial writer is open").await?
    }
}

impl Drop for PartialWriter {
    fn drop(&mut self) {
        drop(self.start_close());
    }
}

pub fn discard_owned_file<D: DownloadDirectory + ?Sized>(
    directory: &D,
    path: &Path,
    id: &FileId,
) -> io::Result<()> {
    if directory.snapshot(path)?.is_some_and(|file| &file.id == id) {
        directory.remove(path)?;
    }
    Ok(())
}

fn folder_access_error(error: io::Error) -> AppError {
    match error.kind() {
        ErrorKind::PermissionDenied => {
            AppError::sftp("sftp_provider_siblings_unavailable", json!({}))
        }
        ErrorKind::Unsupported => AppError::sftp("sftp_folder_operations_unsupported", json!({})),
        _ => error.into(),
    }
}

pub fn normalized_destination(path: &Path) -> AppResult<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "Missing file name"))?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(folder_access_error)?;
    Ok(fs::canonicalize(parent)
        .map_err(folder_access_error)?
        .join(name))
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
    directory: Arc<dyn DownloadDirectory>,
    temporary: PathBuf,
    destination: PathBuf,
    id: FileId,
    original: Option<FileSnapshot>,
    backup: Option<PathBuf>,
    installed: bool,
}

impl LocalPartial {
    pub fn create(destination: &Path) -> AppResult<(Self, PartialWriter)> {
        let destination = normalized_destination(destination)?;
        Self::create_in(destination, Arc::new(NativeDownloadDirectory))
    }

    pub(crate) fn create_in(
        destination: PathBuf,
        directory: Arc<dyn DownloadDirectory>,
    ) -> AppResult<(Self, PartialWriter)> {
        let original = directory
            .snapshot(&destination)
            .map_err(folder_access_error)?;
        // Generate names independently of the final name and user-supplied
        // transfer ID. CREATE_NEW is authoritative even for a UUID collision.
        let temporary =
            destination.with_file_name(format!(".rssh-download-{}.part", Uuid::new_v4()));
        let file = directory.create(&temporary).map_err(folder_access_error)?;
        let identified = local_file_id(&file);
        let writer = PartialWriter::new(file, directory.clone(), &temporary);
        let id = identified?;
        Ok((
            Self {
                directory,
                temporary,
                destination,
                id,
                original,
                backup: None,
                installed: false,
            },
            writer,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.temporary
    }

    pub(crate) fn backup_path(&self) -> PathBuf {
        // Recovery files must be visible in the system folder picker.
        let stem = self
            .temporary
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("generated ASCII part name");
        self.temporary
            .with_file_name(format!("{}.bak", stem.trim_start_matches('.')))
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

    pub fn reopen(&self) -> AppResult<PartialWriter> {
        if let Some(backup) = &self.backup {
            // Once the old file has moved, keep the recovery path visible on
            // every retry. A new occupant must never become another backup.
            if self.installed || self.directory.snapshot(&self.destination)?.is_some() {
                return Err(self.backup_error(backup, "Destination changed during replacement"));
            }
        } else if self
            .directory
            .snapshot(&self.destination)
            .map_err(folder_access_error)?
            != self.original
        {
            return Err(changed("sftp_target_changed", &self.destination));
        }
        let file = self
            .directory
            .open(&self.temporary)
            .map_err(folder_access_error)?;
        let identified = local_file_id(&file);
        let writer = PartialWriter::new(file, self.directory.clone(), &self.temporary);
        if identified? != self.id {
            return Err(changed("sftp_part_changed", &self.temporary));
        }
        Ok(writer)
    }

    fn verify_file(&self, path: &Path, bytes: u64) -> io::Result<()> {
        if self
            .directory
            .snapshot(path)?
            .is_some_and(|file| file.id == self.id && file.len == bytes)
        {
            Ok(())
        } else {
            Err(io::Error::other(
                "File identity or length changed during replacement",
            ))
        }
    }

    pub fn commit(&mut self, bytes: u64) -> AppResult<()> {
        let directory = self.directory.clone();
        self.commit_with(bytes, |from, to, allow| directory.rename(from, to, allow))
    }

    fn commit_with(
        &mut self,
        bytes: u64,
        rename: impl Fn(&Path, &Path, bool) -> io::Result<()>,
    ) -> AppResult<()> {
        if !self.installed {
            if !self
                .directory
                .snapshot(&self.temporary)
                .map_err(folder_access_error)?
                .is_some_and(|part| part.id == self.id && part.len == bytes)
            {
                return Err(changed("sftp_part_changed", &self.temporary));
            }
            if self.backup.is_none() {
                if self
                    .directory
                    .snapshot(&self.destination)
                    .map_err(folder_access_error)?
                    != self.original
                {
                    return Err(changed("sftp_target_changed", &self.destination));
                }
                match rename(&self.temporary, &self.destination, self.original.is_some()) {
                    Ok(()) => {
                        self.verify_file(&self.destination, bytes).map_err(|e| {
                            AppError::sftp("sftp_publish_failed", json!({
                                "target": self.destination, "part": self.temporary, "err": e.to_string()
                            }))
                        })?;
                        self.installed = true;
                        return Ok(());
                    }
                    Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                    Err(e)
                        if matches!(
                            e.kind(),
                            ErrorKind::PermissionDenied | ErrorKind::Unsupported
                        ) =>
                    {
                        return Err(folder_access_error(e))
                    }
                    Err(e) => {
                        return Err(AppError::sftp(
                            "sftp_publish_failed",
                            json!({
                                "target": self.destination, "part": self.temporary, "err": e.to_string()
                            }),
                        ))
                    }
                }
                if self.original.is_none()
                    || self
                        .directory
                        .snapshot(&self.destination)
                        .map_err(folder_access_error)?
                        != self.original
                {
                    return Err(changed("sftp_target_changed", &self.destination));
                }
                // A transfer-specific backup also avoids using another queued
                // download's ordinary target (such as “report.bak”) as scratch.
                let backup = self.backup_path();
                rename(&self.destination, &backup, false).map_err(|e| match e.kind() {
                    ErrorKind::PermissionDenied | ErrorKind::AlreadyExists => {
                        folder_access_error(e)
                    }
                    _ => AppError::sftp(
                        "sftp_backup_failed",
                        json!({
                            "target": self.destination, "backup": backup, "err": e.to_string()
                        }),
                    ),
                })?;
                self.backup = Some(backup);
            }
            let backup = self.backup.as_ref().expect("original was moved");
            if self
                .directory
                .snapshot(backup)
                .map_err(|e| self.backup_error(backup, e))?
                != self.original
            {
                return Err(self.backup_error(backup, "Backup identity changed"));
            }
            self.verify_file(&self.temporary, bytes)
                .map_err(|e| self.backup_error(backup, e))?;
            rename(&self.temporary, &self.destination, false)
                .map_err(|e| self.backup_error(backup, e))?;
        }
        if let Some(backup) = &self.backup {
            self.verify_file(&self.destination, bytes).map_err(|e| {
                AppError::sftp(
                    "sftp_replace_failed",
                    json!({
                        "target": self.destination, "backup": backup, "err": e.to_string()
                    }),
                )
            })?;
            self.installed = true;
            if self
                .directory
                .snapshot(backup)
                .map_err(|e| self.backup_error(backup, e))?
                != self.original
            {
                return Err(self.backup_error(backup, "Backup identity changed"));
            }
            self.directory
                .remove(backup)
                .map_err(|e| self.backup_error(backup, e))?;
            self.backup = None;
        }
        Ok(())
    }
}

impl Drop for LocalPartial {
    fn drop(&mut self) {
        if !self.installed {
            self.directory
                .clone()
                .discard(self.temporary.clone(), self.id.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    async fn downloaded(path: &Path, content: &[u8]) -> LocalPartial {
        let (partial, mut file) = LocalPartial::create(path).unwrap();
        file.stream().write_all(content).await.unwrap();
        file.close().await.unwrap();
        partial
    }

    #[tokio::test]
    async fn provider_receives_completed_writes_on_close_and_task_drop() {
        #[derive(Debug)]
        struct Observed(tokio::sync::mpsc::UnboundedSender<Vec<u8>>);
        impl DownloadDirectory for Observed {
            fn close_writer(&self, path: &Path) -> io::Result<()> {
                self.0.send(fs::read(path)?).unwrap();
                Ok(())
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let provider = Arc::new(Observed(tx));
        let destination = directory.path().join("report");
        let (_partial, mut writer) =
            LocalPartial::create_in(destination.clone(), provider.clone()).unwrap();
        writer.stream().write_all(b"interrupted").await.unwrap();
        drop(writer);
        assert_eq!(rx.recv().await.unwrap(), b"interrupted");

        let (mut partial, mut writer) =
            LocalPartial::create_in(destination.clone(), provider).unwrap();
        writer.stream().write_all(b"finished").await.unwrap();
        writer.close().await.unwrap();
        assert_eq!(rx.try_recv().unwrap(), b"finished");
        partial.commit(b"finished".len() as u64).unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"finished");
    }

    #[tokio::test]
    async fn truncation_on_provider_close_preserves_the_original_target() {
        #[derive(Debug)]
        struct TruncatingProvider;
        impl DownloadDirectory for TruncatingProvider {
            fn close_writer(&self, path: &Path) -> io::Result<()> {
                OpenOptions::new().write(true).open(path)?.set_len(0)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, b"original").unwrap();
        let (mut partial, mut writer) =
            LocalPartial::create_in(target.clone(), Arc::new(TruncatingProvider)).unwrap();
        writer.stream().write_all(b"finished").await.unwrap();
        writer.close().await.unwrap();
        assert_eq!(partial.commit(8).unwrap_err().code(), "sftp_part_changed");
        assert_eq!(fs::read(target).unwrap(), b"original");
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
        transfer.commit(b"replacement".len() as u64).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"replacement");
        assert_eq!(fs::read(bak).unwrap(), b"private backup");
        assert_eq!(fs::read(part).unwrap(), b"private partial");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    }

    #[tokio::test]
    async fn native_overwrite_publishes_in_one_rename_even_if_the_backup_name_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, b"original").unwrap();
        let mut transfer = downloaded(&target, b"replacement").await;
        let backup = transfer.backup_path();
        fs::write(&backup, b"someone else's backup").unwrap();
        let calls = std::cell::Cell::new(0);
        transfer
            .commit_with(b"replacement".len() as u64, |from, to, allow_existing| {
                calls.set(calls.get() + 1);
                NativeDownloadDirectory.rename(from, to, allow_existing)
            })
            .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(fs::read(target).unwrap(), b"replacement");
        assert_eq!(fs::read(backup).unwrap(), b"someone else's backup");
    }

    #[tokio::test]
    async fn first_rename_denial_or_unknown_outcome_never_moves_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, b"original").unwrap();
        for error in [ErrorKind::PermissionDenied, ErrorKind::TimedOut] {
            let mut transfer = downloaded(&target, b"replacement").await;
            let calls = std::cell::Cell::new(0);
            let result = transfer.commit_with(b"replacement".len() as u64, |_, _, _| {
                calls.set(calls.get() + 1);
                Err(error.into())
            });
            assert!(result.is_err());
            assert_eq!(calls.get(), 1);
            assert_eq!(fs::read(&target).unwrap(), b"original");
            assert!(!transfer.backup_path().exists());
        }
    }

    #[tokio::test]
    async fn second_rename_failure_exposes_backup_and_can_retry_without_moving_another_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("target");
        fs::write(&dest, b"original").unwrap();
        let mut transfer = downloaded(&dest, b"replacement").await;
        let tmp = transfer.path().to_owned();
        let canonical_dest = transfer.destination.clone();
        let result = transfer.commit_with(b"replacement".len() as u64, |from, to, _| {
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
        assert!(!backup
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with('.'));
        assert_eq!(fs::read(&backup).unwrap(), b"original");
        fs::write(&dest, b"concurrent arrival").unwrap();
        assert_eq!(
            transfer
                .commit(b"replacement".len() as u64)
                .unwrap_err()
                .code(),
            "sftp_replace_failed"
        );
        assert_eq!(fs::read(&dest).unwrap(), b"concurrent arrival");
        assert_eq!(fs::read(&backup).unwrap(), b"original");
        fs::remove_file(&dest).unwrap();
        transfer.commit(b"replacement".len() as u64).unwrap();
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
        assert_eq!(
            transfer
                .commit(b"checkpoint".len() as u64)
                .unwrap_err()
                .code(),
            "sftp_part_changed"
        );
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
            let outcome = transfer.commit_with(b"downloaded".len() as u64, |from, to, _| {
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
        first.commit(5).unwrap();
        assert_eq!(second.commit(6).unwrap_err().code(), "sftp_target_changed");
        assert_eq!(fs::read(&dest).unwrap(), b"first");
    }
}
