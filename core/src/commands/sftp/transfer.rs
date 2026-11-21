//! In-memory checkpoints have the same lifetime as the transfer queue. An ID
//! binds the request, authenticated peer and owner before any file is opened.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::emitter::Host;
use crate::error::{locked, AppError, AppResult};
use crate::files::atomic::{local_file_id, normalized_destination, FileId, LocalPartial};
use crate::ssh::client::SshPeer;
use crate::ssh::sftp::{RemoteStamp, SftpHandle, CANCELLED_CODE};
use crate::state::{AppState, SessionKind, SessionOwner, SessionPhase};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

#[derive(PartialEq, Eq)]
pub struct Request {
    pub transfer_id: String,
    pub original_session_id: Option<String>,
    pub local_path: String,
    pub remote_path: String,
    pub direction: Direction,
}

struct Entry {
    request: Request,
    peer: SshPeer,
    cancel: AtomicBool,
    checkpoint: Arc<AsyncMutex<Checkpoint>>,
}

#[derive(Default)]
pub struct Transfers {
    entries: HashMap<(SessionOwner, String), Arc<Entry>>,
    active_targets: HashSet<Target>,
}

impl Transfers {
    pub fn cancel(&self, owner: &SessionOwner, id: &str) {
        if let Some(entry) = self.entries.get(&(owner.clone(), id.to_owned())) {
            entry.cancel.store(true, Ordering::Release);
        }
    }

    pub fn forget(&mut self, owner: &SessionOwner, id: &str) -> AppResult<()> {
        let key = (owner.clone(), id.to_owned());
        if let Some(entry) = self.entries.get(&key) {
            if entry.checkpoint.try_lock().is_err() {
                return Err(busy(&entry.request.remote_path));
            }
        }
        self.entries.remove(&key);
        Ok(())
    }

    pub fn close_owner(&mut self, owner: &SessionOwner) {
        self.entries.retain(|(other, _), entry| {
            if other == owner {
                entry.cancel.store(true, Ordering::Release);
                return false;
            }
            true
        });
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Target {
    Local(PathBuf),
    Remote(SshPeer, String),
}

struct TargetGuard<'a> {
    registry: &'a Mutex<Transfers>,
    target: Target,
}

impl<'a> TargetGuard<'a> {
    fn acquire(registry: &'a Mutex<Transfers>, target: Target, display: &str) -> AppResult<Self> {
        if !locked(registry)?.active_targets.insert(target.clone()) {
            return Err(busy(display));
        }
        Ok(Self { registry, target })
    }
}

impl Drop for TargetGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.active_targets.remove(&self.target);
        }
    }
}

fn busy(path: &str) -> AppError {
    AppError::sftp("sftp_target_busy", json!({"path": path}))
}

fn changed(path: &str) -> AppError {
    AppError::sftp("sftp_transfer_changed", json!({"path": path}))
}

fn remote_error(op: &str, err: impl std::fmt::Display) -> AppError {
    AppError::sftp("sftp_io_failed", json!({"op": op, "err": err.to_string()}))
}

#[derive(Default)]
struct Checkpoint {
    files: Option<Files>,
    prefix: Prefix,
    /// An interrupted publish may have already installed the file or moved the
    /// old target. Replay the outcome instead of issuing another rename.
    result: Option<AppResult<u64>>,
}

enum Files {
    Upload {
        source: LocalSource,
        destination: String,
        original: Option<RemoteStamp>,
        part: String,
    },
    Download {
        destination: PathBuf,
        source: RemoteStamp,
        part: LocalPartial,
    },
}

#[derive(PartialEq, Eq)]
struct LocalSource {
    size: Option<u64>,
    modified: Option<std::time::SystemTime>,
    id: Option<FileId>,
}

