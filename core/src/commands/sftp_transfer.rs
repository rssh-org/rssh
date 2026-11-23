//! A queue transfer survives an SFTP channel, but never changes owner, endpoint,
//! source or receiver. Runtime-only checkpoints deliberately do not adopt files
//! left by another process or a different queue entry.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use serde_json::json;

use crate::emitter::Host;
use crate::error::{locked, AppError, AppResult};
use crate::files::transfer::TransferEndpoint;
use crate::ssh::client::VerifiedSshIdentity;
use crate::ssh::transfer::TransferState;
use crate::state::{AppState, SessionKind, SessionOwner, SessionPhase};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Download,
    Upload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Binding {
    owner: SessionOwner,
    identity: VerifiedSshIdentity,
    direction: Direction,
    source: String,
    destination: String,
}

struct Record {
    binding: Binding,
    checkpoint: Arc<tokio::sync::Mutex<TransferState>>,
}

#[derive(Default)]
pub struct TransferRegistry {
    records: Mutex<HashMap<String, Record>>,
    pending_cancels: Mutex<HashMap<String, SessionOwner>>,
    destinations: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
}

impl TransferRegistry {
    fn checkpoint(
        &self,
        id: &str,
        binding: Binding,
    ) -> AppResult<Arc<tokio::sync::Mutex<TransferState>>> {
        uuid::Uuid::parse_str(id)
            .map_err(|_| AppError::sftp("transfer_identity_changed", json!({})))?;
        let mut records = locked(&self.records)?;
        if let Some(record) = records.get(id) {
            if record.binding != binding {
                return Err(AppError::sftp("transfer_identity_changed", json!({})));
            }
            return Ok(record.checkpoint.clone());
        }
        let checkpoint = Arc::new(tokio::sync::Mutex::new(TransferState::default()));
        records.insert(
            id.to_owned(),
            Record {
                binding,
                checkpoint: checkpoint.clone(),
            },
        );
        Ok(checkpoint)
    }

