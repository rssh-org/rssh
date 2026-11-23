//! Queue state belongs to a transfer and its window, independently of short-lived
//! SSH/SFTP sessions. Checkout precedes provider I/O so cancellation can reach
//! preparation. A second reservation protects the resolved destination across owners.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::json;

use crate::emitter::Host;
use crate::error::{locked, AppError, AppResult};
use crate::ssh::client::VerifiedPeer;
use crate::ssh::sftp::transfer::{DownloadDirectory, TransferCheckpoint};
use crate::ssh::sftp::SftpHandle;
use crate::state::{AppState, SessionKind, SessionOwner, SessionPhase};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferRequest {
    pub sftp_id: String,
    pub transfer_id: String,
    pub original_session_id: Option<String>,
    pub local_path: String,
    pub remote_path: String,
}

#[derive(Clone, PartialEq, Eq)]
enum Target {
    Local(String),
    Remote(String, String),
}

impl Target {
    // The remote volume can be case-insensitive even when the client is Linux.
    // Keep the exact path for rebind checks and serialize case aliases on write.
    fn conflicts_with(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Local(a), Self::Local(b)) => a.to_lowercase() == b.to_lowercase(),
            (Self::Remote(server, a), Self::Remote(other, b)) => {
                server == other && a.to_lowercase() == b.to_lowercase()
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Upload,
    Download,
}

#[derive(PartialEq, Eq)]
struct Binding {
    owner: SessionOwner,
    peer: VerifiedPeer,
    local_path: String,
    remote_path: String,
    direction: Direction,
}

struct Entry {
    binding: Binding,
    target: Option<Target>,
    checkpoint: TransferCheckpoint,
    running: Option<Arc<AtomicBool>>,
    target_reserved: bool,
    discard_on_finish: bool,
}

#[derive(Default)]
pub struct TransferQueue(Mutex<HashMap<String, Entry>>);

fn error(code: &'static str) -> AppError {
    AppError::sftp(code, json!({}))
}

impl TransferQueue {
    /// The UI can retain the original parent ID after its terminal is gone.
    /// Match the checkpoint or the original SSH lifecycle record, including
    /// when a queued transfer has not yet opened its first SFTP channel.
    pub(super) fn connection_for(
        &self,
        state: &AppState,
        owner: &SessionOwner,
        id: &str,
        preferred: String,
    ) -> AppResult<String> {
        let registry = locked(&state.lifecycle_sessions)?;
        let expected = {
            let entries = locked(&self.0)?;
            match entries.get(id) {
                Some(entry) if &entry.binding.owner == owner => Some(&entry.binding.peer),
                Some(_) => None,
                None => registry
                    .get(&preferred)
                    .and_then(|record| record.ssh_peer_for(owner)),
            }
            .ok_or_else(|| error("sftp_transfer_identity_changed"))?
            .clone()
        };
        let sessions = locked(&state.sessions)?;
        sessions
            .iter()
            .filter(|(id, session)| {
                registry
                    .get(id.as_str())
                    .is_some_and(|r| &r.owner == owner && r.phase() == SessionPhase::Ready)
                    && session.ssh_handle().peer == expected
                    && !session.ssh_handle().is_closed()
            })
            .min_by_key(|(id, _)| *id != &preferred)
            .map(|(id, _)| id.clone())
            .ok_or_else(|| error("sftp_reconnect_required"))
    }

    fn start(
        &self,
        id: &str,
        binding: Binding,
        original_peer_matches: bool,
    ) -> AppResult<Attempt<'_>> {
        let mut entries = locked(&self.0)?;
        if let Some(entry) = entries.get(id) {
            if entry.binding != binding {
                return Err(error("sftp_transfer_identity_changed"));
            }
            if entry.running.is_some() {
                return Err(error("sftp_transfer_busy"));
            }
        } else if !original_peer_matches {
            return Err(error("sftp_transfer_identity_changed"));
        }
        let flag = Arc::new(AtomicBool::new(false));
        let entry = entries.entry(id.to_owned()).or_insert_with(|| Entry {
            binding,
            target: None,
            checkpoint: TransferCheckpoint::default(),
            running: None,
            target_reserved: false,
            discard_on_finish: false,
        });
        entry.running = Some(flag.clone());
        Ok(Attempt {
            queue: self,
            id: id.to_owned(),
            cancel: flag,
            checkpoint: std::mem::take(&mut entry.checkpoint),
            complete: false,
        })
    }

    pub fn cancel(&self, owner: &SessionOwner, id: &str) -> AppResult<()> {
        if let Some(entry) = locked(&self.0)?.get(id) {
            if &entry.binding.owner != owner {
                return Err(error("sftp_transfer_identity_changed"));
            }
            if let Some(cancel) = &entry.running {
                cancel.store(true, Ordering::Release);
            }
        }
        Ok(())
    }