impl LocalSource {
    async fn inspect(file: &File) -> AppResult<Self> {
        let descriptor = file.try_clone().await?.into_std().await;
        // Some document providers return a stream without stat information.
        // Its grant and the verified prefix still bind the data on retry.
        let meta = match descriptor.metadata() {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {
                return Ok(Self {
                    size: None,
                    modified: None,
                    id: None,
                });
            }
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            size: (meta.is_file() && meta.len() > 0).then_some(meta.len()),
            modified: meta.modified().ok(),
            id: if meta.is_file() {
                Some(local_file_id(&descriptor)?)
            } else {
                None
            },
        })
    }
}

#[derive(Default)]
struct Prefix {
    bytes: u64,
    /// A connection can drop after the write reached the peer but before its
    /// ACK. Only this last, bounded write can extend the confirmed prefix.
    attempted: u64,
    hash: Sha256,
}

impl Prefix {
    async fn validate<R, W>(
        &mut self,
        src: &mut R,
        dst: &mut W,
        len: u64,
        entry: &Entry,
    ) -> AppResult<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncRead + Unpin,
    {
        if len < self.bytes || len > self.attempted {
            return Err(changed(&entry.request.remote_path));
        }
        // SFTP v3 has neither a stable inode nor a mandatory range hash. A
        // full prefix check also catches in-place edits within one mtime tick.
        let mut source = vec![0; 64 * 1024];
        let mut target = vec![0; source.len()];
        let mut checked = 0;
        let mut hash = Sha256::new();
        for end in [self.bytes, len] {
            while checked < end {
                entry.check_cancel()?;
                let n = (end - checked).min(source.len() as u64) as usize;
                src.read_exact(&mut source[..n]).await?;
                dst.read_exact(&mut target[..n]).await?;
                if source[..n] != target[..n] {
                    return Err(changed(&entry.request.remote_path));
                }
                hash.update(&source[..n]);
                checked += n as u64;
            }
            if end == self.bytes && hash.clone().finalize() != self.hash.clone().finalize() {
                return Err(changed(&entry.request.remote_path));
            }
        }
        self.hash = hash;
        self.bytes = len;
        Ok(())
    }

    async fn copy<R, W>(
        &mut self,
        src: &mut R,
        dst: &mut W,
        entry: &Entry,
        host: &Host,
        size: Option<u64>,
    ) -> AppResult<u64>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let event = format!("sftp:progress:{}", entry.request.transfer_id);
        let emit = |n| {
            let _ = host.emit(
                &event,
                json!({"phase": "transferring", "transferred": n, "total": size.unwrap_or(0)}),
            );
        };
        emit(self.bytes);
        let mut buf = vec![0; 64 * 1024];
        loop {
            entry.check_cancel()?;
            let n = src.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            let end = self.bytes + n as u64;
            if size.is_some_and(|s| end > s) {
                return Err(changed(&entry.request.remote_path));
            }
            self.attempted = end;
            dst.write_all(&buf[..n]).await?;
            self.bytes = end;
            self.hash.update(&buf[..n]);
            emit(end);
        }
        if size.is_some_and(|s| self.bytes != s) {
            return Err(changed(&entry.request.remote_path));
        }
        dst.flush().await?;
        entry.check_cancel()?;
        Ok(self.bytes)
    }
}

impl Entry {
    fn verifying(&self, host: &Host, bytes: u64, size: Option<u64>) {
        let _ = host.emit(
            &format!("sftp:progress:{}", self.request.transfer_id),
            json!({
                "phase": "verifying", "transferred": bytes, "total": size.unwrap_or(0),
            }),
        );
    }

    fn publishing(&self, host: &Host, bytes: u64, part: &str, backup: &str) -> AppResult<()> {
        self.check_cancel()?;
        host.emit(
            &format!("sftp:progress:{}", self.request.transfer_id),
            json!({
                "phase": "publishing", "transferred": bytes, "total": bytes,
                "partialPath": part, "backupPath": backup,
            }),
        )
        .map_err(|err| remote_error("notify", err))
    }

