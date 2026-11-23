//! Queue checkpoints survive the SFTP channel. Each retry reopens both ends at
//! byte zero and verifies the saved prefix before it may append or publish.
use super::{SftpHandle, CANCELLED_CODE};
use crate::error::{AppError, AppResult};
use russh_sftp::{
    client::error::Error as SftpError,
    protocol::{FileAttributes, OpenFlags, StatusCode},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs::Metadata;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::fs::{self, File};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

fn changed(code: &'static str) -> AppError {
    AppError::sftp(code, json!({}))
}

fn io_error(op: &str, err: impl std::fmt::Display) -> AppError {
    AppError::sftp(
        "sftp_io_failed",
        json!({ "op": op, "err": err.to_string() }),
    )
}

fn publish_error(code: &'static str, path: &str, err: impl std::fmt::Display) -> AppError {
    AppError::sftp(code, json!({ "path": path, "err": err.to_string() }))
}

fn check_cancel(cancel: &AtomicBool) -> AppResult<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(changed(CANCELLED_CODE));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceStamp {
    size: Option<u64>,
    identity: String,
}

impl SourceStamp {
    pub fn new(size: Option<u64>, identity: String) -> Self {
        Self { size, identity }
    }

    fn unknown() -> Self {
        Self::new(None, String::new())
    }

    pub fn from_metadata(meta: &Metadata) -> Self {
        if !meta.is_file() {
            return Self::unknown();
        }
        let size = Some(meta.len());
        Self::new(
            size,
            format!("{}:{:?}", file_id(meta), meta.modified().ok()),
        )
    }

    fn remote(meta: &FileAttributes) -> Self {
        Self::new(
            meta.size,
            format!(
                "{:?}:{:?}:{:?}:{:?}",
                meta.mtime, meta.permissions, meta.uid, meta.gid
            ),
        )
    }
}

fn file_id(meta: &Metadata) -> String {
    #[cfg(any(macos, linux, android, ios, ohos))]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", meta.dev(), meta.ino())
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.creation_time().to_string()
    }
}

#[derive(Default)]
pub struct TransferCheckpoint {
    source: Option<SourceStamp>,
    destination: Option<Option<SourceStamp>>,
    part: Option<String>,
    part_location: Option<String>,
    destination_hash: Option<[u8; 32]>,
    local_file_id: Option<String>,
    submitted: u64,
    acknowledged: u64,
    hash: Sha256,
    // Keep the recovery location if staging setup or a rename needs manual
    // recovery. An ordinary retry must not create or move another file.
    terminal_error: Option<AppError>,
}

impl TransferCheckpoint {
    fn part_path(&self) -> Option<&str> {
        self.part_location.as_deref()
    }

    pub fn recovery_path(&self) -> Option<&str> {
        if let Some(AppError::Sftp(cause)) = &self.terminal_error {
            if let Some(path) = cause.params["path"].as_str() {
                return Some(path);
            }
        }
        self.part_path()
    }

    fn commit_started(&mut self, path: &str) {
        // Set this BEFORE sending a request: dropping the enclosing command
        // future can lose the reply even if the server finished the rename.
        self.terminal_error = Some(publish_error("sftp_replace_uncertain", path, "rename"));
    }

    fn check(&mut self, source: &SourceStamp, destination: &Option<SourceStamp>) -> AppResult<()> {
        if let Some(err) = &self.terminal_error {
            return Err(err.clone());
        }
        if self.source.get_or_insert_with(|| source.clone()) != source {
            return Err(changed("sftp_source_changed"));
        }
        self.check_destination(destination)
    }

    fn check_destination(&mut self, destination: &Option<SourceStamp>) -> AppResult<()> {
        if self.destination.get_or_insert_with(|| destination.clone()) != destination {
            return Err(changed("sftp_target_changed"));
        }
        Ok(())
    }

    fn published(&mut self, result: AppResult<()>) -> AppResult<()> {
        self.terminal_error = None;
        if let Err(err) = &result {
            if matches!(
                err.code(),
                "sftp_replace_backup" | "sftp_replace_uncertain" | "sftp_backup_cleanup"
            ) {
                self.terminal_error = Some(err.clone());
            }
        }
        result
    }
}

