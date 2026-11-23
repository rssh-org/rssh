//! One transfer state machine for SFTP, filesystem and authorized document trees.
//!
//! A checkpoint belongs to one immutable transfer (the command registry owns its
//! host, owner and paths). Files left after an error are deliberately retained.
//! Only exclusive creation makes a temporary file ours; filenames alone never do.

use std::io::SeekFrom;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::emitter::Host;
use crate::error::{AppError, AppResult};
use crate::files::transfer::{FileIdentity, RenameError, TransferEndpoint, TransferMode};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    identity: FileIdentity,
    sha256: [u8; 32],
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum CommitPhase {
    #[default]
    Receiving,
    /// The request may have succeeded even if its reply was lost.
    DirectRename,
    /// The old file may already be at the reserved backup location.
    BackupRename,
    Install,
    Cleanup,
    Complete,
}

#[derive(Debug, Default)]
pub struct TransferState {
    source: Option<Snapshot>,
    original: Option<Option<Snapshot>>,
    source_location: Option<String>,
    target_location: Option<String>,
    part: Option<String>,
    part_id: Option<String>,
    backup: Option<String>,
    /// A generated candidate is not evidence that the old file moved there.
    backup_confirmed: bool,
    destination: Option<String>,
    transferred: u64,
    phase: CommitPhase,
}

impl TransferState {
    pub fn transferred(&self) -> u64 {
        self.transferred
    }
    pub fn part_path(&self) -> Option<&str> {
        self.part.as_deref()
    }
    pub fn backup_path(&self) -> Option<&str> {
        self.backup.as_deref()
    }
    pub fn completed(&self) -> bool {
        self.phase == CommitPhase::Complete
    }
}

fn failure(code: &'static str, path: &str) -> AppError {
    AppError::sftp(code, json!({ "path": path }))
}

fn check_cancel(cancel: &AtomicBool) -> AppResult<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(AppError::sftp(super::sftp::CANCELLED_CODE, json!({})))
    } else {
        Ok(())
    }
}

async fn wait_for_commit<'a>(
    lock: &'a tokio::sync::Mutex<()>,
    cancel: &AtomicBool,
) -> AppResult<tokio::sync::MutexGuard<'a, ()>> {
    check_cancel(cancel)?;
    let waiting = lock.lock();
    tokio::pin!(waiting);
    loop {
        tokio::select! {
            guard = &mut waiting => {
                check_cancel(cancel)?;
                return Ok(guard);
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                // Poll the atomic flag without recreating the lock future:
                // an unrelated transfer may need a long readback or rename,
                // and non-cancelled waiters must retain their queue position.
                check_cancel(cancel)?;
            }
        }
    }
}

fn progress(
    host: &Host,
    id: &str,
    state: &TransferState,
    phase: &str,
    verified: u64,
    verify_total: u64,
) {
    let _ = host.emit(
        &format!("sftp:progress:{id}"),
        json!({
            "transferred": state.transferred,
            "total": state.source.as_ref().map(|s| s.identity.size).unwrap_or(verify_total),
            "phase": phase,
            "verified": verified,
            "verificationTotal": verify_total,
        }),
    );
}

async fn digest(
    endpoint: &dyn TransferEndpoint,
    path: &str,
    length: u64,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64),
) -> AppResult<[u8; 32]> {
    let mut stream = endpoint.open(path, TransferMode::Read).await?;
    let mut hasher = Sha256::new();
    let mut remaining = length;
    let mut bytes = vec![0; 65536];
    while remaining > 0 {
        check_cancel(cancel)?;
        let take = remaining.min(bytes.len() as u64) as usize;
        let n = stream.read(&mut bytes[..take]).await?;
        if n == 0 {
            return Err(failure("sftp_transfer_source_changed", path));
        }
        hasher.update(&bytes[..n]);
        remaining -= n as u64;
        on_progress(length - remaining);
    }
    stream.close().await?;
    Ok(hasher.finalize().into())
}

async fn snapshot(
    endpoint: &dyn TransferEndpoint,
    path: &str,
    cancel: &AtomicBool,
    on_progress: impl FnMut(u64),
) -> AppResult<Option<Snapshot>> {
    let Some(identity) = endpoint.stat(path).await? else {
        return Ok(None);
    };
    let sha256 = digest(endpoint, path, identity.size, cancel, on_progress).await?;
    if endpoint.stat(path).await?.as_ref() != Some(&identity) {
        return Err(failure("sftp_transfer_source_changed", path));
    }
    Ok(Some(Snapshot { identity, sha256 }))
}

fn same_file(expected: &Snapshot, actual: &Snapshot) -> bool {
    expected == actual
}

/// Generic engine shared by both directions. The command layer serializes
/// attempts targeting the same canonical destination, including across SFTP
/// reconnects, and rejects changes to owner, host identity, source or target.
/// Every destination in the same receiver namespace shares `commit_lock` so
/// filesystem aliases cannot interleave final validation, moves or recovery.
pub async fn transfer(
    source: &dyn TransferEndpoint,
    source_path: &str,
    receiver: &dyn TransferEndpoint,
    destination: &str,
    state: &mut TransferState,
    host: &Host,
    transfer_id: &str,
    cancel: Arc<AtomicBool>,
    commit_lock: &tokio::sync::Mutex<()>,
) -> AppResult<u64> {
    let result = run_transfer(
        source,
        source_path,
        receiver,
        destination,
        state,
        host,
        transfer_id,
        cancel,
        commit_lock,
    )
    .await;
    result.map_err(|error| recovery_error(state, error))
}

