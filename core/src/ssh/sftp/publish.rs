use std::collections::HashMap;
use std::fmt::Display;
use std::io::{self, SeekFrom};
use std::sync::{Arc, LazyLock, Mutex, Weak};

use russh_sftp::client::{error::Error, fs::File, RawSftpSession};
use russh_sftp::protocol::{FileAttributes, OpenFlags, Packet, StatusCode};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use super::{join_remote, SftpHandle};
use crate::error::{locked, AppError, AppResult};
use crate::ssh::client::SshPeer;

// SFTP v3 has neither inode numbers nor a conditional rename. Check attributes
// and bounded content samples, and serialize RSSH publishers of the same name.
type Publishers = HashMap<(SshPeer, String), Weak<AsyncMutex<()>>>;
static PUBLISHERS: LazyLock<Mutex<Publishers>> = LazyLock::new(Mutex::default);
const POSIX_RENAME: &str = "posix-rename@openssh.com";

#[derive(Debug, Clone, Eq, PartialEq)]
struct Attributes {
    size: Option<u64>,
    mode: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
    mtime: Option<u32>,
}

impl Attributes {
    fn regular(meta: &FileAttributes) -> Option<Self> {
        if !meta.file_type().is_file() {
            return None;
        }
        Some(Self::from(meta))
    }
}