#[async_trait::async_trait]
pub trait DownloadDirectory: Send + Sync {
    async fn create_part(&self, name: &str) -> AppResult<File>;
    async fn open_part(&self, name: &str) -> AppResult<File>;
    async fn target_stamp(&self, name: &str) -> AppResult<Option<SourceStamp>>;
    async fn publish(&self, part: &str, name: &str) -> AppResult<()>;
    fn display_path(&self, name: &str) -> String;
}

pub struct PathDirectory {
    path: PathBuf,
    folder_grant: bool,
}

impl PathDirectory {
    pub fn new(parent: PathBuf) -> Self {
        Self {
            path: if parent.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                parent
            },
            folder_grant: false,
        }
    }

    /// A picked file provider directory can permit writes but deny child renames.
    pub fn granted(parent: PathBuf) -> Self {
        Self {
            path: parent,
            folder_grant: true,
        }
    }

    fn access_error(&self, error: std::io::Error) -> AppError {
        if self.folder_grant && error.kind() == ErrorKind::PermissionDenied {
            AppError::sftp("sftp_directory_required", serde_json::json!({}))
        } else {
            error.into()
        }
    }

    fn child(&self, name: &str) -> AppResult<PathBuf> {
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(Component::Normal(component)) if component == name)
            || components.next().is_some()
        {
            return Err(io_error("open", "invalid child name"));
        }
        Ok(self.path.join(name))
    }
}

// Unlike the first, overwrite-capable rename, both fallback moves must fail
// if another writer has already created the requested name.
#[cfg(any(macos, linux, android, ios, ohos))]
fn move_noclobber(source: &Path, destination: &Path) -> std::io::Result<()> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};
    Ok(renameat_with(
        CWD,
        source,
        CWD,
        destination,
        RenameFlags::NOREPLACE,
    )?)
}

#[cfg(windows)]
fn move_noclobber(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    fn wide_name(path: &Path) -> std::io::Result<Vec<u16>> {
        // Canonicalize the parent to preserve long-path support while keeping
        // the final name (which need not exist) literal.
        let parent = path.parent().unwrap_or(Path::new(".")).canonicalize()?;
        let name = path
            .file_name()
            .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "missing file name"))?;
        let mut wide: Vec<_> = parent.join(name).as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "NUL in file name",
            ));
        }
        wide.push(0);
        Ok(wide)
    }
    let source = wide_name(source)?;
    let destination = wide_name(destination)?;
    // Both UTF-16 buffers are NUL terminated and live through the call.
    if unsafe { MoveFileExW(source.as_ptr(), destination.as_ptr(), 0) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[async_trait::async_trait]
impl DownloadDirectory for PathDirectory {
    async fn create_part(&self, name: &str) -> AppResult<File> {
        let path = self.child(name)?;
        if !self.folder_grant {
            fs::create_dir_all(&self.path).await?;
        }
        fs::OpenOptions::from(super::private_staging_options())
            .open(path)
            .await
            .map_err(|err| self.access_error(err))
    }

    async fn open_part(&self, name: &str) -> AppResult<File> {
        let path = self.child(name)?;
        if !fs::symlink_metadata(&path)
            .await
            .map_err(|err| self.access_error(err))?
            .is_file()
        {
            return Err(changed("sftp_partial_changed"));
        }
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .await
            .map_err(|err| self.access_error(err))
    }

    async fn target_stamp(&self, name: &str) -> AppResult<Option<SourceStamp>> {
        match fs::symlink_metadata(self.child(name)?).await {
            Ok(meta) if meta.is_file() => Ok(Some(SourceStamp::from_metadata(&meta))),
            Ok(_) => Err(io_error("stat", "destination is not a regular file")),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => Err(self.access_error(err)),
        }
    }

    async fn publish(&self, part: &str, name: &str) -> AppResult<()> {
        let part = self.child(part)?;
        let target = self.child(name)?;
        match fs::rename(&part, &target).await {
            Ok(()) => return Ok(()),
            Err(err)
                if err.kind() == ErrorKind::AlreadyExists
                    && fs::symlink_metadata(&target)
                        .await
                        .map_err(|err| self.access_error(err))?
                        .is_file() => {}
            Err(err) if self.folder_grant && err.kind() == ErrorKind::PermissionDenied => {
                return Err(AppError::sftp(
                    "sftp_directory_publish_denied",
                    serde_json::json!({ "path": part.to_string_lossy() }),
                ))
            }
            Err(err) => return Err(self.access_error(err)),
        }
        let backup = part.with_extension("bak");
        let original = SourceStamp::from_metadata(
            &fs::symlink_metadata(&target)
                .await
                .map_err(|err| self.access_error(err))?,
        );
        move_noclobber(&target, &backup).map_err(|err| self.access_error(err))?;
        let backup_path = backup.to_string_lossy();
        let saved = fs::symlink_metadata(&backup)
            .await
            .map_err(|e| publish_error("sftp_replace_backup", &backup_path, e))?;
        if SourceStamp::from_metadata(&saved) != original {
            return Err(publish_error(
                "sftp_replace_backup",
                &backup_path,
                "backup changed",
            ));
        }
        move_noclobber(&part, &target)
            .map_err(|e| publish_error("sftp_replace_backup", &backup_path, e))?;
        let saved = fs::symlink_metadata(&backup)
            .await
            .map_err(|e| publish_error("sftp_backup_cleanup", &backup_path, e))?;
        if SourceStamp::from_metadata(&saved) != original {
            return Err(publish_error(
                "sftp_backup_cleanup",
                &backup_path,
                "backup changed",
            ));
        }
        fs::remove_file(&backup)
            .await
            .map_err(|e| publish_error("sftp_backup_cleanup", &backup_path, e))
    }

    fn display_path(&self, name: &str) -> String {
        // Folder downloads may create a missing subtree after taking the
        // destination lock. Resolve aliases above that subtree on every attempt.
        let absolute = std::path::absolute(&self.path).unwrap_or_else(|_| self.path.clone());
        let parent = absolute
            .ancestors()
            .find_map(|ancestor| {
                let resolved = ancestor.canonicalize().ok()?;
                Some(resolved.join(absolute.strip_prefix(ancestor).ok()?))
            })
            .unwrap_or(absolute);
        parent.join(name).to_string_lossy().into_owned()
    }
}

fn private_name(extension: &str) -> String {
    format!(".rssh-{}.{}", Uuid::new_v4(), extension)
}

fn backup_name(part: &str) -> String {
    format!(
        "{}.bak",
        part.strip_suffix(".part").expect("private staging name")
    )
}

fn remote_sibling(path: &str, extension: &str) -> String {
    let name = private_name(extension);
    match path.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/{name}"),
        None => name,
    }
}