    fn check_cancel(&self) -> AppResult<()> {
        if self.cancel.load(Ordering::Acquire) {
            return Err(AppError::sftp(CANCELLED_CODE, json!({})));
        }
        Ok(())
    }
}

pub struct Attempt {
    sftp: Arc<SftpHandle>,
    entry: Arc<Entry>,
    checkpoint: OwnedMutexGuard<Checkpoint>,
}

impl Attempt {
    pub fn begin(
        state: &AppState,
        owner: &SessionOwner,
        sftp_id: &str,
        request: Request,
    ) -> AppResult<Self> {
        let sessions = locked(&state.lifecycle_sessions)?;
        let record = sessions
            .get(sftp_id)
            .filter(|r| {
                r.kind == SessionKind::Sftp && r.phase() == SessionPhase::Ready && &r.owner == owner
            })
            .ok_or_else(|| AppError::not_found("sftp_session_not_found", json!({})))?;
        let sftp = super::get_sftp(state, sftp_id)?;
        let mut registry = locked(&state.transfers)?;
        let key = (owner.clone(), request.transfer_id.clone());
        let entry = match registry.entries.get(&key) {
            Some(entry) if entry.request == request && entry.peer == sftp.peer => entry.clone(),
            Some(_) => return Err(changed(&request.remote_path)),
            None => {
                if request.original_session_id.is_some()
                    && request.original_session_id != record.parent
                {
                    return Err(AppError::sftp(
                        "sftp_transfer_reconnect_required",
                        json!({}),
                    ));
                }
                let entry = Arc::new(Entry {
                    request,
                    peer: sftp.peer.clone(),
                    cancel: AtomicBool::new(false),
                    checkpoint: Arc::default(),
                });
                registry.entries.insert(key, entry.clone());
                entry
            }
        };
        let checkpoint = entry
            .checkpoint
            .clone()
            .try_lock_owned()
            .map_err(|_| busy(&entry.request.remote_path))?;
        entry.cancel.store(false, Ordering::Release);
        Ok(Self {
            sftp,
            entry,
            checkpoint,
        })
    }

    pub async fn upload(mut self, host: &Host, source: &mut File) -> AppResult<u64> {
        if let Some(result) = &self.checkpoint.result {
            return result.clone();
        }
        self.entry.check_cancel()?;
        let path = &self.entry.request.remote_path;
        if let Some((parent, _)) = path.rsplit_once('/') {
            self.sftp.mkdir_p(parent).await?;
        }
        let destination = self.sftp.canonical_remote_target(path).await?;
        let _guard = TargetGuard::acquire(
            &host.state().transfers,
            Target::Remote(self.entry.peer.clone(), destination.clone()),
            path,
        )?;
        let stamp = LocalSource::inspect(source).await?;
        let expected = self.sftp.remote_destination_stamp(&destination).await?;
        let cp = &mut *self.checkpoint;
        let (mut remote, resume_at) = match &cp.files {
            Some(Files::Upload {
                source: saved,
                destination: target,
                original,
                part,
            }) if saved == &stamp && target == &destination && original == &expected => {
                let (file, length) = self
                    .sftp
                    .reopen_remote_part(part, cp.prefix.bytes, cp.prefix.attempted)
                    .await?;
                (file, Some(length))
            }
            Some(_) => return Err(changed(path)),
            None => {
                let (part, remote) = self.sftp.create_remote_part(&destination).await?;
                cp.files = Some(Files::Upload {
                    source: stamp,
                    destination: destination.clone(),
                    original: expected,
                    part,
                });
                (remote, None)
            }
        };
        let Some(Files::Upload {
            source: stamp,
            original,
            part,
            ..
        }) = &cp.files
        else {
            unreachable!()
        };
        let _ = host.emit(&format!("sftp:progress:{}", self.entry.request.transfer_id), json!({
            "phase": if resume_at.is_some() { "verifying" } else { "transferring" },
            "transferred": cp.prefix.bytes, "total": stamp.size.unwrap_or(0), "partialPath": part,
        }));
        if let Some(len) = resume_at {
            cp.prefix
                .validate(source, &mut remote, len, &self.entry)
                .await?;
        }
        let n = cp
            .prefix
            .copy(source, &mut remote, &self.entry, host, stamp.size)
            .await?;
        if &LocalSource::inspect(source).await? != stamp {
            return Err(changed(&self.entry.request.local_path));
        }
        let uploaded = self.sftp.close_remote_part(remote, part).await?;
        let backup = crate::ssh::sftp::backup_path(part)?;
        self.entry.publishing(host, n, part, &backup)?;
        cp.result = Some(Err(AppError::sftp(
            "sftp_commit_uncertain",
            json!({"path": destination, "backup": backup}),
        )));
        let result = self
            .sftp
            .commit_remote_part(part, &destination, original, &uploaded)
            .await
            .map(|_| n);
        cp.result = match &result {
            Ok(_) => Some(result.clone()),
            Err(error)
                if matches!(
                    error.code(),
                    "sftp_publish_failed"
                        | "sftp_backup_failed"
                        | "sftp_backup_changed"
                        | "sftp_replace_failed"
                        | "sftp_backup_cleanup_failed"
                ) =>
            {
                Some(result.clone())
            }
            // Preflight failed before any rename, so a retry can revalidate.
            Err(_) => None,
        };
        result
    }