    fn destination(&self, key: String) -> AppResult<Arc<tokio::sync::Mutex<()>>> {
        let mut destinations = locked(&self.destinations)?;
        destinations.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = destinations.get(&key).and_then(Weak::upgrade) {
            return Ok(lock);
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        destinations.insert(key, Arc::downgrade(&lock));
        Ok(lock)
    }

    pub fn validate_rebind(
        &self,
        owner: &SessionOwner,
        id: &str,
        identity: &VerifiedSshIdentity,
    ) -> AppResult<()> {
        if let Some(record) = locked(&self.records)?.get(id) {
            if &record.binding.owner != owner || &record.binding.identity != identity {
                return Err(AppError::sftp("transfer_identity_changed", json!({})));
            }
        }
        Ok(())
    }

    pub fn begin_attempt(&self, owner: &SessionOwner, id: &str) -> AppResult<()> {
        let records = locked(&self.records)?;
        if let Some(record) = records.get(id) {
            if &record.binding.owner != owner {
                return Err(AppError::sftp("transfer_identity_changed", json!({})));
            }
            if Arc::strong_count(&record.checkpoint) != 1 {
                return Err(AppError::sftp("transfer_busy", json!({})));
            }
        }
        let mut pending = locked(&self.pending_cancels)?;
        if pending.get(id).is_some_and(|requester| requester != owner) {
            return Err(AppError::sftp("transfer_identity_changed", json!({})));
        }
        // The prior attempt may have failed during native endpoint preparation,
        // before it could consume cancellation. A new channel starts a new attempt.
        pending.remove(id);
        Ok(())
    }

    pub fn validate_owner(&self, owner: &SessionOwner, id: &str) -> AppResult<()> {
        if let Some(record) = locked(&self.records)?.get(id) {
            if &record.binding.owner != owner {
                return Err(AppError::sftp("transfer_identity_changed", json!({})));
            }
        }
        Ok(())
    }

    pub fn cancel(&self, state: &AppState, owner: &SessionOwner, id: &str) -> AppResult<()> {
        uuid::Uuid::parse_str(id)
            .map_err(|_| AppError::sftp("transfer_identity_changed", json!({})))?;
        self.validate_owner(owner, id)?;
        // Keep the active-flag lock until the pending request is recorded. A
        // new attempt cannot register/apply cancellation in the gap.
        let active = locked(&state.transfer_cancels)?;
        if let Some(flag) = active.get(id) {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        } else {
            let mut pending = locked(&self.pending_cancels)?;
            if pending.get(id).is_some_and(|existing| existing != owner) {
                return Err(AppError::sftp("transfer_identity_changed", json!({})));
            }
            pending.insert(id.to_owned(), owner.clone());
        }
        Ok(())
    }

    fn apply_pending_cancel(
        &self,
        owner: &SessionOwner,
        id: &str,
        flag: &std::sync::atomic::AtomicBool,
    ) -> AppResult<()> {
        let mut pending = locked(&self.pending_cancels)?;
        if let Some(requester) = pending.get(id) {
            if requester != owner {
                return Err(AppError::sftp("transfer_identity_changed", json!({})));
            }
            pending.remove(id);
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }

    pub fn forget(&self, owner: &SessionOwner, id: &str) -> AppResult<()> {
        let mut records = locked(&self.records)?;
        if let Some(record) = records.get(id) {
            if &record.binding.owner != owner {
                return Err(AppError::sftp("transfer_identity_changed", json!({})));
            }
            // A caller may already hold its checkpoint but not yet have polled
            // try_lock. Do not let forget replace that record with another
            // checkpoint for the same transfer ID in the intervening window.
            if Arc::strong_count(&record.checkpoint) != 1 {
                return Err(AppError::sftp("transfer_busy", json!({})));
            }
            let _idle = record
                .checkpoint
                .try_lock()
                .map_err(|_| AppError::sftp("transfer_busy", json!({})))?;
        }
        // Never delete a retained .part or .bak as a side effect of removing a row.
        let mut pending = locked(&self.pending_cancels)?;
        if pending.get(id).is_some_and(|requester| requester != owner) {
            return Err(AppError::sftp("transfer_identity_changed", json!({})));
        }
        pending.remove(id);
        records.remove(id);
        Ok(())
    }
}

pub fn session_identity(
    state: &AppState,
    owner: &SessionOwner,
    session_id: &str,
) -> AppResult<VerifiedSshIdentity> {
    let registry = locked(&state.lifecycle_sessions)?;
    let record = registry
        .get(session_id)
        .ok_or_else(|| AppError::not_found("ssh_session_not_found_msg", json!({})))?;
    if &record.owner != owner
        || record.kind != SessionKind::Ssh
        || record.phase() != SessionPhase::Ready
    {
        return Err(AppError::sftp("transfer_identity_changed", json!({})));
    }
    locked(&state.sessions)?
        .get(session_id)
        .map(|session| session.identity().clone())
        .ok_or_else(|| AppError::not_found("ssh_session_not_found_msg", json!({})))
}

/// Both host adapters enter this function, so ownership, same-target exclusion,
/// cancellation, checkpoint handling and progress are identical on every host.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    state: &AppState,
    owner: &SessionOwner,
    host: &Host,
    sftp_id: &str,
    local: &dyn TransferEndpoint,
    local_path: &str,
    remote_path: &str,
    transfer_id: &str,
    direction: Direction,
) -> AppResult<()> {
    let sftp = super::sftp::get_sftp(state, sftp_id)?;
    let parent = sftp
        .parent_ssh_id()
        .ok_or_else(|| AppError::sftp("transfer_identity_changed", json!({})))?;
    {
        let sessions = locked(&state.lifecycle_sessions)?;
        let record = sessions
            .get(sftp_id)
            .ok_or_else(|| AppError::not_found("sftp_session_not_found", json!({})))?;
        if &record.owner != owner || record.phase() != SessionPhase::Ready {
            return Err(AppError::sftp("transfer_identity_changed", json!({})));
        }
    }
    let identity = session_identity(state, owner, parent)?;
    let (source, source_path, receiver, destination): (
        &dyn TransferEndpoint,
        &str,
        &dyn TransferEndpoint,
        &str,
    ) = match direction {
        Direction::Download => (sftp.as_ref(), remote_path, local, local_path),
        Direction::Upload => (local, local_path, sftp.as_ref(), remote_path),
    };
    let binding = Binding {
        owner: owner.clone(),
        identity: identity.clone(),
        direction,
        source: source_path.to_owned(),
        destination: destination.to_owned(),
    };
    let checkpoint = state.sftp_transfers.checkpoint(transfer_id, binding)?;
    let mut checkpoint = checkpoint
        .try_lock()
        .map_err(|_| AppError::sftp("transfer_busy", json!({})))?;
    let (_cancel_guard, cancel) =
        super::sftp::CancelGuard::register(state, transfer_id.to_owned())?;
    state
        .sftp_transfers
        .apply_pending_cancel(owner, transfer_id, &cancel)?;
    if direction == Direction::Upload {
        sftp.prepare_upload(remote_path).await?;
    }
    let transfer_source = source.resolve_source(source_path).await?;
    let destination_identity = receiver.canonicalize_target(destination).await?;
    // The canonical target is shared across windows and channels. Downloads are
    // local; uploads share exclusion by host key even through aliases/accounts.
    // Rebinding still checks the full endpoint/account identity above.
    let key = match direction {
        Direction::Download => format!("local:{destination_identity}"),
        Direction::Upload => format!("remote:{}:{destination_identity}", identity.host_key),
    };
    let destination_lock = state.sftp_transfers.destination(key)?;
    let _destination = destination_lock
        .try_lock()
        .map_err(|_| AppError::sftp("transfer_target_busy", json!({})))?;
    crate::ssh::transfer::transfer(
        source,
        &transfer_source,
        receiver,
        destination,
        &mut checkpoint,
        host,
        transfer_id,
        cancel,
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> Binding {
        Binding {
            owner: SessionOwner::Window("main".into()),
            identity: VerifiedSshIdentity {
                host: "host".into(),
                port: 22,
                username: "user".into(),
                host_key: "SHA256:one".into(),
            },
            direction: Direction::Upload,
            source: "/local".into(),
            destination: "/remote".into(),
        }
    }

    #[tokio::test]
    async fn retries_keep_checkpoint_and_reject_changed_identity_or_paths() {
        let registry = TransferRegistry::default();
        let id = uuid::Uuid::new_v4().to_string();
        let first = registry.checkpoint(&id, binding()).unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &registry.checkpoint(&id, binding()).unwrap()
        ));
        for alter in 0..5 {
            let mut changed = binding();
            match alter {
                0 => changed.owner = SessionOwner::Window("other".into()),
                1 => changed.identity.host_key = "SHA256:two".into(),
                2 => changed.identity.username = "other".into(),
                3 => changed.source = "/other".into(),
                _ => changed.destination = "/other".into(),
            }
            assert!(registry.checkpoint(&id, changed).is_err());
        }
        let guard = first.lock().await;
        assert_eq!(
            registry.forget(&binding().owner, &id).unwrap_err().code(),
            "transfer_busy"
        );
        drop(guard);
        // Even an unpolled caller holding a clone prevents removal.
        assert_eq!(
            registry.forget(&binding().owner, &id).unwrap_err().code(),
            "transfer_busy"
        );
        let previous = Arc::downgrade(&first);
        drop(first);
        registry.forget(&binding().owner, &id).unwrap();
        assert!(previous.upgrade().is_none());
        registry.checkpoint(&id, binding()).unwrap();
    }