const CHUNK: usize = 32 * 1024;

// Read exactly the amount present on the receiving side, including a write
// whose acknowledgement was lost. Both cursors are then at the resume offset.
// Also compare with the original checkpoint so simultaneous edits of BOTH
// copies cannot disguise a changed acknowledged prefix.
async fn verify_prefix<R: AsyncRead + Unpin, W: AsyncRead + Unpin>(
    source: &mut R,
    destination: &mut W,
    length: u64,
    checkpoint: &mut TransferCheckpoint,
    cancel: &AtomicBool,
) -> AppResult<()> {
    if length < checkpoint.acknowledged
        || length > checkpoint.submitted
        || checkpoint
            .source
            .as_ref()
            .and_then(|s| s.size)
            .is_some_and(|s| length > s)
    {
        return Err(changed("sftp_partial_changed"));
    }
    let mut original = Sha256::new();
    let mut received = Sha256::new();
    let mut offset = 0u64;
    let mut buffer = vec![0; CHUNK];
    while offset < length {
        check_cancel(cancel)?;
        let end = if offset < checkpoint.acknowledged {
            checkpoint.acknowledged
        } else {
            length
        };
        let n = (end - offset).min(CHUNK as u64) as usize;
        source.read_exact(&mut buffer[..n]).await?;
        original.update(&buffer[..n]);
        destination.read_exact(&mut buffer[..n]).await?;
        received.update(&buffer[..n]);
        offset += n as u64;
        if offset == checkpoint.acknowledged
            && original.clone().finalize() != checkpoint.hash.clone().finalize()
        {
            return Err(changed("sftp_source_changed"));
        }
    }
    if original.clone().finalize() != received.finalize() {
        return Err(changed("sftp_partial_changed"));
    }
    checkpoint.hash = original;
    checkpoint.acknowledged = length;
    Ok(())
}