    pub fn forget(&self, owner: &SessionOwner, id: &str) -> AppResult<Option<String>> {
        let mut entries = locked(&self.0)?;
        if let Some(entry) = entries.get(id) {
            if &entry.binding.owner != owner {
                return Err(error("sftp_transfer_identity_changed"));
            }
            if entry.running.is_some() {
                return Err(error("sftp_transfer_busy"));
            }
            let recovered = entry.checkpoint.recovery_path().map(str::to_owned);
            entries.remove(id);
            return Ok(recovered);
        }
        Ok(None)
    }

    pub(crate) fn close_owner(&self, owner: &SessionOwner) -> AppResult<()> {
        locked(&self.0)?.retain(|_, entry| {
            if &entry.binding.owner != owner {
                return true;
            }
            if let Some(cancel) = &entry.running {
                cancel.store(true, Ordering::Release);
                entry.discard_on_finish = true;
                return true;
            }
            false
        });
        Ok(())
    }
}

struct Attempt<'a> {
    queue: &'a TransferQueue,
    id: String,
    cancel: Arc<AtomicBool>,
    checkpoint: TransferCheckpoint,
    complete: bool,
}

impl Attempt<'_> {
    fn reserve_target(&mut self, target: Target) -> AppResult<()> {
        if self.cancel.load(Ordering::Acquire) {
            return Err(error(crate::ssh::sftp::CANCELLED_CODE));
        }
        let mut entries = locked(&self.queue.0)?;
        if entries.iter().any(|(id, e)| {
            id != &self.id
                && e.target_reserved
                && e.target.as_ref().is_some_and(|t| t.conflicts_with(&target))
        }) {
            return Err(error("sftp_target_busy"));
        }
        let entry = entries
            .get_mut(&self.id)
            .ok_or_else(|| error("sftp_transfer_identity_changed"))?;
        if entry.target.get_or_insert(target.clone()) != &target {
            return Err(error("sftp_transfer_identity_changed"));
        }
        entry.target_reserved = true;
        Ok(())
    }
}

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        if let Ok(mut entries) = self.queue.0.lock() {
            if let Some(entry) = entries.get_mut(&self.id) {
                if self.complete || entry.discard_on_finish {
                    entries.remove(&self.id);
                } else {
                    entry.checkpoint = std::mem::take(&mut self.checkpoint);
                    entry.target_reserved = false;
                    entry.running = None;
                }
            }
        }
    }
}

impl TransferRequest {
    pub fn begin<'a>(
        &'a self,
        state: &'a AppState,
        owner: &SessionOwner,
        direction: Direction,
    ) -> AppResult<QueueTransfer<'a>> {
        let registry = locked(&state.lifecycle_sessions)?;
        let record = registry
            .get(&self.sftp_id)
            .filter(|entry| entry.kind == SessionKind::Sftp && entry.phase() == SessionPhase::Ready)
            .ok_or_else(|| error("sftp_session_not_found"))?;
        if &record.owner != owner {
            return Err(AppError::config(
                "session_owner_mismatch",
                json!({ "id": self.sftp_id }),
            ));
        }
        let sftp = super::get_sftp(state, &self.sftp_id)?;
        let matches = self.original_session_id.as_deref().is_none_or(|id| {
            registry
                .get(id)
                .and_then(|original| original.ssh_peer_for(owner))
                == Some(&sftp.peer)
        });
        let binding = Binding {
            owner: owner.clone(),
            peer: sftp.peer.clone(),
            local_path: self.local_path.clone(),
            remote_path: self.remote_path.clone(),
            direction,
        };
        let attempt = state.transfers.start(&self.transfer_id, binding, matches)?;
        Ok(QueueTransfer {
            sftp,
            request: self,
            attempt,
        })
    }

    fn progress(&self, host: &Host) -> impl FnMut(u64, u64) + Send {
        let event = format!("sftp:progress:{}", self.transfer_id);
        let host = host.clone();
        move |transferred, total| {
            let _ = host.emit(
                &event,
                json!({ "transferred": transferred, "total": total }),
            );
        }
    }

    pub async fn upload_bytes(
        state: &AppState,
        owner: &SessionOwner,
        sftp_id: String,
        path: String,
        bytes: &[u8],
    ) -> AppResult<()> {
        let request = Self {
            sftp_id,
            transfer_id: uuid::Uuid::new_v4().to_string(),
            original_session_id: None,
            local_path: String::new(),
            remote_path: path,
        };
        let mut transfer = request.begin(state, owner, Direction::Upload)?;
        // The legacy byte-buffer command has no queue row to retry.
        transfer.attempt.complete = true;
        let path = transfer.reserve_upload().await?;
        transfer.sftp.upload(&path, bytes).await
    }
}

pub struct QueueTransfer<'a> {
    sftp: Arc<SftpHandle>,
    request: &'a TransferRequest,
    attempt: Attempt<'a>,
}