    #[test]
    fn target_lock_is_shared_while_held_and_rebind_checks_actual_key() {
        let registry = TransferRegistry::default();
        let target = registry.destination("local:/target".into()).unwrap();
        assert!(Arc::ptr_eq(
            &target,
            &registry.destination("local:/target".into()).unwrap()
        ));
        let _busy = target.try_lock().unwrap();
        assert!(registry
            .destination("local:/target".into())
            .unwrap()
            .try_lock()
            .is_err());
        let id = uuid::Uuid::new_v4().to_string();
        registry.checkpoint(&id, binding()).unwrap();
        let mut different = binding().identity;
        different.host_key = "SHA256:changed".into();
        assert!(registry
            .validate_rebind(&binding().owner, &id, &different)
            .is_err());
        assert!(registry
            .validate_rebind(&binding().owner, &id, &binding().identity)
            .is_ok());
    }

    #[test]
    fn early_cancellation_is_owner_scoped_consumed_once_and_forgotten_with_the_row() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (host, _events) = crate::ssh::test_support::headless_host();
        let state = host.state();
        let registry = &state.sftp_transfers;
        let owner = binding().owner;
        let other = SessionOwner::Window("other".into());
        let id = uuid::Uuid::new_v4().to_string();
        registry.cancel(state, &owner, &id).unwrap();
        let flag = AtomicBool::new(false);
        assert!(registry.apply_pending_cancel(&other, &id, &flag).is_err());
        assert!(!flag.load(Ordering::SeqCst));
        registry.apply_pending_cancel(&owner, &id, &flag).unwrap();
        assert!(flag.load(Ordering::SeqCst));
        let retry = AtomicBool::new(false);
        registry.apply_pending_cancel(&owner, &id, &retry).unwrap();
        assert!(!retry.load(Ordering::SeqCst));

        // If the frontend cancelled while connecting, no transfer body consumed
        // that request. Starting the next attempt clears it, while a later
        // cancellation during the new connection still reaches its body.
        registry.cancel(state, &owner, &id).unwrap();
        assert!(registry.begin_attempt(&other, &id).is_err());
        registry.begin_attempt(&owner, &id).unwrap();
        registry.apply_pending_cancel(&owner, &id, &retry).unwrap();
        assert!(!retry.load(Ordering::SeqCst));
        registry.cancel(state, &owner, &id).unwrap();
        registry.apply_pending_cancel(&owner, &id, &retry).unwrap();
        assert!(retry.load(Ordering::SeqCst));
        retry.store(false, Ordering::SeqCst);

        registry.cancel(state, &owner, &id).unwrap();
        assert!(registry.forget(&other, &id).is_err());
        registry.forget(&owner, &id).unwrap();
        registry.apply_pending_cancel(&owner, &id, &retry).unwrap();
        assert!(!retry.load(Ordering::SeqCst));
    }
}

#[cfg(all(test, macos))]
mod openssh_tests;