async fn copy_remaining<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    source: &mut R,
    destination: &mut W,
    checkpoint: &mut TransferCheckpoint,
    mut progress: impl FnMut(u64, u64),
    cancel: &AtomicBool,
) -> AppResult<()> {
    let size = checkpoint.source.as_ref().and_then(|s| s.size);
    let total = size.unwrap_or(0);
    progress(checkpoint.acknowledged, total);
    let mut buffer = vec![0; CHUNK];
    let mut written = checkpoint.acknowledged;
    let mut hash = checkpoint.hash.clone();
    loop {
        check_cancel(cancel)?;
        let n = source.read(&mut buffer).await?;
        if n > 0 {
            written += n as u64;
            if size.is_some_and(|s| written > s) {
                return Err(changed("sftp_source_changed"));
            }
            checkpoint.submitted = written;
            destination.write_all(&buffer[..n]).await?;
            hash.update(&buffer[..n]);
        }
        // SFTP pipelines writes. Only flushed (acknowledged) batches count as
        // progress. A lost acknowledgement is recovered by hashing on retry.
        if n == 0 || written - checkpoint.acknowledged >= 256 * 1024 {
            destination.flush().await?;
            checkpoint.hash = hash.clone();
            checkpoint.acknowledged = written;
            progress(written, total);
        }
        if n == 0 {
            break;
        }
    }
    if size.is_some_and(|s| s != written) {
        return Err(changed("sftp_source_changed"));
    }
    check_cancel(cancel)
}

fn is_status(error: &SftpError, code: StatusCode) -> bool {
    matches!(error, SftpError::Status(s) if s.status_code == code)
}

fn uncertain(error: &SftpError) -> bool {
    match error {
        SftpError::Status(s) => matches!(
            s.status_code,
            StatusCode::ConnectionLost | StatusCode::NoConnection
        ),
        _ => true,
    }
}

impl SftpHandle {
    pub(crate) async fn canonical_upload_target(&self, path: &str) -> AppResult<String> {
        let (parent, name) = path.rsplit_once('/').unwrap_or((".", path));
        if matches!(name, "" | "." | "..") || name.contains('\0') {
            return Err(io_error("canonicalize", "invalid destination name"));
        }
        let parent = if parent.is_empty() { "/" } else { parent };
        self.mkdir_p(parent).await?;
        let path = self
            .sftp
            .canonicalize(parent)
            .await
            .map_err(|err| io_error("canonicalize", err))?;
        let directory =
            std::str::from_utf8(path.as_ref()).map_err(|err| io_error("canonicalize", err))?;
        Ok(super::join_remote(directory, name))
    }

    async fn remote_target(&self, path: &str) -> AppResult<Option<SourceStamp>> {
        match self.sftp.symlink_metadata(path).await {
            Ok(meta) if meta.file_type().is_file() => Ok(Some(SourceStamp::remote(&meta))),
            Ok(_) => Err(io_error("lstat", "destination is not a regular file")),
            Err(err) if is_status(&err, StatusCode::NoSuchFile) => Ok(None),
            Err(err) => Err(io_error("lstat", err)),
        }
    }

    // V3 has no inode/file-id and its mtime is only accurate to one second.
    // Capture the contents of a pre-existing target so a same-size replacement
    // within that second cannot be mistaken for the old target on retry.
    async fn target_hash(&self, path: &str, cancel: &AtomicBool) -> AppResult<Option<[u8; 32]>> {
        let mut file = match self.sftp.open(path).await {
            Ok(file) => file,
            Err(err) if is_status(&err, StatusCode::PermissionDenied) => return Ok(None),
            Err(err) => return Err(io_error("open existing destination", err)),
        };
        let mut hash = Sha256::new();
        let mut buffer = vec![0; CHUNK];
        loop {
            check_cancel(cancel)?;
            let read = file
                .read(&mut buffer)
                .await
                .map_err(|e| io_error("read existing destination", e))?;
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
        }
        file.close()
            .await
            .map_err(|e| io_error("close existing destination", e))?;
        Ok(Some(hash.finalize().into()))
    }

    async fn create_remote_part(&self, path: &str) -> AppResult<russh_sftp::client::fs::File> {
        self.sftp
            .open_with_flags_and_attributes(
                path,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                FileAttributes {
                    permissions: Some(0o600),
                    ..Default::default()
                },
            )
            .await
            .map_err(|err| io_error("create", err))
    }