async fn run_transfer(
    source: &dyn TransferEndpoint,
    source_path: &str,
    receiver: &dyn TransferEndpoint,
    destination: &str,
    state: &mut TransferState,
    host: &Host,
    transfer_id: &str,
    cancel: Arc<AtomicBool>,
    commit_lock: &tokio::sync::Mutex<()>,
) -> AppResult<u64> {
    if state.completed() {
        return Ok(state.transferred);
    }
    check_cancel(&cancel)?;
    let source_location = source.canonicalize_target(source_path).await?;
    let target_location = receiver.canonicalize_target(destination).await?;
    for (expected, actual) in [
        (&state.source_location, &source_location),
        (&state.target_location, &target_location),
    ] {
        if expected.as_ref().is_some_and(|expected| expected != actual) {
            return Err(failure("sftp_transfer_identity_changed", actual));
        }
    }
    state.source_location.get_or_insert(source_location);
    state.target_location.get_or_insert(target_location);
    state
        .destination
        .get_or_insert_with(|| destination.to_owned());

    // A complete digest is recorded before the first byte is sent. Metadata
    // alone cannot detect same-size/same-mtime edits to an untransferred suffix.
    let verify_total = source
        .stat(source_path)
        .await?
        .ok_or_else(|| failure("sftp_transfer_source_changed", source_path))?
        .size;
    progress(host, transfer_id, state, "verifying", 0, verify_total);
    let current_source = snapshot(source, source_path, &cancel, |n| {
        progress(host, transfer_id, state, "verifying", n, verify_total);
    })
    .await?
    .ok_or_else(|| failure("sftp_transfer_source_changed", source_path))?;
    if state
        .source
        .as_ref()
        .is_some_and(|saved| !same_file(saved, &current_source))
    {
        return Err(failure("sftp_transfer_source_changed", source_path));
    }
    state.source.get_or_insert(current_source);
    let total = state.source.as_ref().unwrap().identity.size;

    // Lost rename replies are reconciled by read-back before any further move.
    if state.phase != CommitPhase::Receiving {
        let _commit = wait_for_commit(commit_lock, &cancel).await?;
        if recover_commit(receiver, destination, state, &cancel).await? {
            progress(host, transfer_id, state, "committing", 0, 0);
            return finish_cleanup(receiver, state).await;
        }
    }

    if state.original.is_none() {
        state.original = Some(snapshot(receiver, destination, &cancel, |_| {}).await?);
    }
    if state.phase == CommitPhase::Receiving || state.phase == CommitPhase::DirectRename {
        verify_original(receiver, destination, state, &cancel).await?;
    }

    if state.part.is_none() {
        let part = receiver
            .sibling(destination, &format!(".rssh-{}.part", uuid::Uuid::new_v4()))
            .await?;
        // Only a successful exclusive open gives this transfer ownership. An
        // uncertain create is abandoned; a retry must never adopt that name.
        let file = receiver.open(&part, TransferMode::CreateNew).await?;
        state.part = Some(part.clone());
        // Close before path-based metadata: provider coordination may keep an
        // exclusive writer claim until the actual descriptor is released.
        let close_result = file.close().await;
        state.part_id = receiver.stat(&part).await?.and_then(|m| m.stable_id);
        close_result?;
    }
    let part = state.part.clone().unwrap();
    let partial = receiver
        .stat(&part)
        .await?
        .ok_or_else(|| failure("sftp_transfer_partial_changed", &part))?;
    if partial.stable_id != state.part_id
        || partial.size < state.transferred
        || partial.size > total
    {
        return Err(failure("sftp_transfer_partial_changed", &part));
    }
    let offset = partial.size;
    if offset != 0 {
        // Strict resume reads both prefixes. This also adopts fully written
        // bytes whose write acknowledgement was lost, only after validation.
        let verification_total = offset
            .checked_mul(2)
            .ok_or_else(|| failure("sftp_transfer_invalid_file", &part))?;
        progress(host, transfer_id, state, "verifying", 0, verification_total);
        let source_prefix = digest(source, source_path, offset, &cancel, |n| {
            progress(host, transfer_id, state, "verifying", n, verification_total);
        })
        .await?;
        let partial_prefix = digest(receiver, &part, offset, &cancel, |n| {
            progress(
                host,
                transfer_id,
                state,
                "verifying",
                offset + n,
                verification_total,
            );
        })
        .await?;
        if source_prefix != partial_prefix {
            return Err(failure("sftp_transfer_partial_changed", &part));
        }
    }
    state.transferred = offset;
    progress(host, transfer_id, state, "transferring", 0, 0);
    if offset < total {
        let mut input = source.open(source_path, TransferMode::Read).await?;
        let mut output = receiver.open(&part, TransferMode::ReadWrite).await?;
        input.seek(SeekFrom::Start(offset)).await?;
        output.seek(SeekFrom::Start(offset)).await?;
        let mut bytes = vec![0; 65536];
        while state.transferred < total {
            check_cancel(&cancel)?;
            let take = (total - state.transferred).min(bytes.len() as u64) as usize;
            let n = input.read(&mut bytes[..take]).await?;
            if n == 0 {
                return Err(failure("sftp_transfer_source_changed", source_path));
            }
            output.write_all(&bytes[..n]).await?;
            // Progress is acknowledged bytes, never merely buffered writes.
            output.flush().await?;
            state.transferred += n as u64;
            progress(host, transfer_id, state, "transferring", 0, 0);
        }
        output.close().await?;
        input.close().await?;
    }
    check_cancel(&cancel)?;
    progress(host, transfer_id, state, "verifying", 0, total);
    let staged = snapshot(receiver, &part, &cancel, |n| {
        progress(host, transfer_id, state, "verifying", n, total)
    })
    .await?
    .ok_or_else(|| failure("sftp_transfer_partial_changed", &part))?;
    if staged.identity.size != total || staged.sha256 != state.source.as_ref().unwrap().sha256 {
        return Err(failure("sftp_transfer_partial_changed", &part));
    }
    if source.stat(source_path).await?.as_ref() != Some(&state.source.as_ref().unwrap().identity) {
        return Err(failure("sftp_transfer_source_changed", source_path));
    }
    progress(host, transfer_id, state, "committing", 0, 0);
    // Stream and hash unrelated transfers concurrently; only validation of the
    // old destination plus commit/cleanup must exclude namespace aliases.
    let _commit = wait_for_commit(commit_lock, &cancel).await?;
    commit(receiver, destination, state, &cancel).await?;
    finish_cleanup(receiver, state).await
}

async fn verify_original(
    endpoint: &dyn TransferEndpoint,
    destination: &str,
    state: &TransferState,
    cancel: &AtomicBool,
) -> AppResult<()> {
    let current = snapshot(endpoint, destination, cancel, |_| {}).await?;
    if state.original.as_ref() != Some(&current) {
        return Err(failure("sftp_transfer_target_changed", destination));
    }
    Ok(())
}

fn recovery_error(state: &TransferState, cause: AppError) -> AppError {
    if let Some(backup) = &state.backup {
        AppError::sftp(
            if state.backup_confirmed {
                "sftp_transfer_backup_preserved"
            } else {
                "sftp_transfer_backup_location_unknown"
            },
            json!({
                "backup": backup, "destination": state.destination, "part": state.part, "err": cause.code(),
            }),
        )
    } else {
        cause
    }
}