    pub async fn download(mut self, host: &Host, destination: &Path) -> AppResult<u64> {
        if let Some(result) = &self.checkpoint.result {
            return result.clone();
        }
        self.entry.check_cancel()?;
        let path = &self.entry.request.remote_path;
        let destination = normalized_destination(destination)?;
        let _guard = TargetGuard::acquire(
            &host.state().transfers,
            Target::Local(destination.clone()),
            &destination.to_string_lossy(),
        )?;
        let mut source = self.sftp.open_remote_source(path).await?;
        let stamp = self.sftp.stamp_open_file(&mut source, path).await?;
        let size = stamp.size();
        let cp = &mut *self.checkpoint;
        let mut local = match &cp.files {
            Some(Files::Download {
                source: saved,
                destination: saved_destination,
                part,
            }) if saved == &stamp && saved_destination == &destination => {
                self.entry.verifying(host, cp.prefix.bytes, size);
                let mut file = part.reopen()?;
                let len = file.metadata().await?.len();
                cp.prefix
                    .validate(&mut source, &mut file, len, &self.entry)
                    .await?;
                file
            }
            Some(_) => return Err(changed(path)),
            None => {
                let (part, file) = LocalPartial::create(&destination)?;
                cp.files = Some(Files::Download {
                    destination,
                    source: stamp,
                    part,
                });
                file
            }
        };
        let n = cp
            .prefix
            .copy(&mut source, &mut local, &self.entry, host, size)
            .await?;
        let Some(Files::Download {
            source: stamp,
            part,
            ..
        }) = &mut cp.files
        else {
            unreachable!()
        };
        if &self.sftp.stamp_open_file(&mut source, path).await? != stamp {
            return Err(changed(path));
        }
        source
            .shutdown()
            .await
            .map_err(|e| remote_error("close", e))?;
        local.shutdown().await?;
        drop(local.into_std().await);
        self.entry.publishing(
            host,
            n,
            &part.path().to_string_lossy(),
            &part.backup_path().to_string_lossy(),
        )?;
        let result = part.commit().map(|_| n);
        if result.is_ok()
            || result.as_ref().is_err_and(|e| {
                matches!(
                    e.code(),
                    "sftp_replace_failed" | "sftp_backup_cleanup_failed"
                )
            })
        {
            cp.result = Some(result.clone());
        }
        result
    }
}

#[cfg(test)]
mod openssh_tests;