impl From<&FileAttributes> for Attributes {
    fn from(meta: &FileAttributes) -> Self {
        Self {
            size: meta.size,
            mode: meta.permissions,
            uid: meta.uid,
            gid: meta.gid,
            mtime: meta.mtime,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RemoteStamp {
    attrs: Attributes,
    digest: [u8; 32],
}

impl RemoteStamp {
    pub fn size(&self) -> Option<u64> {
        self.attrs.size
    }
}

impl SftpHandle {
    async fn replacing_session(&self) -> Result<Option<RawSftpSession>, Error> {
        let Some(transport) = &self.transport else {
            return Ok(None);
        };
        let timeout = std::time::Duration::from_secs(10);
        let mut channel = match tokio::time::timeout(timeout, async {
            transport.lock().await.channel_open_session().await
        })
        .await?
        {
            Ok(channel) => channel,
            Err(russh::Error::ChannelOpenFailure(
                russh::ChannelOpenFailure::AdministrativelyProhibited
                | russh::ChannelOpenFailure::ConnectFailed
                | russh::ChannelOpenFailure::ResourceShortage,
            )) => return Ok(None),
            Err(error) => return Err(Error::IO(error.to_string())),
        };
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| Error::IO(e.to_string()))?;
        let reply = tokio::time::timeout(timeout, channel.wait()).await;
        if !matches!(reply, Ok(Some(russh::ChannelMsg::Success))) {
            // ChannelStream sends SSH_MSG_CHANNEL_CLOSE on drop even when
            // the server denied the subsystem or didn't acknowledge it.
            drop(channel.into_stream());
            return match reply? {
                Some(russh::ChannelMsg::Failure) => Ok(None),
                _ => Err(Error::UnexpectedPacket),
            };
        }
        let raw = RawSftpSession::new(channel.into_stream());
        let version = raw.init().await?;
        Ok(version
            .extensions
            .get(POSIX_RENAME)
            .is_some_and(|v| v == "1")
            .then_some(raw))
    }

    async fn open_attributes(&self, file: &File, path: &str) -> AppResult<Attributes> {
        let meta = match file.metadata().await {
            Ok(attrs) => attrs,
            Err(Error::Status(status)) if status.status_code == StatusCode::OpUnsupported => self
                .sftp
                .metadata(path)
                .await
                .map_err(|e| io_error("stat", path, e))?,
            Err(error) => return Err(io_error("fstat", path, error)),
        };
        Ok(Attributes::from(&meta))
    }

    pub(crate) async fn stamp_open_file(
        &self,
        file: &mut File,
        path: &str,
    ) -> AppResult<RemoteStamp> {
        let attrs = self.open_attributes(file, path).await?;
        let position = file
            .stream_position()
            .await
            .map_err(|e| io_error("seek", path, e))?;
        let digest = sample_contents(file, attrs.size)
            .await
            .map_err(|e| io_error("read", path, e))?;
        file.seek(SeekFrom::Start(position))
            .await
            .map_err(|e| io_error("seek", path, e))?;
        if self.open_attributes(file, path).await? != attrs {
            return Err(target_changed(path));
        }
        Ok(RemoteStamp { attrs, digest })
    }
}

pub(super) fn io_error(op: &str, path: &str, error: impl Display) -> AppError {
    AppError::sftp(
        "sftp_io_failed",
        json!({ "op": format!("{op} {path}"), "err": error.to_string() }),
    )
}

fn target_changed(path: &str) -> AppError {
    AppError::sftp("sftp_target_changed", json!({ "path": path }))
}

fn split_target(path: &str) -> AppResult<(&str, &str)> {
    let (parent, name) = match path.rsplit_once('/') {
        Some(("", name)) => ("/", name),
        Some(parts) => parts,
        None => (".", path),
    };
    if matches!(name, "" | "." | "..") || path.contains('\0') {
        return Err(AppError::sftp("sftp_invalid_filename", json!({})));
    }
    Ok((parent, name))
}

pub(crate) fn backup_path(part: &str) -> AppResult<String> {
    let stem = part
        .strip_suffix(".part")
        .ok_or_else(|| target_changed(part))?;
    let (directory, name) = stem.split_at(stem.rfind('/').map_or(0, |slash| slash + 1));
    let name = if name.starts_with(".rssh-") {
        &name[1..]
    } else {
        name
    };
    Ok(format!("{directory}{name}.bak"))
}

fn sibling(path: &str, extension: &str) -> AppResult<String> {
    let (parent, _) = split_target(path)?;
    Ok(join_remote(
        parent,
        &format!(".rssh-{}.{extension}", Uuid::new_v4()),
    ))
}

// A v3 server such as OpenSSH returns SSH_FX_FAILURE for EEXIST. Only a
// server reply can trigger LSTAT conflict checks. A timeout/transport error
// may mean the rename succeeded and its response was lost.
fn may_be_collision(error: &Error) -> bool {
    let Error::Status(status) = error else {
        return false;
    };
    if status.status_code != StatusCode::Failure {
        return false;
    }
    let message = status.error_message.trim();
    [
        "",
        "failure",
        "file exists",
        "file already exists",
        "already exists",
    ]
    .iter()
    .any(|detail| message.eq_ignore_ascii_case(detail))
}

// Small files are hashed in full. For large files the wire cost stays bounded
// even if a user replaces a multi-gigabyte file over a slow link.
const WINDOW: u64 = 32 * 1024;
fn sample_ranges(size: Option<u64>) -> Vec<(u64, u64)> {
    let length = size.unwrap_or(WINDOW);
    if length <= 3 * WINDOW {
        vec![(0, length)]
    } else {
        vec![
            (0, WINDOW),
            ((length - WINDOW) / 2, WINDOW),
            (length - WINDOW, WINDOW),
        ]
    }
}

async fn sample_contents(file: &mut File, size: Option<u64>) -> io::Result<[u8; 32]> {
    let mut digest = Sha256::new();
    for (offset, len) in sample_ranges(size) {
        file.seek(SeekFrom::Start(offset)).await?;
        let mut bytes = Vec::with_capacity(len as usize);
        (&mut *file).take(len).read_to_end(&mut bytes).await?;
        if size.is_some() && bytes.len() as u64 != len {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        digest.update(bytes);
    }
    Ok(digest.finalize().into())
}

// SFTP subsystems on one SSH connection can expose different namespaces.
// Verify the transfer's original stamps through the channel that will rename.
async fn raw_matches(
    raw: &RawSftpSession,
    path: &str,
    expected: &RemoteStamp,
) -> Result<bool, Error> {
    let matches_attrs =
        |attrs: &FileAttributes| Attributes::regular(attrs).as_ref() == Some(&expected.attrs);
    match raw.lstat(path).await {
        Ok(meta) if matches_attrs(&meta.attrs) => (),
        Ok(_) => return Ok(false),
        Err(Error::Status(s)) if s.status_code == StatusCode::NoSuchFile => return Ok(false),
        Err(error) => return Err(error),
    }
    let handle = raw
        .open(path, OpenFlags::READ, FileAttributes::default())
        .await?
        .handle;
    let checked = async {
        match raw.fstat(&handle).await {
            Ok(meta) if !matches_attrs(&meta.attrs) => return Ok(false),
            Ok(_) => (),
            Err(Error::Status(s)) if s.status_code == StatusCode::OpUnsupported => (),
            Err(error) => return Err(error),
        }
        let mut digest = Sha256::new();
        for (mut offset, mut remaining) in sample_ranges(expected.size()) {
            while remaining > 0 {
                let request = remaining.min(WINDOW) as u32;
                let bytes = match raw.read(&handle, offset, request).await {
                    Ok(data) => data.data,
                    Err(Error::Status(s))
                        if s.status_code == StatusCode::Eof && expected.size().is_none() =>
                    {
                        break
                    }
                    Err(error) => return Err(error),
                };
                if bytes.is_empty() || bytes.len() > request as usize {
                    return Err(Error::UnexpectedPacket);
                }
                remaining -= bytes.len() as u64;
                offset += bytes.len() as u64;
                digest.update(bytes);
            }
        }
        let digest: [u8; 32] = digest.finalize().into();
        Ok(digest == expected.digest && matches_attrs(&raw.lstat(path).await?.attrs))
    }
    .await;
    raw.close(handle).await?;
    checked
}

fn posix_rename_applied(reply: Packet) -> Result<bool, Error> {
    match reply {
        Packet::Status(s) if s.status_code == StatusCode::Ok => Ok(true),
        Packet::Status(s) if s.status_code == StatusCode::OpUnsupported => Ok(false),
        Packet::Status(s) => Err(Error::Status(s)),
        _ => Err(Error::UnexpectedPacket),
    }
}

impl SftpHandle {
    pub async fn canonical_remote_target(&self, path: &str) -> AppResult<String> {
        let (directory, name) = split_target(path)?;
        // The target itself may not exist. Resolve the parent to unify aliases
        // without following a symlink that occupies the final name.
        let parent = self
            .sftp
            .canonicalize(directory)
            .await
            .map_err(|error| io_error("realpath", directory, error))?;
        Ok(join_remote(&parent, name))
    }

    pub async fn open_remote_source(&self, path: &str) -> AppResult<File> {
        self.sftp
            .open(path)
            .await
            .map_err(|error| io_error("open", path, error))
    }

    pub async fn create_remote_part(&self, target: &str) -> AppResult<(String, File)> {
        let part = sibling(target, "part")?;
        let flags = OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::READ | OpenFlags::WRITE;
        let file = self
            .sftp
            .open_with_flags(&part, flags)
            .await
            .map_err(|error| io_error("create", &part, error))?;
        Ok((part, file))
    }

    pub(crate) async fn reopen_remote_part(
        &self,
        part: &str,
        confirmed: u64,
        attempted: u64,
    ) -> AppResult<(File, u64)> {
        let changed = || AppError::sftp("sftp_part_changed", json!({ "path": part }));
        let before = self
            .sftp
            .symlink_metadata(part)
            .await
            .map_err(|error| io_error("lstat", part, error))?;
        let attrs = Attributes::regular(&before).ok_or_else(changed)?;
        let mut file = self
            .sftp
            .open_with_flags(part, OpenFlags::READ | OpenFlags::WRITE)
            .await
            .map_err(|error| io_error("open", part, error))?;
        let opened = self.open_attributes(&file, part).await?;
        if opened != attrs {
            return Err(changed());
        }
        let length = match opened.size {
            Some(size) => size,
            None => {
                // v3 size is optional. At most the last in-flight write can
                // extend the confirmed prefix. One more byte detects oversize.
                file.seek(SeekFrom::Start(confirmed)).await?;
                let mut tail = (&mut file).take(attempted - confirmed + 1);
                let extra = tokio::io::copy(&mut tail, &mut tokio::io::sink()).await?;
                file.seek(SeekFrom::Start(0)).await?;
                confirmed + extra
            }
        };
        Ok((file, length))
    }

    pub async fn remote_destination_stamp(&self, path: &str) -> AppResult<Option<RemoteStamp>> {
        let meta = match self.sftp.symlink_metadata(path).await {
            Ok(meta) => meta,
            Err(Error::Status(status)) if status.status_code == StatusCode::NoSuchFile => {
                return Ok(None)
            }
            Err(error) => return Err(io_error("lstat", path, error)),
        };
        let attrs = Attributes::regular(&meta).ok_or_else(|| target_changed(path))?;
        let mut file = self.open_remote_source(path).await?;
        let stamp = self.stamp_open_file(&mut file, path).await?;
        file.shutdown()
            .await
            .map_err(|error| io_error("close", path, error))?;
        if stamp.attrs != attrs {
            return Err(target_changed(path));
        }
        Ok(Some(stamp))
    }

    pub(crate) async fn close_remote_part(
        &self,
        mut file: File,
        part: &str,
    ) -> AppResult<RemoteStamp> {
        let stamp = self.stamp_open_file(&mut file, part).await?;
        file.shutdown()
            .await
            .map_err(|e| io_error("close", part, e))?;
        Ok(stamp)
    }

    async fn verify_uploaded_file(&self, path: &str, uploaded: &RemoteStamp) -> AppResult<()> {
        if self.remote_destination_stamp(path).await?.as_ref() != Some(uploaded) {
            return Err(AppError::sftp("sftp_part_changed", json!({"path": path})));
        }
        Ok(())
    }

    async fn vacant_name(&self, path: &str) -> Result<bool, Error> {
        match self.sftp.symlink_metadata(path).await {
            Ok(_) => Ok(false),
            Err(Error::Status(s)) if s.status_code == StatusCode::NoSuchFile => Ok(true),
            Err(error) => Err(error),
        }
    }

    pub async fn commit_remote_part(
        &self,
        part: &str,
        destination: &str,
        expected: &Option<RemoteStamp>,
        uploaded: &RemoteStamp,
    ) -> AppResult<()> {
        let target = self.canonical_remote_target(destination).await?;
        let publish = {
            let mut publishers = locked(&PUBLISHERS)?;
            publishers.retain(|_, lock| lock.strong_count() > 0);
            let slot = publishers
                .entry((self.peer.clone(), target.clone()))
                .or_default();
            match slot.upgrade() {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(AsyncMutex::new(()));
                    *slot = Arc::downgrade(&lock);
                    lock
                }
            }
        };
        // An initially free name must not replace a new arrival. Only request
        // POSIX overwrite semantics for a destination the transfer identified.
        let replacing = if expected.is_some() {
            self.replacing_session()
                .await
                .map_err(|e| io_error("negotiate rename", &target, e))?
        } else {
            None
        };
        let _guard = publish.lock().await;
        let publish_error = |error: Error| {
            AppError::sftp(
                "sftp_publish_failed",
                json!({ "target": target, "part": part, "err": error.to_string() }),
            )
        };
        let mut replaced = false;
        if let (Some(session), Some(original)) = (&replacing, expected) {
            if !raw_matches(session, &target, original)
                .await
                .map_err(|e| io_error("verify", &target, e))?
            {
                return Err(target_changed(&target));
            }
            if !raw_matches(session, part, uploaded)
                .await
                .map_err(|e| io_error("verify", part, e))?
            {
                return Err(AppError::sftp("sftp_part_changed", json!({"path": part})));
            }
            let paths = russh_sftp::ser::to_bytes(&(part, &target))
                .map_err(|e| io_error("encode rename", &target, e))?;
            // Only OpUnsupported proves an advertised extension was not applied.
            replaced = session
                .extended(POSIX_RENAME, paths.to_vec())
                .await
                .and_then(posix_rename_applied)
                .map_err(|e| match e {
                    Error::Status(s) if s.status_code == StatusCode::PermissionDenied => {
                        io_error("rename", &target, s.status_code)
                    }
                    other => publish_error(other),
                })?;
        }
        let result = if replaced {
            Ok(())
        } else {
            if &self.remote_destination_stamp(&target).await? != expected {
                return Err(target_changed(&target));
            }
            self.verify_uploaded_file(part, uploaded).await?;
            self.sftp.rename(part, &target).await
        };
        match result {
            Ok(()) => {
                if self.verify_uploaded_file(&target, uploaded).await.is_err() {
                    return Err(AppError::sftp(
                        "sftp_publish_failed",
                        json!({
                            "target": target, "part": part, "err": "Published file identity could not be verified",
                        }),
                    ));
                }
                return Ok(());
            }
            Err(Error::Status(status)) if status.status_code == StatusCode::PermissionDenied => {
                return Err(io_error("rename", &target, status.status_code));
            }
            Err(error) if expected.is_some() && may_be_collision(&error) => {}
            Err(error) => return Err(publish_error(error)),
        };
        if &self.remote_destination_stamp(&target).await? != expected {
            return Err(target_changed(&target));
        }
        self.verify_uploaded_file(part, uploaded).await?;

        // A fresh sibling keeps previous backups (including name.bak) intact.
        // If the reply to this rename is lost, either path may hold the original.
        // The part is an exclusively-created UUID sibling. Derive the recovery
        // name before moving the old file so a dropped future can report it.
        let backup = backup_path(part)?;
        if !self
            .vacant_name(&backup)
            .await
            .map_err(|e| io_error("lstat", &backup, e))?
        {
            return Err(AppError::sftp(
                "sftp_backup_failed",
                json!({
                    "target": target, "backup": backup, "err": "Backup path is occupied"
                }),
            ));
        }
        self.sftp.rename(&target, &backup).await.map_err(|error| {
            if matches!(&error, Error::Status(s) if s.status_code == StatusCode::PermissionDenied) {
                return io_error("rename", &target, error);
            }
            AppError::sftp(
                "sftp_backup_failed",
                json!({
                    "target": target, "backup": backup, "err": error.to_string()
                }),
            )
        })?;
        if self.remote_destination_stamp(&backup).await.as_ref().ok() != Some(expected) {
            return Err(AppError::sftp(
                "sftp_backup_changed",
                json!({ "target": target, "backup": backup }),
            ));
        }
        let replace_error = |err: String| {
            AppError::sftp(
                "sftp_replace_failed",
                json!({
                    "target": target, "backup": backup, "err": err
                }),
            )
        };
        if self.verify_uploaded_file(part, uploaded).await.is_err() {
            return Err(replace_error(
                "Partial file identity could not be verified".into(),
            ));
        }
        if !self
            .vacant_name(&target)
            .await
            .map_err(|e| replace_error(e.to_string()))?
        {
            return Err(replace_error("Destination name is occupied".into()));
        }
        self.sftp
            .rename(part, &target)
            .await
            .map_err(|error| replace_error(error.to_string()))?;
        if self.verify_uploaded_file(&target, uploaded).await.is_err() {
            return Err(replace_error(
                "Published file identity could not be verified".into(),
            ));
        }
        if self.remote_destination_stamp(&backup).await.as_ref().ok() != Some(expected) {
            return Err(AppError::sftp(
                "sftp_backup_cleanup_failed",
                json!({
                    "target": target, "backup": backup, "err": "Backup identity could not be verified"
                }),
            ));
        }
        self.sftp.remove_file(&backup).await.map_err(|error| {
            AppError::sftp(
                "sftp_backup_cleanup_failed",
                json!({
                    "target": target, "backup": backup, "err": error.to_string()
                }),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh_sftp::protocol::{Attrs, Data, Handle, Name, Status};
    use russh_sftp::{client::SftpSession, server};
    use std::sync::Mutex;

    const PART: &str = "/owned.part";
    const TARGET: &str = "/destination";
    type Files = Arc<Mutex<HashMap<String, Vec<u8>>>>;

    struct MockServer {
        files: Files,
        fault: PublishFault,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum PublishFault {
        None,
        OverwritesBackup,
        DenyFinalRename,
        ReplacePartAfterBackup,
        ReplacePartAtFinalRename,
    }

    fn status(id: u32, code: StatusCode, message: &str) -> Status {
        Status {
            id,
            status_code: code,
            error_message: message.into(),
            language_tag: String::new(),
        }
    }

    impl server::Handler for MockServer {
        type Error = StatusCode;
        fn unimplemented(&self) -> Self::Error {
            StatusCode::OpUnsupported
        }
        async fn realpath(&mut self, id: u32, path: String) -> Result<Name, StatusCode> {
            Ok(Name {
                id,
                files: vec![russh_sftp::protocol::File::dummy(path)],
            })
        }
        async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, StatusCode> {
            let files = self.files.lock().unwrap();
            let data = files.get(&path).ok_or(StatusCode::NoSuchFile)?;
            let mut attrs = FileAttributes::empty();
            attrs.size = Some(data.len() as u64);
            attrs.set_regular(true);
            Ok(Attrs { id, attrs })
        }
        async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, StatusCode> {
            self.lstat(id, handle).await
        }
        async fn open(
            &mut self,
            id: u32,
            path: String,
            _: OpenFlags,
            _: FileAttributes,
        ) -> Result<Handle, StatusCode> {
            self.lstat(id, path.clone()).await?;
            Ok(Handle { id, handle: path })
        }
        async fn read(
            &mut self,
            id: u32,
            handle: String,
            offset: u64,
            len: u32,
        ) -> Result<Data, StatusCode> {
            let files = self.files.lock().unwrap();
            let bytes = &files[&handle];
            let start = offset as usize;
            if start >= bytes.len() {
                return Err(StatusCode::Eof);
            }
            Ok(Data {
                id,
                data: bytes[start..(start + len as usize).min(bytes.len())].to_vec(),
            })
        }
        async fn close(&mut self, id: u32, _: String) -> Result<Status, StatusCode> {
            Ok(status(id, StatusCode::Ok, ""))
        }
        async fn rename(
            &mut self,
            id: u32,
            old: String,
            new: String,
        ) -> Result<Status, StatusCode> {
            let mut files = self.files.lock().unwrap();
            if files.contains_key(&new)
                && !(self.fault == PublishFault::OverwritesBackup && new.ends_with(".bak"))
            {
                return Err(StatusCode::Failure);
            }
            if self.fault == PublishFault::DenyFinalRename && old == PART {
                return Err(StatusCode::PermissionDenied);
            }
            if (self.fault == PublishFault::ReplacePartAfterBackup && old == TARGET)
                || (self.fault == PublishFault::ReplacePartAtFinalRename && old == PART)
            {
                files.insert(PART.to_owned(), b"stranger".to_vec());
            }
            let bytes = files.remove(&old).ok_or(StatusCode::NoSuchFile)?;
            files.insert(new, bytes);
            Ok(status(id, StatusCode::Ok, ""))
        }
        async fn remove(&mut self, id: u32, path: String) -> Result<Status, StatusCode> {
            self.files
                .lock()
                .unwrap()
                .remove(&path)
                .ok_or(StatusCode::NoSuchFile)?;
            Ok(status(id, StatusCode::Ok, ""))
        }
    }

    async fn fixture(fault: PublishFault) -> (SftpHandle, Files) {
        let files = Arc::new(Mutex::new(HashMap::from([
            (PART.into(), b"new data".to_vec()),
            (TARGET.into(), b"old data".to_vec()),
            (format!("{TARGET}.bak"), b"prior backup".to_vec()),
            (format!("{TARGET}.part"), b"prior part".to_vec()),
        ])));
        let (client, server) = tokio::io::duplex(4096);
        server::run(
            server,
            MockServer {
                files: files.clone(),
                fault,
            },
        )
        .await;
        let peer = SshPeer {
            host: "example".into(),
            port: 22,
            username: "user".into(),
            fingerprint: "key".into(),
        };
        let handle = SftpHandle {
            sftp: SftpSession::new(client).await.unwrap(),
            transport: None,
            parent_ssh_id: None,
            peer,
        };
        (handle, files)
    }

    #[test]
    fn only_an_acknowledged_unspecific_failure_or_exists_status_can_be_a_collision() {
        assert!(may_be_collision(&Error::Status(status(
            1,
            StatusCode::Failure,
            "Failure"
        ))));
        assert!(may_be_collision(&Error::Status(status(
            1,
            StatusCode::Failure,
            "File already exists"
        ))));
        for code in [
            StatusCode::PermissionDenied,
            StatusCode::ConnectionLost,
            StatusCode::OpUnsupported,
        ] {
            assert!(!may_be_collision(&Error::Status(status(
                1, code, "Failure"
            ))));
        }
        assert!(!may_be_collision(&Error::Status(status(
            1,
            StatusCode::Failure,
            "Permission denied"
        ))));
        assert!(!may_be_collision(&Error::Timeout));
    }

    #[tokio::test]
    async fn v3_replacement_preserves_unrelated_siblings() {
        let (sftp, files) = fixture(PublishFault::None).await;
        let expected = sftp.remote_destination_stamp(TARGET).await.unwrap();
        let uploaded = sftp.remote_destination_stamp(PART).await.unwrap().unwrap();
        sftp.commit_remote_part(PART, TARGET, &expected, &uploaded)
            .await
            .unwrap();
        let files = files.lock().unwrap();
        assert_eq!(files[TARGET], b"new data");
        assert_eq!(files[&format!("{TARGET}.bak")], b"prior backup");
        assert_eq!(files[&format!("{TARGET}.part")], b"prior part");
        assert_eq!(files.len(), 3);
    }

    #[tokio::test]
    async fn occupied_transfer_backup_is_never_replaced() {
        let (sftp, files) = fixture(PublishFault::OverwritesBackup).await;
        files
            .lock()
            .unwrap()
            .insert("/owned.bak".into(), b"other backup".to_vec());
        let expected = sftp.remote_destination_stamp(TARGET).await.unwrap();
        let uploaded = sftp.remote_destination_stamp(PART).await.unwrap().unwrap();
        let error = sftp
            .commit_remote_part(PART, TARGET, &expected, &uploaded)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "sftp_backup_failed");
        let files = files.lock().unwrap();
        assert_eq!(files[TARGET], b"old data");
        assert_eq!(files["/owned.bak"], b"other backup");
        assert_eq!(files[PART], b"new data");
    }

    #[test]
    fn posix_failure_does_not_authorize_the_v3_backup_protocol() {
        for code in [
            StatusCode::Failure,
            StatusCode::PermissionDenied,
            StatusCode::ConnectionLost,
        ] {
            let response = Packet::Status(status(1, code, "File exists"));
            assert!(posix_rename_applied(response).is_err());
        }
        let unsupported = Packet::Status(status(1, StatusCode::OpUnsupported, ""));
        assert!(!posix_rename_applied(unsupported).unwrap());
    }

    #[test]
    fn backup_is_visible_in_the_same_remote_directory() {
        assert_eq!(backup_path("/owned.part").unwrap(), "/owned.bak");
        assert_eq!(
            backup_path("/.cache/a.b/.rssh-502c19f1-4b16-4a40-b061-3e9c312d4697.part").unwrap(),
            "/.cache/a.b/rssh-502c19f1-4b16-4a40-b061-3e9c312d4697.bak"
        );
    }

    #[tokio::test]
    async fn raw_extension_channel_must_see_the_files_the_primary_channel_verified() {
        let (sftp, files) = fixture(PublishFault::None).await;
        let expected = sftp.remote_destination_stamp(PART).await.unwrap().unwrap();
        let (stream, server_stream) = tokio::io::duplex(4096);
        server::run(
            server_stream,
            MockServer {
                files: files.clone(),
                fault: PublishFault::None,
            },
        )
        .await;
        let raw = RawSftpSession::new(stream);
        raw.init().await.unwrap();
        assert!(raw_matches(&raw, PART, &expected).await.unwrap());
        files
            .lock()
            .unwrap()
            .insert(PART.to_string(), b"stranger".to_vec());
        assert!(!raw_matches(&raw, PART, &expected).await.unwrap());
    }

    #[tokio::test]
    async fn late_denial_reports_the_backup_containing_the_original_bytes() {
        let (sftp, files) = fixture(PublishFault::DenyFinalRename).await;
        let expected = sftp.remote_destination_stamp(TARGET).await.unwrap();
        let uploaded = sftp.remote_destination_stamp(PART).await.unwrap().unwrap();
        let error = sftp
            .commit_remote_part(PART, TARGET, &expected, &uploaded)
            .await
            .unwrap_err();
        let AppError::Sftp(message) = error else {
            panic!("wrong error: {error}")
        };
        assert_eq!(message.code, "sftp_replace_failed");
        let backup = message.params["backup"].as_str().unwrap();
        assert!(backup.ends_with(".bak"));
        let files = files.lock().unwrap();
        assert_eq!(files[backup], b"old data");
        assert_eq!(files[PART], b"new data");
        assert_eq!(files[&format!("{TARGET}.bak")], b"prior backup");
        assert!(!files.contains_key(TARGET));
    }

    #[tokio::test]
    async fn swapped_part_cannot_be_published_to_a_free_or_occupied_target() {
        for existing in [false, true] {
            let (sftp, files) = fixture(PublishFault::None).await;
            if !existing {
                files.lock().unwrap().remove(TARGET);
            }
            let expected = sftp.remote_destination_stamp(TARGET).await.unwrap();
            let uploaded = sftp.remote_destination_stamp(PART).await.unwrap().unwrap();
            files
                .lock()
                .unwrap()
                .insert(PART.into(), b"stranger".to_vec());
            let err = sftp
                .commit_remote_part(PART, TARGET, &expected, &uploaded)
                .await
                .unwrap_err();
            assert_eq!(err.code(), "sftp_part_changed");
            let files = files.lock().unwrap();
            assert_eq!(
                files.get(TARGET).map(Vec::as_slice),
                existing.then_some(&b"old data"[..])
            );
            assert_eq!(files[PART], b"stranger");
            assert!(!files.contains_key("/owned.bak"));
        }
    }

    #[tokio::test]
    async fn part_substitution_during_publication_keeps_the_original_backup() {
        for fault in [
            PublishFault::ReplacePartAfterBackup,
            PublishFault::ReplacePartAtFinalRename,
        ] {
            let (sftp, files) = fixture(fault).await;
            let expected = sftp.remote_destination_stamp(TARGET).await.unwrap();
            let uploaded = sftp.remote_destination_stamp(PART).await.unwrap().unwrap();
            let err = sftp
                .commit_remote_part(PART, TARGET, &expected, &uploaded)
                .await
                .unwrap_err();
            let AppError::Sftp(message) = err else {
                panic!("missing backup path")
            };
            assert_eq!(message.code, "sftp_replace_failed");
            let backup = message.params["backup"].as_str().unwrap();
            let files = files.lock().unwrap();
            assert_eq!(files[backup], b"old data");
            if fault == PublishFault::ReplacePartAfterBackup {
                assert!(!files.contains_key(TARGET));
                assert_eq!(files[PART], b"stranger");
            }
        }
    }

    #[tokio::test]
    async fn destination_changed_with_same_size_and_metadata_is_preserved() {
        let (sftp, files) = fixture(PublishFault::None).await;
        let expected = sftp.remote_destination_stamp(TARGET).await.unwrap();
        let uploaded = sftp.remote_destination_stamp(PART).await.unwrap().unwrap();
        files
            .lock()
            .unwrap()
            .insert(TARGET.into(), b"different"[..8].to_vec());
        let error = sftp
            .commit_remote_part(PART, TARGET, &expected, &uploaded)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "sftp_target_changed");
        let files = files.lock().unwrap();
        assert_eq!(files[TARGET], b"differen");
        assert_eq!(files.len(), 4);
    }
}