async fn recover_commit(
    endpoint: &dyn TransferEndpoint,
    destination: &str,
    state: &mut TransferState,
    cancel: &AtomicBool,
) -> AppResult<bool> {
    let part = state.part.as_ref().expect("commit owns a partial");
    let current = snapshot(endpoint, destination, cancel, |_| {})
        .await
        .map_err(|e| recovery_error(state, e))?;
    let partial = endpoint
        .stat(part)
        .await
        .map_err(|e| recovery_error(state, e))?;
    if partial.is_none() {
        // A lost backup-move reply cannot also have installed the staged file.
        // Do not adopt or delete its candidate when an outside actor removed
        // our part before we established ownership of the backup.
        if state.backup.is_some()
            && !state.backup_confirmed
            && state.phase == CommitPhase::BackupRename
        {
            return Err(recovery_error(
                state,
                failure("sftp_transfer_result_unknown", destination),
            ));
        }
        if let Some(installed) = &current {
            let source = state.source.as_ref().unwrap();
            if installed.identity.size == source.identity.size && installed.sha256 == source.sha256
            {
                state.phase = CommitPhase::Cleanup;
                return Ok(true);
            }
        }
        return Err(recovery_error(
            state,
            failure("sftp_transfer_result_unknown", destination),
        ));
    }
    if matches!(
        state.phase,
        CommitPhase::BackupRename | CommitPhase::Install | CommitPhase::Cleanup
    ) {
        let backup = state.backup.as_ref().unwrap();
        let saved = snapshot(endpoint, backup, cancel, |_| {})
            .await
            .map_err(|e| recovery_error(state, e))?;
        match (saved, current) {
            (Some(saved), None)
                if state
                    .original
                    .as_ref()
                    .and_then(|s| s.as_ref())
                    .is_some_and(|old| old == &saved) =>
            {
                state.backup_confirmed = true;
                state.phase = CommitPhase::Install;
            }
            (None, current)
                if state.phase == CommitPhase::BackupRename
                    && state.original.as_ref() == Some(&current) =>
            {
                // The failed backup request did not move the original.
                state.backup = None;
                state.backup_confirmed = false;
                state.phase = CommitPhase::DirectRename;
            }
            _ => {
                return Err(recovery_error(
                    state,
                    failure("sftp_transfer_result_unknown", destination),
                ))
            }
        }
    }
    Ok(false)
}

/// A failed rename response alone says nothing about where the old file is.
/// Read back both names when possible, otherwise retain the uncertain state.
async fn failed_backup_move(
    endpoint: &dyn TransferEndpoint,
    destination: &str,
    state: &mut TransferState,
    cause: AppError,
    conflict: bool,
) -> AppError {
    let backup = state.backup.as_ref().expect("backup candidate").clone();
    let inspect = AtomicBool::new(false);
    if conflict {
        // A proven no-replace conflict means this candidate belongs to someone
        // else. Never adopt it, even if its bytes happen to match the original.
        state.backup = None;
        state.backup_confirmed = false;
        state.phase = CommitPhase::DirectRename;
        return match snapshot(endpoint, destination, &inspect, |_| {}).await {
            Ok(current) if state.original.as_ref() == Some(&current) => AppError::sftp(
                "sftp_transfer_backup_failed",
                json!({ "destination": destination, "part": state.part, "err": cause.code() }),
            ),
            Ok(_) => failure("sftp_transfer_target_changed", destination),
            Err(error) => error,
        };
    }
    let saved = snapshot(endpoint, &backup, &inspect, |_| {}).await;
    let current = snapshot(endpoint, destination, &inspect, |_| {}).await;
    match (saved, current) {
        (Ok(None), Ok(current)) if state.original.as_ref() == Some(&current) => {
            state.backup = None;
            state.backup_confirmed = false;
            state.phase = CommitPhase::DirectRename;
            AppError::sftp(
                "sftp_transfer_backup_failed",
                json!({
                    "destination": destination, "part": state.part, "err": cause.code(),
                }),
            )
        }
        (Ok(Some(saved)), Ok(None))
            if state.original.as_ref().and_then(|old| old.as_ref()) == Some(&saved) =>
        {
            state.backup_confirmed = true;
            state.phase = CommitPhase::Install;
            recovery_error(state, cause)
        }
        _ => recovery_error(state, cause),
    }
}

async fn commit(
    endpoint: &dyn TransferEndpoint,
    destination: &str,
    state: &mut TransferState,
    cancel: &AtomicBool,
) -> AppResult<()> {
    let part = state.part.clone().unwrap();
    if state.phase != CommitPhase::Install {
        verify_original(endpoint, destination, state, cancel).await?;
        state.phase = CommitPhase::DirectRename;
        match endpoint.rename(&part, destination, true).await {
            Ok(()) => {
                state.phase = CommitPhase::Cleanup;
                return Ok(());
            }
            Err(RenameError::Other(error)) => return Err(error),
            Err(RenameError::Conflict(error)) => {
                // A directory, permission failure or ambiguous SFTP status is
                // never sufficient reason to move the user's old file.
                verify_original(endpoint, destination, state, cancel).await?;
                if state.original.as_ref().and_then(|s| s.as_ref()).is_none() {
                    return Err(error);
                }
            }
        }
        let backup = endpoint
            .sibling(destination, &format!(".rssh-{}.bak", uuid::Uuid::new_v4()))
            .await?;
        state.backup = Some(backup.clone());
        state.backup_confirmed = false;
        state.phase = CommitPhase::BackupRename;
        if let Err(error) = endpoint.rename(destination, &backup, false).await {
            let conflict = matches!(&error, RenameError::Conflict(_));
            return Err(failed_backup_move(
                endpoint,
                destination,
                state,
                error.into_error(),
                conflict,
            )
            .await);
        }
        state.backup_confirmed = true;
        state.phase = CommitPhase::Install;
    }
    // A failed/uncertain second rename retains the old contents at backup.
    endpoint
        .rename(&part, destination, false)
        .await
        .map_err(|e| recovery_error(state, e.into_error()))?;
    state.phase = CommitPhase::Cleanup;
    Ok(())
}