    pub(crate) async fn resume_file_upload(
        &self,
        remote_path: &str,
        file: &mut File,
        checkpoint: &mut TransferCheckpoint,
        progress: impl FnMut(u64, u64) + Send,
        cancel: &AtomicBool,
    ) -> AppResult<()> {
        let stamp = SourceStamp::from_metadata(&file.metadata().await?);
        let handle = file.try_clone().await?;
        self.upload_checked(
            remote_path,
            file,
            stamp.clone(),
            checkpoint,
            progress,
            cancel,
            async move {
                if SourceStamp::from_metadata(&handle.metadata().await?) != stamp {
                    return Err(changed("sftp_source_changed"));
                }
                Ok(())
            },
        )
        .await
    }

    pub(super) async fn resume_upload<R: AsyncRead + Unpin + Send>(
        &self,
        remote_path: &str,
        reader: &mut R,
        source: SourceStamp,
        checkpoint: &mut TransferCheckpoint,
        progress: impl FnMut(u64, u64) + Send,
        cancel: &AtomicBool,
    ) -> AppResult<()> {
        self.upload_checked(
            remote_path,
            reader,
            source,
            checkpoint,
            progress,
            cancel,
            async { Ok(()) },
        )
        .await
    }

    async fn upload_checked<R: AsyncRead + Unpin + Send>(
        &self,
        remote_path: &str,
        reader: &mut R,
        source: SourceStamp,
        checkpoint: &mut TransferCheckpoint,
        mut progress: impl FnMut(u64, u64) + Send,
        cancel: &AtomicBool,
        check_source: impl std::future::Future<Output = AppResult<()>> + Send,
    ) -> AppResult<()> {
        if let Some(err) = &checkpoint.terminal_error {
            return Err(err.clone());
        }
        check_cancel(cancel)?;
        let existing = self.remote_target(remote_path).await?;
        checkpoint.check(&source, &existing)?;
        if existing.is_some() {
            let digest = self.target_hash(remote_path, cancel).await?;
            if checkpoint.part.is_some()
                && (digest.is_none() || digest != checkpoint.destination_hash)
            {
                return Err(changed("sftp_target_changed"));
            }
            checkpoint.destination_hash = digest;
        }
        let (mut file, part) = if let Some(part) = &checkpoint.part {
            let stamp = self
                .remote_target(part)
                .await?
                .ok_or_else(|| changed("sftp_partial_changed"))?;
            let mut file = self
                .sftp
                .open_with_flags(part.as_str(), OpenFlags::READ | OpenFlags::WRITE)
                .await
                .map_err(|err| io_error("open", err))?;
            let metadata = file
                .metadata()
                .await
                .map_err(|err| io_error("fstat", err))?;
            if SourceStamp::remote(&metadata) != stamp {
                return Err(changed("sftp_partial_changed"));
            }
            let length = metadata
                .size
                .ok_or_else(|| io_error("fstat", "partial file size unavailable"))?;
            let part = part.clone();
            verify_prefix(reader, &mut file, length, checkpoint, cancel).await?;
            (file, part)
        } else {
            let part = remote_sibling(remote_path, "part");
            let file = self.create_remote_part(&part).await?;
            checkpoint.part_location = Some(part.clone());
            checkpoint.part = Some(part.clone());
            (file, part)
        };
        copy_remaining(reader, &mut file, checkpoint, &mut progress, cancel).await?;
        let written =
            SourceStamp::remote(&file.metadata().await.map_err(|e| io_error("fstat", e))?);
        file.close().await.map_err(|e| io_error("close", e))?;
        check_source.await?;
        checkpoint.check_destination(&self.remote_target(remote_path).await?)?;
        check_cancel(cancel)?;
        let result = self
            .publish_remote(&part, remote_path, &written, checkpoint)
            .await;
        checkpoint.published(result)?;
        progress(checkpoint.acknowledged, checkpoint.acknowledged);
        Ok(())
    }