impl QueueTransfer<'_> {
    pub async fn download(
        mut self,
        host: &Host,
        dir: &dyn DownloadDirectory,
        name: &str,
    ) -> AppResult<()> {
        self.attempt
            .reserve_target(Target::Local(dir.display_path(name)))?;
        let result = self
            .sftp
            .resume_download(
                &self.request.remote_path,
                dir,
                name,
                &mut self.attempt.checkpoint,
                self.request.progress(host),
                &self.attempt.cancel,
            )
            .await;
        self.attempt.complete = result.is_ok();
        result
    }

    async fn reserve_upload(&mut self) -> AppResult<String> {
        let canonical = self
            .sftp
            .canonical_upload_target(&self.request.remote_path)
            .await?;
        self.attempt.reserve_target(Target::Remote(
            self.sftp.peer.server.host_key.clone(),
            canonical.clone(),
        ))?;
        Ok(canonical)
    }

    pub async fn upload(mut self, host: &Host, source: &mut tokio::fs::File) -> AppResult<()> {
        let canonical = self.reserve_upload().await?;
        let result = self
            .sftp
            .resume_file_upload(
                &canonical,
                source,
                &mut self.attempt.checkpoint,
                self.request.progress(host),
                &self.attempt.cancel,
            )
            .await;
        self.attempt.complete = result.is_ok();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(owner: &SessionOwner) -> Binding {
        Binding {
            owner: owner.clone(),
            peer: VerifiedPeer::default(),
            direction: Direction::Upload,
            local_path: "source".into(),
            remote_path: "/file".into(),
        }
    }

    #[test]
    fn queued_without_checkpoint_uses_only_an_authenticated_original_from_its_owner() {
        use crate::commands::lifecycle::reserve_resource;

        let (host, _events) = crate::ssh::test_support::headless_host();
        let state = host.state();
        let owner = SessionOwner::Window("main".into());
        let other = SessionOwner::Window("other".into());
        let source_id = "original";
        let connect = |owner| {
            state
                .transfers
                .connection_for(state, owner, "never-dispatched", source_id.into())
        };
        let reservation =
            reserve_resource(state, source_id, SessionKind::Ssh, owner.clone()).unwrap();
        assert_eq!(
            connect(&owner).unwrap_err().code(),
            "sftp_transfer_identity_changed"
        );
        drop(reservation);
        // A failed handshake also leaves a tombstone but establishes no identity.
        assert_eq!(
            connect(&owner).unwrap_err().code(),
            "sftp_transfer_identity_changed"
        );
        {
            let mut registry = state.lifecycle_sessions.lock().unwrap();
            let record = registry.get_mut(source_id).unwrap();
            assert_eq!(record.phase(), SessionPhase::Closed);
            record.verified_ssh_peer = Some(Box::new(VerifiedPeer::default()));
        }
        // The old typed handle has gone away. A valid historical identity
        // requires a live matching SSH and must never return the stale ID.
        assert_eq!(
            connect(&owner).unwrap_err().code(),
            "sftp_reconnect_required"
        );
        assert_eq!(
            connect(&other).unwrap_err().code(),
            "sftp_transfer_identity_changed"
        );
        assert!(state.transfers.0.lock().unwrap().is_empty());
    }

    #[test]
    fn retries_require_the_same_authenticated_peer_owner_direction_and_target() {
        let queue = TransferQueue::default();
        let owner = SessionOwner::Window("main".into());
        assert!(queue.start("a", binding(&owner), false).is_err());
        let mut first = queue.start("a", binding(&owner), true).unwrap();
        first.reserve_target(Target::Local("first".into())).unwrap();
        assert!(queue.start("a", binding(&owner), true).is_err());
        drop(first);
        let other = SessionOwner::Window("other".into());
        let mut changed = binding(&owner);
        changed.peer.server.host_key = "different server key".into();
        assert!(queue.start("a", changed, true).is_err());
        assert!(queue.start("a", binding(&other), true).is_err());
        let mut changed = binding(&owner);
        changed.direction = Direction::Download;
        assert!(queue.start("a", changed, true).is_err());
        let mut rebound = queue.start("a", binding(&owner), false).unwrap();
        assert!(rebound
            .reserve_target(Target::Local("other".into()))
            .is_err());
        rebound
            .reserve_target(Target::Local("first".into()))
            .unwrap();
    }

    #[test]
    fn destination_reservation_survives_owner_close_until_running_attempt_exits() {
        let queue = TransferQueue::default();
        let first = SessionOwner::Window("a".into());
        let second = SessionOwner::Window("b".into());
        let mut a = queue.start("a", binding(&first), true).unwrap();
        let mut b = queue.start("b", binding(&second), true).unwrap();
        a.reserve_target(Target::Local("File".into())).unwrap();
        assert!(queue.cancel(&second, "a").is_err());
        queue.close_owner(&first).unwrap();
        assert!(a.cancel.load(Ordering::Acquire));
        assert!(b.reserve_target(Target::Local("file".into())).is_err());
        drop(a);
        b.reserve_target(Target::Local("file".into())).unwrap();
        assert!(queue.forget(&second, "b").is_err());
        drop(b);
        queue.forget(&second, "b").unwrap();
        assert!(queue.0.lock().unwrap().is_empty());
    }
}