async fn finish_cleanup(
    endpoint: &dyn TransferEndpoint,
    state: &mut TransferState,
) -> AppResult<u64> {
    if let Some(backup) = &state.backup {
        // Only our successful backup rename permits removal, and only after
        // the staged file's rename is confirmed. Never remove pre-existing .bak.
        if let Some(actual) = snapshot(endpoint, backup, &AtomicBool::new(false), |_| {}).await? {
            if state.original.as_ref().and_then(|s| s.as_ref()) != Some(&actual) {
                return Err(recovery_error(
                    state,
                    failure("sftp_transfer_target_changed", backup),
                ));
            }
            // Removal can succeed with its acknowledgement lost. Stop asserting
            // that this location still contains the old file once it is sent.
            state.backup_confirmed = false;
            endpoint
                .remove(backup)
                .await
                .map_err(|e| recovery_error(state, e))?;
        }
        state.backup = None;
        state.backup_confirmed = false;
    }
    state.phase = CommitPhase::Complete;
    Ok(state.transferred)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::transfer::{NativeEndpoint, TransferStream};
    use std::sync::Mutex;

    struct MemoryFile {
        id: String,
        bytes: Mutex<Vec<u8>>,
    }

    type ReadObserver = Arc<dyn Fn(u64) + Send + Sync>;

    /// Models two directory names that the storage backend treats as aliases,
    /// without depending on the host filesystem's case/normalization rules.
    #[derive(Default)]
    struct AliasedEndpoint {
        files: Mutex<std::collections::HashMap<String, Arc<MemoryFile>>>,
        first_rename: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        observe_partial_read: Mutex<Option<ReadObserver>>,
        lose_rename_reply: AtomicBool,
    }

    impl AliasedEndpoint {
        fn key(path: &str) -> &str {
            if path == "/Target" {
                "/target"
            } else {
                path
            }
        }

        fn seed(&self, path: &str, bytes: &[u8]) {
            self.files.lock().unwrap().insert(
                Self::key(path).into(),
                Arc::new(MemoryFile {
                    id: uuid::Uuid::new_v4().to_string(),
                    bytes: Mutex::new(bytes.to_vec()),
                }),
            );
        }

        fn contents(&self, path: &str) -> Vec<u8> {
            self.files.lock().unwrap()[Self::key(path)]
                .bytes
                .lock()
                .unwrap()
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl TransferEndpoint for AliasedEndpoint {
        async fn canonicalize_target(&self, path: &str) -> AppResult<String> {
            Ok(path.into())
        }
        async fn sibling(&self, _path: &str, name: &str) -> AppResult<String> {
            Ok(format!("/{name}"))
        }
        async fn stat(&self, path: &str) -> AppResult<Option<FileIdentity>> {
            Ok(self
                .files
                .lock()
                .unwrap()
                .get(Self::key(path))
                .map(|file| FileIdentity {
                    size: file.bytes.lock().unwrap().len() as u64,
                    modified: None,
                    stable_id: Some(file.id.clone()),
                }))
        }
        async fn open(&self, path: &str, mode: TransferMode) -> AppResult<Box<dyn TransferStream>> {
            if matches!(mode, TransferMode::CreateNew) {
                if self.files.lock().unwrap().contains_key(Self::key(path)) {
                    return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists).into());
                }
                self.seed(path, &[]);
            }
            let file = self
                .files
                .lock()
                .unwrap()
                .get(Self::key(path))
                .cloned()
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))?;
            let observer = if matches!(mode, TransferMode::Read) && path.ends_with(".part") {
                self.observe_partial_read.lock().unwrap().take()
            } else {
                None
            };
            Ok(Box::new(MemoryStream {
                file,
                position: 0,
                observer,
            }))
        }
        async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<(), RenameError> {
            let gate = self.first_rename.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.await.unwrap();
            }
            let mut files = self.files.lock().unwrap();
            if !replace && files.contains_key(Self::key(to)) {
                return Err(RenameError::Conflict(
                    std::io::Error::from(std::io::ErrorKind::AlreadyExists).into(),
                ));
            }
            let file = files.remove(Self::key(from)).ok_or_else(|| {
                RenameError::Other(std::io::Error::from(std::io::ErrorKind::NotFound).into())
            })?;
            files.insert(Self::key(to).into(), file);
            if self.lose_rename_reply.swap(false, Ordering::Relaxed) {
                return Err(RenameError::Other(
                    std::io::Error::other("rename acknowledgement lost").into(),
                ));
            }
            Ok(())
        }
        async fn remove(&self, path: &str) -> AppResult<()> {
            self.files.lock().unwrap().remove(Self::key(path));
            Ok(())
        }
    }

    struct MemoryStream {
        file: Arc<MemoryFile>,
        position: u64,
        observer: Option<ReadObserver>,
    }
    impl tokio::io::AsyncRead for MemoryStream {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if let Some(observer) = &self.observer {
                observer(self.position);
            }
            let n = {
                let bytes = self.file.bytes.lock().unwrap();
                let start = (self.position as usize).min(bytes.len());
                let n =
                    (bytes.len() - start)
                        .min(buf.remaining())
                        .min(if self.observer.is_some() {
                            1024
                        } else {
                            usize::MAX
                        });
                buf.put_slice(&bytes[start..start + n]);
                n
            };
            self.position += n as u64;
            std::task::Poll::Ready(Ok(()))
        }
    }
    impl tokio::io::AsyncSeek for MemoryStream {
        fn start_seek(
            mut self: std::pin::Pin<&mut Self>,
            position: SeekFrom,
        ) -> std::io::Result<()> {
            self.position = match position {
                SeekFrom::Start(n) => n,
                SeekFrom::Current(n) => self.position.checked_add_signed(n).unwrap(),
                SeekFrom::End(n) => (self.file.bytes.lock().unwrap().len() as u64)
                    .checked_add_signed(n)
                    .unwrap(),
            };
            Ok(())
        }
        fn poll_complete(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<u64>> {
            std::task::Poll::Ready(Ok(self.position))
        }
    }
    impl tokio::io::AsyncWrite for MemoryStream {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            bytes: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            {
                let mut output = self.file.bytes.lock().unwrap();
                let start = self.position as usize;
                let length = output.len().max(start + bytes.len());
                output.resize(length, 0);
                output[start..start + bytes.len()].copy_from_slice(bytes);
            }
            self.position += bytes.len() as u64;
            std::task::Poll::Ready(Ok(bytes.len()))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }
    #[async_trait::async_trait]
    impl TransferStream for MemoryStream {
        async fn close(self: Box<Self>) -> AppResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn aliased_targets_serialize_commit_without_serializing_streams() {
        for second_destination in ["/target", "/different"] {
            let endpoint = AliasedEndpoint::default();
            endpoint.seed("/first-source", b"first contents");
            endpoint.seed("/second-source", b"second contents");
            let (release, held) = tokio::sync::oneshot::channel();
            *endpoint.first_rename.lock().unwrap() = Some(held);
            let (host, _) = crate::ssh::test_support::headless_host();
            let mut first_state = TransferState::default();
            let mut second_state = TransferState::default();
            let commit_lock = tokio::sync::Mutex::new(());
            let mut first = Box::pin(transfer(
                &endpoint,
                "/first-source",
                &endpoint,
                "/Target",
                &mut first_state,
                &host,
                "first",
                Arc::new(AtomicBool::new(false)),
                &commit_lock,
            ));
            let mut second = Box::pin(transfer(
                &endpoint,
                "/second-source",
                &endpoint,
                second_destination,
                &mut second_state,
                &host,
                "second",
                Arc::new(AtomicBool::new(false)),
                &commit_lock,
            ));
            // All fake I/O is immediately ready except the first rename. Each
            // poll deterministically reaches the commit boundary, with no sleep.
            assert!(futures_util::poll!(first.as_mut()).is_pending());
            assert!(
                futures_util::poll!(second.as_mut()).is_pending(),
                "another commit entered while the first rename was outstanding"
            );
            let staged: Vec<_> = endpoint
                .files
                .lock()
                .unwrap()
                .iter()
                .filter(|(path, _)| path.ends_with(".part"))
                .map(|(_, file)| file.bytes.lock().unwrap().clone())
                .collect();
            assert!(
                staged.contains(&b"second contents".to_vec()),
                "unrelated data streaming must finish while commit is occupied"
            );
            release.send(()).unwrap();
            first.await.unwrap();
            let second_result = second.await;
            if second_destination == "/target" {
                assert_eq!(
                    second_result.unwrap_err().code(),
                    "sftp_transfer_target_changed"
                );
            } else {
                second_result.unwrap();
                assert_eq!(endpoint.contents("/different"), b"second contents");
            }
            assert_eq!(endpoint.contents("/target"), b"first contents");
            assert_eq!(endpoint.contents("/first-source"), b"first contents");
            assert_eq!(endpoint.contents("/second-source"), b"second contents");
        }
    }

    #[tokio::test]
    async fn prefix_verification_stays_incomplete_until_receiver_bytes_are_read() {
        let endpoint = AliasedEndpoint::default();
        endpoint.seed("/source", &vec![b'x'; 180_000]);
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let (base, _) = crate::ssh::test_support::headless_host();
        let first_host = Host::new(
            base.weak_state().upgrade().unwrap(),
            Arc::new(move |_, event| {
                if event["phase"] == "transferring" && event["transferred"].as_u64().unwrap() > 0 {
                    flag.store(true, Ordering::Relaxed);
                }
                true
            }),
        );
        let mut state = TransferState::default();
        assert!(transfer(
            &endpoint,
            "/source",
            &endpoint,
            "/target",
            &mut state,
            &first_host,
            "resume",
            cancel,
            &tokio::sync::Mutex::new(()),
        )
        .await
        .is_err());
        let offset = state.transferred();
        let latest = Arc::new(Mutex::new(serde_json::Value::Null));
        let ticks = latest.clone();
        let host = Host::new(
            base.weak_state().upgrade().unwrap(),
            Arc::new(move |_, event| {
                *ticks.lock().unwrap() = event;
                true
            }),
        );
        let observations = Arc::new(Mutex::new(Vec::new()));
        let observed = observations.clone();
        *endpoint.observe_partial_read.lock().unwrap() = Some(Arc::new(move |position| {
            observed
                .lock()
                .unwrap()
                .push((position, latest.lock().unwrap().clone()));
        }));
        transfer(
            &endpoint,
            "/source",
            &endpoint,
            "/target",
            &mut state,
            &host,
            "resume",
            Arc::new(AtomicBool::new(false)),
            &tokio::sync::Mutex::new(()),
        )
        .await
        .unwrap();
        let observations = observations.lock().unwrap();
        assert!(observations.len() > 1);
        for (position, tick) in observations.iter() {
            assert!(*position < offset);
            assert!(
                tick["verified"].as_u64().unwrap() < tick["verificationTotal"].as_u64().unwrap(),
                "verification reported complete before receiver read at {position}: {tick}"
            );
        }
        assert!(
            observations.last().unwrap().1["verified"].as_u64().unwrap()
                > observations[0].1["verified"].as_u64().unwrap(),
            "receiver readback must advance verification progress"
        );
    }

    #[tokio::test]
    async fn lost_rename_recovery_waits_for_the_same_commit_lock() {
        let endpoint = AliasedEndpoint::default();
        endpoint.seed("/source", b"replacement");
        endpoint.lose_rename_reply.store(true, Ordering::Relaxed);
        let (host, _) = crate::ssh::test_support::headless_host();
        let mut state = TransferState::default();
        let commit_lock = tokio::sync::Mutex::new(());
        assert!(transfer(
            &endpoint,
            "/source",
            &endpoint,
            "/target",
            &mut state,
            &host,
            "recover",
            Arc::new(AtomicBool::new(false)),
            &commit_lock,
        )
        .await
        .is_err());
        assert_eq!(state.phase, CommitPhase::DirectRename);
        assert_eq!(endpoint.contents("/target"), b"replacement");
        let held = commit_lock.lock().await;
        let mut retry = Box::pin(transfer(
            &endpoint,
            "/source",
            &endpoint,
            "/target",
            &mut state,
            &host,
            "recover",
            Arc::new(AtomicBool::new(false)),
            &commit_lock,
        ));
        assert!(futures_util::poll!(retry.as_mut()).is_pending());
        drop(held);
        retry.await.unwrap();
        assert!(state.completed());
    }

    #[tokio::test]
    async fn cancelling_commit_or_recovery_wait_does_not_wait_for_another_transfer() {
        for recovering in [false, true] {
            let endpoint = AliasedEndpoint::default();
            endpoint.seed("/source", b"replacement");
            let (host, _) = crate::ssh::test_support::headless_host();
            let mut state = TransferState::default();
            let commit_lock = tokio::sync::Mutex::new(());
            if recovering {
                endpoint.lose_rename_reply.store(true, Ordering::Relaxed);
                assert!(transfer(
                    &endpoint,
                    "/source",
                    &endpoint,
                    "/target",
                    &mut state,
                    &host,
                    "cancel-recovery",
                    Arc::new(AtomicBool::new(false)),
                    &commit_lock,
                )
                .await
                .is_err());
                assert_eq!(state.phase, CommitPhase::DirectRename);
            }
            let held = commit_lock.lock().await;
            let cancel = Arc::new(AtomicBool::new(false));
            let mut attempt = Box::pin(transfer(
                &endpoint,
                "/source",
                &endpoint,
                "/target",
                &mut state,
                &host,
                "cancel-waiting",
                cancel.clone(),
                &commit_lock,
            ));
            // Fake I/O is ready: the attempt has reached the occupied commit
            // lock. Keep that other transfer's guard for the entire assertion.
            assert!(futures_util::poll!(attempt.as_mut()).is_pending());
            cancel.store(true, Ordering::Relaxed);
            let error = tokio::time::timeout(std::time::Duration::from_secs(1), attempt)
                .await
                .expect("cancel must finish while another transfer still holds the commit lock")
                .unwrap_err();
            assert_eq!(error.code(), super::super::sftp::CANCELLED_CODE);
            assert!(commit_lock.try_lock().is_err());
            assert!(!state.completed());
            assert_eq!(endpoint.contents("/source"), b"replacement");
            if recovering {
                assert_eq!(endpoint.contents("/target"), b"replacement");
            } else {
                assert!(endpoint.stat("/target").await.unwrap().is_none());
                assert_eq!(
                    endpoint.contents(state.part_path().unwrap()),
                    b"replacement"
                );
            }
            drop(held);
        }
    }

    #[derive(Default)]
    struct FaultEndpoint {
        no_replace: bool,
        generic_failure: bool,
        fail_install: AtomicBool,
        fail_backup: AtomicBool,
        occupy_backup_candidate: AtomicBool,
        fail_recovery_readback: AtomicBool,
        lose_direct_reply: AtomicBool,
        lose_backup_reply: AtomicBool,
        fail_cleanup: AtomicBool,
        lose_cleanup_reply: AtomicBool,
        fail_close: Arc<AtomicBool>,
        fail_flush: Arc<AtomicBool>,
        renames: Mutex<Vec<(String, String, bool)>>,
    }

    #[async_trait::async_trait]
    impl TransferEndpoint for FaultEndpoint {
        async fn canonicalize_target(&self, path: &str) -> AppResult<String> {
            NativeEndpoint.canonicalize_target(path).await
        }
        async fn sibling(&self, path: &str, name: &str) -> AppResult<String> {
            let sibling = NativeEndpoint.sibling(path, name).await?;
            if name.ends_with(".bak") && self.occupy_backup_candidate.swap(false, Ordering::Relaxed)
            {
                std::fs::write(&sibling, b"preexisting backup")?;
            }
            Ok(sibling)
        }
        async fn stat(&self, path: &str) -> AppResult<Option<FileIdentity>> {
            if path.ends_with(".bak") && self.fail_recovery_readback.swap(false, Ordering::Relaxed)
            {
                return Err(std::io::Error::other("connection unavailable during readback").into());
            }
            NativeEndpoint.stat(path).await
        }
        async fn open(&self, path: &str, mode: TransferMode) -> AppResult<Box<dyn TransferStream>> {
            let stream = NativeEndpoint.open(path, mode).await?;
            if matches!(mode, TransferMode::ReadWrite) {
                Ok(Box::new(FaultStream {
                    inner: stream,
                    close: self.fail_close.clone(),
                    flush: self.fail_flush.clone(),
                }))
            } else {
                Ok(stream)
            }
        }
        async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<(), RenameError> {
            self.renames
                .lock()
                .unwrap()
                .push((from.to_owned(), to.to_owned(), replace));
            let fail = || {
                RenameError::Other(AppError::sftp(
                    "sftp_io_failed",
                    json!({"op":"rename","err":"simulated failure"}),
                ))
            };
            if self.generic_failure {
                return Err(fail());
            }
            if !replace
                && from.ends_with(".part")
                && self.fail_install.swap(false, Ordering::Relaxed)
            {
                return Err(fail());
            }
            if replace && self.no_replace && NativeEndpoint.stat(to).await.unwrap().is_some() {
                return Err(RenameError::Conflict(AppError::sftp(
                    "sftp_io_failed",
                    json!({"op":"rename","err":"exists"}),
                )));
            }
            if to.ends_with(".bak") && self.fail_backup.swap(false, Ordering::Relaxed) {
                return Err(fail());
            }
            NativeEndpoint.rename(from, to, replace).await?;
            if (replace && self.lose_direct_reply.swap(false, Ordering::Relaxed))
                || (to.ends_with(".bak") && self.lose_backup_reply.swap(false, Ordering::Relaxed))
            {
                return Err(fail());
            }
            Ok(())
        }
        async fn remove(&self, path: &str) -> AppResult<()> {
            if self.fail_cleanup.swap(false, Ordering::Relaxed) {
                return Err(AppError::sftp(
                    "sftp_io_failed",
                    json!({"op":"remove","err":"simulated failure"}),
                ));
            }
            NativeEndpoint.remove(path).await?;
            if self.lose_cleanup_reply.swap(false, Ordering::Relaxed) {
                return Err(std::io::Error::other("cleanup acknowledgement lost").into());
            }
            Ok(())
        }
    }

    struct FaultStream {
        inner: Box<dyn TransferStream>,
        close: Arc<AtomicBool>,
        flush: Arc<AtomicBool>,
    }
    impl tokio::io::AsyncRead for FaultStream {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }
    impl tokio::io::AsyncSeek for FaultStream {
        fn start_seek(
            mut self: std::pin::Pin<&mut Self>,
            position: SeekFrom,
        ) -> std::io::Result<()> {
            std::pin::Pin::new(&mut self.inner).start_seek(position)
        }
        fn poll_complete(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<u64>> {
            std::pin::Pin::new(&mut self.inner).poll_complete(cx)
        }
    }
    impl tokio::io::AsyncWrite for FaultStream {
        fn poll_write(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
        }
        fn poll_flush(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.flush.swap(false, Ordering::Relaxed) {
                return std::task::Poll::Ready(Err(std::io::Error::other(
                    "final buffered write failed",
                )));
            }
            std::pin::Pin::new(&mut self.inner).poll_flush(cx)
        }
        fn poll_shutdown(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }
    #[async_trait::async_trait]
    impl TransferStream for FaultStream {
        async fn close(self: Box<Self>) -> AppResult<()> {
            self.inner.close().await?;
            if self.close.swap(false, Ordering::Relaxed) {
                return Err(std::io::Error::other("close failed").into());
            }
            Ok(())
        }
    }

    fn files(content: &[u8]) -> (tempfile::TempDir, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source").to_string_lossy().into_owned();
        let dest = dir.path().join("target").to_string_lossy().into_owned();
        std::fs::write(&source, content).unwrap();
        (dir, source, dest)
    }

    async fn run(
        source: &str,
        dest: &str,
        endpoint: &dyn TransferEndpoint,
        state: &mut TransferState,
    ) -> AppResult<u64> {
        let (host, _events) = crate::ssh::test_support::headless_host();
        transfer(
            &NativeEndpoint,
            source,
            endpoint,
            dest,
            state,
            &host,
            "test-transfer",
            Arc::new(AtomicBool::new(false)),
            &tokio::sync::Mutex::new(()),
        )
        .await
    }

    #[tokio::test]
    async fn new_and_existing_destinations_keep_sources_and_unrelated_part_and_backup() {
        for existing in [false, true] {
            let (dir, source, dest) = files(b"new file contents");
            if existing {
                std::fs::write(&dest, b"old").unwrap();
            }
            std::fs::write(format!("{dest}.part"), b"user part").unwrap();
            std::fs::write(format!("{dest}.bak"), b"user backup").unwrap();
            let endpoint = FaultEndpoint::default();
            let mut state = TransferState::default();
            assert_eq!(
                run(&source, &dest, &endpoint, &mut state).await.unwrap(),
                17
            );
            assert_eq!(std::fs::read(&dest).unwrap(), b"new file contents");
            assert_eq!(std::fs::read(&source).unwrap(), b"new file contents");
            assert_eq!(std::fs::read(format!("{dest}.part")).unwrap(), b"user part");
            assert_eq!(
                std::fs::read(format!("{dest}.bak")).unwrap(),
                b"user backup"
            );
            assert_eq!(endpoint.renames.lock().unwrap().len(), 1);
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 4);
        }
    }

    #[tokio::test]
    async fn confirmed_conflict_moves_old_file_only_after_direct_rename_and_removes_own_backup() {
        let (_dir, source, dest) = files(b"replacement");
        std::fs::write(&dest, b"original").unwrap();
        let endpoint = FaultEndpoint {
            no_replace: true,
            ..Default::default()
        };
        let mut state = TransferState::default();
        run(&source, &dest, &endpoint, &mut state).await.unwrap();
        let ops = endpoint.renames.lock().unwrap();
        assert_eq!(ops.len(), 3);
        assert!(ops[0].0.ends_with(".part") && ops[0].1 == dest && ops[0].2);
        assert!(ops[1].0 == dest && ops[1].1.ends_with(".bak") && !ops[1].2);
        assert!(ops[2].0.ends_with(".part") && ops[2].1 == dest && !ops[2].2);
        assert!(!std::path::Path::new(&ops[1].1).exists());
        assert_eq!(std::fs::read(dest).unwrap(), b"replacement");
    }

    #[tokio::test]
    async fn final_buffered_write_or_close_error_never_commits_and_retry_checks_retained_bytes() {
        for final_close in [false, true] {
            let (_dir, source, dest) = files(b"buffered until flush");
            std::fs::write(&dest, b"original").unwrap();
            let endpoint = FaultEndpoint {
                fail_close: Arc::new(AtomicBool::new(final_close)),
                fail_flush: Arc::new(AtomicBool::new(!final_close)),
                ..Default::default()
            };
            let mut state = TransferState::default();
            assert!(run(&source, &dest, &endpoint, &mut state).await.is_err());
            assert!(endpoint.renames.lock().unwrap().is_empty());
            assert!(!state.completed());
            assert_eq!(std::fs::read(&dest).unwrap(), b"original");
            if !final_close {
                assert_eq!(state.transferred(), 0);
            }
            run(&source, &dest, &endpoint, &mut state).await.unwrap();
            assert_eq!(std::fs::read(dest).unwrap(), b"buffered until flush");
        }
    }

    #[tokio::test]
    async fn generic_failure_with_existing_target_never_moves_old_content() {
        let (_dir, source, dest) = files(b"replacement");
        std::fs::write(&dest, b"original").unwrap();
        let endpoint = FaultEndpoint {
            generic_failure: true,
            ..Default::default()
        };
        let mut state = TransferState::default();
        assert!(run(&source, &dest, &endpoint, &mut state).await.is_err());
        assert_eq!(endpoint.renames.lock().unwrap().len(), 1);
        assert!(state.backup_path().is_none());
        assert_eq!(std::fs::read(dest).unwrap(), b"original");
        assert_eq!(
            std::fs::read(state.part_path().unwrap()).unwrap(),
            b"replacement"
        );
    }

    #[tokio::test]
    async fn failed_install_retains_backup_reports_location_and_retry_finishes() {
        let (_dir, source, dest) = files(b"replacement");
        std::fs::write(&dest, b"original").unwrap();
        let endpoint = FaultEndpoint {
            no_replace: true,
            fail_install: AtomicBool::new(true),
            ..Default::default()
        };
        let mut state = TransferState::default();
        let error = run(&source, &dest, &endpoint, &mut state)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "sftp_transfer_backup_preserved");
        let backup = state.backup_path().unwrap().to_owned();
        assert!(error.to_string().contains(&backup));
        assert_eq!(std::fs::read(&backup).unwrap(), b"original");
        assert_eq!(
            std::fs::read(state.part_path().unwrap()).unwrap(),
            b"replacement"
        );
        run(&source, &dest, &endpoint, &mut state).await.unwrap();
        assert_eq!(std::fs::read(dest).unwrap(), b"replacement");
        assert!(!std::path::Path::new(&backup).exists());
    }

    #[tokio::test]
    async fn failed_backup_move_reports_the_existing_original_not_an_uncreated_candidate() {
        let (_dir, source, dest) = files(b"replacement");
        std::fs::write(&dest, b"original").unwrap();
        let endpoint = FaultEndpoint {
            no_replace: true,
            fail_backup: AtomicBool::new(true),
            ..Default::default()
        };
        let mut state = TransferState::default();
        let error = run(&source, &dest, &endpoint, &mut state)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "sftp_transfer_backup_failed");
        assert!(error.to_string().contains(&dest));
        assert!(state.backup_path().is_none());
        assert_eq!(std::fs::read(&dest).unwrap(), b"original");
        assert_eq!(
            std::fs::read(state.part_path().unwrap()).unwrap(),
            b"replacement"
        );
        run(&source, &dest, &endpoint, &mut state).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"replacement");
    }

    #[tokio::test]
    async fn backup_candidate_collision_is_never_adopted_and_retry_uses_a_new_name() {
        let (_dir, source, dest) = files(b"replacement");
        std::fs::write(&dest, b"original").unwrap();
        let endpoint = FaultEndpoint {
            no_replace: true,
            occupy_backup_candidate: AtomicBool::new(true),
            ..Default::default()
        };
        let mut state = TransferState::default();
        let error = run(&source, &dest, &endpoint, &mut state)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "sftp_transfer_backup_failed");
        let occupied = endpoint.renames.lock().unwrap().last().unwrap().1.clone();
        assert!(state.backup_path().is_none());
        assert_eq!(std::fs::read(&occupied).unwrap(), b"preexisting backup");
        run(&source, &dest, &endpoint, &mut state).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"replacement");
        assert_eq!(std::fs::read(&occupied).unwrap(), b"preexisting backup");
    }

    #[tokio::test]
    async fn unknown_backup_outcomes_name_both_locations_and_retry_reconciles_before_install() {
        for moved in [false, true] {
            let (_dir, source, dest) = files(b"replacement");
            std::fs::write(&dest, b"original").unwrap();
            let endpoint = FaultEndpoint {
                no_replace: true,
                fail_backup: AtomicBool::new(!moved),
                lose_backup_reply: AtomicBool::new(moved),
                fail_recovery_readback: AtomicBool::new(true),
                ..Default::default()
            };
            let mut state = TransferState::default();
            let error = run(&source, &dest, &endpoint, &mut state)
                .await
                .unwrap_err();
            assert_eq!(error.code(), "sftp_transfer_backup_location_unknown");
            let backup = state.backup_path().unwrap().to_owned();
            assert!(error.to_string().contains(&dest) && error.to_string().contains(&backup));
            assert_eq!(
                std::fs::read(if moved { &backup } else { &dest }).unwrap(),
                b"original"
            );
            assert_eq!(
                std::fs::read(state.part_path().unwrap()).unwrap(),
                b"replacement"
            );
            run(&source, &dest, &endpoint, &mut state).await.unwrap();
            assert_eq!(std::fs::read(&dest).unwrap(), b"replacement");
            assert!(state.completed());
        }
    }

    #[tokio::test]
    async fn lost_backup_delete_reply_does_not_claim_the_deleted_backup_is_still_present() {
        let (_dir, source, dest) = files(b"replacement");
        std::fs::write(&dest, b"original").unwrap();
        let endpoint = FaultEndpoint {
            no_replace: true,
            lose_cleanup_reply: AtomicBool::new(true),
            ..Default::default()
        };
        let mut state = TransferState::default();
        let error = run(&source, &dest, &endpoint, &mut state)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "sftp_transfer_backup_location_unknown");
        assert!(!std::path::Path::new(state.backup_path().unwrap()).exists());
        assert_eq!(std::fs::read(&dest).unwrap(), b"replacement");
        run(&source, &dest, &endpoint, &mut state).await.unwrap();
        assert!(state.completed());
    }

    #[tokio::test]
    async fn lost_rename_replies_reconcile_without_replacing_a_second_time() {
        for backup_reply in [false, true] {
            let (_dir, source, dest) = files(b"replacement");
            std::fs::write(&dest, b"original").unwrap();
            let endpoint = FaultEndpoint {
                no_replace: backup_reply,
                lose_direct_reply: AtomicBool::new(!backup_reply),
                lose_backup_reply: AtomicBool::new(backup_reply),
                ..Default::default()
            };
            let mut state = TransferState::default();
            assert!(run(&source, &dest, &endpoint, &mut state).await.is_err());
            run(&source, &dest, &endpoint, &mut state).await.unwrap();
            assert_eq!(std::fs::read(dest).unwrap(), b"replacement");
            assert!(state.completed());
            assert_eq!(
                endpoint.renames.lock().unwrap().len(),
                if backup_reply { 3 } else { 1 }
            );
        }
    }

    async fn interrupted(source: &str, dest: &str) -> TransferState {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let (base, _events) = crate::ssh::test_support::headless_host();
        let host = Host::new(
            base.weak_state().upgrade().unwrap(),
            Arc::new(move |_, event| {
                if event["phase"] == "transferring" && event["transferred"].as_u64().unwrap() > 0 {
                    flag.store(true, Ordering::Relaxed);
                }
                true
            }),
        );
        let mut state = TransferState::default();
        let error = transfer(
            &NativeEndpoint,
            source,
            &NativeEndpoint,
            dest,
            &mut state,
            &host,
            "test-transfer",
            cancel,
            &tokio::sync::Mutex::new(()),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "transfer_cancelled");
        assert!(state.transferred() > 0);
        state
    }

    #[tokio::test]
    async fn interrupted_transfer_reads_back_prefix_and_resumes_from_valid_bytes() {
        let content: Vec<u8> = (0..220_000).map(|n| (n % 251) as u8).collect();
        let (_dir, source, dest) = files(&content);
        std::fs::write(&dest, b"original").unwrap();
        let mut state = interrupted(&source, &dest).await;
        let offset = state.transferred();
        assert!(offset < content.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), b"original");
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let (base, _events) = crate::ssh::test_support::headless_host();
        let host = Host::new(
            base.weak_state().upgrade().unwrap(),
            Arc::new(move |_, event| {
                captured.lock().unwrap().push(event);
                true
            }),
        );
        transfer(
            &NativeEndpoint,
            &source,
            &NativeEndpoint,
            &dest,
            &mut state,
            &host,
            "test-transfer",
            Arc::new(AtomicBool::new(false)),
            &tokio::sync::Mutex::new(()),
        )
        .await
        .unwrap();
        let events = events.lock().unwrap();
        assert!(events
            .iter()
            .any(|e| e["phase"] == "verifying" && e["verificationTotal"] == offset * 2));
        assert_eq!(
            events
                .iter()
                .find(|e| e["phase"] == "transferring")
                .unwrap()["transferred"],
            offset
        );
        assert_eq!(std::fs::read(dest).unwrap(), content);
    }

    #[tokio::test]
    async fn same_size_same_mtime_source_edits_and_partial_tampering_are_rejected() {
        for edit_source in [false, true] {
            let (_dir, source, dest) = files(&vec![b'a'; 180_000]);
            std::fs::write(&dest, b"original").unwrap();
            let mut state = interrupted(&source, &dest).await;
            let path = if edit_source {
                source.clone()
            } else {
                state.part_path().unwrap().to_owned()
            };
            let metadata = std::fs::metadata(&path).unwrap();
            let mut changed = std::fs::read(&path).unwrap();
            let index = if edit_source { changed.len() - 1 } else { 0 };
            changed[index] = b'b';
            std::fs::write(&path, changed).unwrap();
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
                .unwrap();
            let error = run(&source, &dest, &NativeEndpoint, &mut state)
                .await
                .unwrap_err();
            assert_eq!(
                error.code(),
                if edit_source {
                    "sftp_transfer_source_changed"
                } else {
                    "sftp_transfer_partial_changed"
                }
            );
            assert_eq!(std::fs::read(dest).unwrap(), b"original");
        }
    }

    #[tokio::test]
    async fn replaced_target_or_backup_is_never_overwritten_or_deleted_by_retry() {
        let (_dir, source, dest) = files(&vec![b'a'; 180_000]);
        std::fs::write(&dest, b"original").unwrap();
        let mut state = interrupted(&source, &dest).await;
        std::fs::write(&dest, b"another download").unwrap();
        assert_eq!(
            run(&source, &dest, &NativeEndpoint, &mut state)
                .await
                .unwrap_err()
                .code(),
            "sftp_transfer_target_changed"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"another download");

        let endpoint = FaultEndpoint {
            no_replace: true,
            fail_cleanup: AtomicBool::new(true),
            ..Default::default()
        };
        let mut state = TransferState::default();
        assert_eq!(
            run(&source, &dest, &endpoint, &mut state)
                .await
                .unwrap_err()
                .code(),
            "sftp_transfer_backup_location_unknown"
        );
        let backup = state.backup_path().unwrap().to_owned();
        std::fs::write(&backup, b"unrelated backup").unwrap();
        assert_eq!(
            run(&source, &dest, &endpoint, &mut state)
                .await
                .unwrap_err()
                .code(),
            "sftp_transfer_backup_location_unknown"
        );
        assert_eq!(std::fs::read(backup).unwrap(), b"unrelated backup");
    }
}