    pub(crate) async fn resume_download(
        &self,
        remote_path: &str,
        target: &dyn DownloadDirectory,
        name: &str,
        checkpoint: &mut TransferCheckpoint,
        mut progress: impl FnMut(u64, u64) + Send,
        cancel: &AtomicBool,
    ) -> AppResult<()> {
        if let Some(err) = &checkpoint.terminal_error {
            return Err(err.clone());
        }
        check_cancel(cancel)?;
        let mut remote = self
            .sftp
            .open(remote_path)
            .await
            .map_err(|e| io_error("open", e))?;
        let meta = remote.metadata().await.map_err(|e| io_error("fstat", e))?;
        let source = SourceStamp::remote(&meta);
        checkpoint.check(&source, &target.target_stamp(name).await?)?;
        let (mut file, part) = if let Some(part) = &checkpoint.part {
            let part = part.clone();
            let mut file = target.open_part(&part).await?;
            let meta = file.metadata().await?;
            if Some(file_id(&meta)) != checkpoint.local_file_id {
                return Err(changed("sftp_partial_changed"));
            }
            verify_prefix(&mut remote, &mut file, meta.len(), checkpoint, cancel).await?;
            (file, part)
        } else {
            let part = private_name("part");
            let file = target.create_part(&part).await.inspect_err(|err| {
                if err.code() == "sftp_staging_retained" {
                    checkpoint.terminal_error = Some(err.clone());
                }
            })?;
            let location = target.display_path(&part);
            let meta = file.metadata().await.map_err(|err| {
                let err = publish_error("sftp_staging_retained", &location, err);
                checkpoint.terminal_error = Some(err.clone());
                err
            })?;
            checkpoint.local_file_id = Some(file_id(&meta));
            checkpoint.part_location = Some(location);
            checkpoint.part = Some(part.clone());
            (file, part)
        };
        copy_remaining(&mut remote, &mut file, checkpoint, &mut progress, cancel).await?;
        file.shutdown().await?;
        let written = SourceStamp::from_metadata(&file.metadata().await?);
        drop(file.into_std().await);
        let current_source = self
            .sftp
            .metadata(remote_path)
            .await
            .map_err(|e| io_error("stat", e))?;
        if SourceStamp::remote(&current_source) != source {
            return Err(changed("sftp_source_changed"));
        }
        remote
            .shutdown()
            .await
            .map_err(|err| io_error("close", err))?;
        // Providers can have opaque child IDs; check their opened descriptor, not a decoded URI.
        let current_file = target.open_part(&part).await?;
        if SourceStamp::from_metadata(&current_file.metadata().await?) != written {
            return Err(changed("sftp_partial_changed"));
        }
        drop(current_file.into_std().await);
        checkpoint.check_destination(&target.target_stamp(name).await?)?;
        check_cancel(cancel)?;
        let backup = backup_name(&part);
        checkpoint.commit_started(&target.display_path(&backup));
        checkpoint.published(target.publish(&part, name).await)?;
        progress(checkpoint.acknowledged, checkpoint.acknowledged);
        Ok(())
    }

    // V3 has no EEXIST status. Test the SAME staged file and sibling name
    // twice. Only an occupied-name failure followed by a successful vacant-
    // name rename confirms the server's no-overwrite behavior for this file.
    async fn confirm_collision(
        &self,
        part: &str,
        written: &SourceStamp,
        original_error: &SftpError,
        checkpoint: &mut TransferCheckpoint,
    ) -> AppResult<Option<String>> {
        let probe = remote_sibling(part, "part");
        let file = self.create_remote_part(&probe).await?;
        let empty = SourceStamp::remote(&file.metadata().await.map_err(|e| io_error("fstat", e))?);
        file.close().await?;
        let candidates = format!("{part}, {probe}");
        checkpoint.commit_started(&candidates);
        let occupied = self.sftp.rename(part, &probe).await;
        if occupied.is_ok() {
            // This server DOES replace the target. Our staged data now lives
            // at the probe name. Never delete it or touch the original target.
            checkpoint.part_location = Some(probe.clone());
            checkpoint.part = Some(probe);
            checkpoint.terminal_error = None;
            return Ok(None);
        }
        let occupied = occupied.unwrap_err();
        if uncertain(&occupied) {
            return Err(publish_error(
                "sftp_replace_uncertain",
                &candidates,
                occupied,
            ));
        }
        // A negative server reply plus both original entries still present
        // establishes that the probe is the empty file we created.
        if self.remote_target(&probe).await?.as_ref() != Some(&empty)
            || self.remote_target(part).await?.as_ref() != Some(written)
        {
            return Err(publish_error(
                "sftp_replace_uncertain",
                &candidates,
                occupied,
            ));
        }
        checkpoint.terminal_error = None;
        self.sftp
            .remove_file(&probe)
            .await
            .map_err(|e| io_error("remove probe", e))?;
        if !matches!((&occupied,original_error), (SftpError::Status(actual),SftpError::Status(original))
            if actual.status_code == original.status_code && actual.error_message == original.error_message)
        {
            return Ok(None);
        }
        checkpoint.commit_started(&candidates);
        self.sftp.rename(part, &probe).await.map_err(|e| {
            if uncertain(&e) {
                publish_error("sftp_replace_uncertain", &candidates, e)
            } else {
                io_error("rename probe", e)
            }
        })?;
        checkpoint.part = Some(probe.clone());
        checkpoint.part_location = Some(probe.clone());
        checkpoint.terminal_error = None;
        Ok(Some(probe))
    }

    async fn publish_remote(
        &self,
        part: &str,
        target: &str,
        written: &SourceStamp,
        checkpoint: &mut TransferCheckpoint,
    ) -> AppResult<()> {
        if written.size != Some(checkpoint.acknowledged)
            || self.remote_target(part).await?.as_ref() != Some(written)
        {
            return Err(changed("sftp_partial_changed"));
        }
        let possible_locations = format!("{target}, {part}");
        checkpoint.commit_started(&possible_locations);
        match self.sftp.posix_rename(part, target).await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(e) if is_status(&e, StatusCode::OpUnsupported) => {}
            Err(e) if uncertain(&e) => {
                return Err(publish_error(
                    "sftp_replace_uncertain",
                    &possible_locations,
                    e,
                ))
            }
            Err(e) => return Err(io_error("posix rename", e)),
        }
        let err = match self.sftp.rename(part, target).await {
            Ok(()) => return Ok(()),
            Err(err) => err,
        };
        if uncertain(&err) {
            return Err(publish_error(
                "sftp_replace_uncertain",
                &possible_locations,
                err,
            ));
        }
        checkpoint.terminal_error = None;
        if !is_status(&err, StatusCode::Failure)
            || checkpoint.destination.as_ref().is_none_or(Option::is_none)
        {
            return Err(io_error("rename", err));
        }
        let Some(confirmed_part) = self
            .confirm_collision(part, written, &err, checkpoint)
            .await?
        else {
            return Err(io_error("rename", err));
        };
        let original = self.remote_target(target).await?;
        checkpoint.check_destination(&original)?;
        if self.remote_target(&confirmed_part).await?.as_ref() != Some(written) {
            return Err(changed("sftp_partial_changed"));
        }
        let backup = backup_name(part);
        if self.remote_target(&backup).await?.is_some() {
            return Err(io_error("rename", "backup already exists"));
        }
        let possible_locations = format!("{target}, {backup}");
        checkpoint.commit_started(&possible_locations);
        self.sftp.rename(target, &backup).await.map_err(|e| {
            if uncertain(&e) {
                publish_error("sftp_replace_uncertain", &possible_locations, e)
            } else {
                io_error("rename to backup", e)
            }
        })?;
        checkpoint.commit_started(&backup);
        let saved = self
            .remote_target(&backup)
            .await
            .map_err(|e| publish_error("sftp_replace_backup", &backup, e.code()))?;
        if saved != original {
            return Err(publish_error(
                "sftp_replace_backup",
                &backup,
                "backup changed",
            ));
        }
        self.sftp
            .rename(&confirmed_part, target)
            .await
            .map_err(|e| publish_error("sftp_replace_backup", &backup, e))?;
        let saved = self
            .remote_target(&backup)
            .await
            .map_err(|e| publish_error("sftp_backup_cleanup", &backup, e.code()))?;
        if saved != original {
            return Err(publish_error(
                "sftp_backup_cleanup",
                &backup,
                "backup changed",
            ));
        }
        if let Some(expected) = checkpoint.destination_hash {
            let retained = self
                .target_hash(&backup, &AtomicBool::new(false))
                .await
                .map_err(|e| publish_error("sftp_backup_cleanup", &backup, e.code()))?;
            if retained != Some(expected) {
                return Err(publish_error(
                    "sftp_backup_cleanup",
                    &backup,
                    "backup contents changed",
                ));
            }
        }
        self.sftp
            .remove_file(&backup)
            .await
            .map_err(|e| publish_error("sftp_backup_cleanup", &backup, e))
    }
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;
