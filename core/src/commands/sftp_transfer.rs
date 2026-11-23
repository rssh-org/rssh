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
    direction: Direction,
    source: String,
    destination: String,
}

struct Record {
    owner: SessionOwner,
    identity: VerifiedSshIdentity,
    binding: Option<Binding>,
    checkpoint: Arc<tokio::sync::Mutex<TransferState>>,
}

#[derive(Default)]
pub struct TransferRegistry {
    records: Mutex<HashMap<String, Record>>,
    pending_cancels: Mutex<HashMap<String, SessionOwner>>,
    destinations: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
}

impl TransferRegistry {
    fn register(
        &self,
        owner: &SessionOwner,
        id: &str,
        identity: &VerifiedSshIdentity,
    ) -> AppResult<()> {
        uuid::Uuid::parse_str(id)
            .map_err(|_| AppError::sftp("transfer_identity_changed", json!({})))?;
        let mut records = locked(&self.records)?;
        if let Some(record) = records.get(id) {
            if &record.owner != owner || &record.identity != identity {
                return Err(AppError::sftp("transfer_identity_changed", json!({})));
            }
            return Ok(());
        }
        records.insert(
            id.to_owned(),
            Record {
                owner: owner.clone(),
                identity: identity.clone(),
                binding: None,
                checkpoint: Arc::new(tokio::sync::Mutex::new(TransferState::default())),
            },
        );
        Ok(())
    }

    fn checkpoint(
        &self,
        owner: &SessionOwner,
        id: &str,
        identity: &VerifiedSshIdentity,
        binding: Binding,
    ) -> AppResult<Arc<tokio::sync::Mutex<TransferState>>> {
        let mut records = locked(&self.records)?;
        let record = records
            .get_mut(id)
            .ok_or_else(|| AppError::sftp("transfer_identity_changed", json!({})))?;
        if &record.owner != owner
            || &record.identity != identity
            || record
                .binding
                .as_ref()
                .is_some_and(|saved| saved != &binding)
        {
            return Err(AppError::sftp("transfer_identity_changed", json!({})));
        }
        // Directory authorization and file resolution may fail before this
        // point. The queued transfer's verified SSH identity already exists;
        // the first actual attempt fills its immutable file paths exactly once.
        record.binding.get_or_insert(binding);
        Ok(record.checkpoint.clone())
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
        let records = locked(&self.records)?;
        let record = records
            .get(id)
            .ok_or_else(|| AppError::sftp("transfer_identity_changed", json!({})))?;
        if &record.owner != owner || &record.identity != identity {
            return Err(AppError::sftp("transfer_identity_changed", json!({})));
        }
        Ok(())
    }

    pub fn begin_attempt(&self, owner: &SessionOwner, id: &str) -> AppResult<()> {
        let records = locked(&self.records)?;
        let record = records
            .get(id)
            .ok_or_else(|| AppError::sftp("transfer_identity_changed", json!({})))?;
        if &record.owner != owner {
            return Err(AppError::sftp("transfer_identity_changed", json!({})));
        }
        if Arc::strong_count(&record.checkpoint) != 1 {
            return Err(AppError::sftp("transfer_busy", json!({})));
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
        let records = locked(&self.records)?;
        let record = records
            .get(id)
            .ok_or_else(|| AppError::sftp("transfer_identity_changed", json!({})))?;
        if &record.owner != owner {
            return Err(AppError::sftp("transfer_identity_changed", json!({})));
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
            if &record.owner != owner {
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

/// Capture the queue entry's original, already verified SSH identity before
/// opening an SFTP channel or preparing either file endpoint. Registration does
/// no file I/O and never resets an existing checkpoint or path binding.
pub fn register(
    state: &AppState,
    owner: &SessionOwner,
    session_id: &str,
    transfer_id: &str,
) -> AppResult<()> {
    let identity = session_identity(state, owner, session_id)?;
    state.sftp_transfers.register(owner, transfer_id, &identity)
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
        direction,
        source: source_path.to_owned(),
        destination: destination.to_owned(),
    };
    let checkpoint = state
        .sftp_transfers
        .checkpoint(owner, transfer_id, &identity, binding)?;
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

    fn owner() -> SessionOwner {
        SessionOwner::Window("main".into())
    }

    fn identity() -> VerifiedSshIdentity {
        VerifiedSshIdentity {
            host: "host".into(),
            port: 22,
            username: "user".into(),
            host_key: "SHA256:one".into(),
        }
    }

    fn binding() -> Binding {
        Binding {
            direction: Direction::Upload,
            source: "/local".into(),
            destination: "/remote".into(),
        }
    }

    #[test]
    fn unregistered_transfers_cannot_rebind_or_create_checkpoints() {
        let registry = TransferRegistry::default();
        let id = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            registry
                .validate_rebind(&owner(), &id, &identity())
                .unwrap_err()
                .code(),
            "transfer_identity_changed"
        );
        assert_eq!(
            registry
                .checkpoint(&owner(), &id, &identity(), binding())
                .unwrap_err()
                .code(),
            "transfer_identity_changed"
        );
        assert!(registry.begin_attempt(&owner(), &id).is_err());
        assert!(registry.records.lock().unwrap().is_empty());
    }

    #[test]
    fn queued_transfers_reject_changed_endpoint_account_key_or_owner() {
        let registry = TransferRegistry::default();
        let id = uuid::Uuid::new_v4().to_string();
        registry.register(&owner(), &id, &identity()).unwrap();
        for alter in 0..5 {
            let mut changed_owner = owner();
            let mut changed_identity = identity();
            match alter {
                0 => changed_owner = SessionOwner::Window("other".into()),
                1 => changed_identity.host = "other-host".into(),
                2 => changed_identity.port = 2222,
                3 => changed_identity.username = "other-user".into(),
                _ => changed_identity.host_key = "SHA256:two".into(),
            }
            assert_eq!(
                registry
                    .validate_rebind(&changed_owner, &id, &changed_identity)
                    .unwrap_err()
                    .code(),
                "transfer_identity_changed"
            );
            assert_eq!(
                registry
                    .register(&changed_owner, &id, &changed_identity)
                    .unwrap_err()
                    .code(),
                "transfer_identity_changed"
            );
            assert_eq!(
                registry
                    .checkpoint(&changed_owner, &id, &changed_identity, binding())
                    .unwrap_err()
                    .code(),
                "transfer_identity_changed"
            );
        }
        assert!(registry
            .records
            .lock()
            .unwrap()
            .get(&id)
            .unwrap()
            .binding
            .is_none());
        registry
            .validate_rebind(&owner(), &id, &identity())
            .unwrap();
    }

    #[test]
    fn failed_preparation_keeps_original_identity_until_paths_are_available() {
        let registry = TransferRegistry::default();
        let id = uuid::Uuid::new_v4().to_string();
        registry.register(&owner(), &id, &identity()).unwrap();
        registry.begin_attempt(&owner(), &id).unwrap();
        // A folder authorization/open failure can stop the host adapter before
        // run/checkpoint. A new verified connection still must match the queued
        // identity even though no source/destination binding exists yet.
        let mut changed = identity();
        changed.username = "replacement-user".into();
        assert!(registry.validate_rebind(&owner(), &id, &changed).is_err());
        assert!(registry.register(&owner(), &id, &changed).is_err());
        assert!(registry
            .checkpoint(&owner(), &id, &changed, binding())
            .is_err());
        assert!(registry
            .records
            .lock()
            .unwrap()
            .get(&id)
            .unwrap()
            .binding
            .is_none());
        registry
            .validate_rebind(&owner(), &id, &identity())
            .unwrap();
        registry.begin_attempt(&owner(), &id).unwrap();
        registry
            .checkpoint(&owner(), &id, &identity(), binding())
            .unwrap();
    }

    #[test]
    fn registration_requires_an_owned_ready_verified_session() {
        let (host, _) = crate::ssh::test_support::headless_host();
        let state = host.state();
        let id = uuid::Uuid::new_v4().to_string();
        let reservation = crate::commands::lifecycle::reserve_generated_resource(
            state,
            SessionKind::Ssh,
            owner(),
        )
        .unwrap();
        assert_eq!(
            register(state, &owner(), reservation.id(), &id)
                .unwrap_err()
                .code(),
            "transfer_identity_changed"
        );
        assert!(state.sftp_transfers.records.lock().unwrap().is_empty());
        drop(reservation);
        assert!(register(state, &owner(), "missing-session", &id).is_err());
        assert!(state.sftp_transfers.records.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn retries_keep_checkpoint_and_reject_changed_identity_or_paths() {
        let registry = TransferRegistry::default();
        let id = uuid::Uuid::new_v4().to_string();
        registry.register(&owner(), &id, &identity()).unwrap();
        let first = registry
            .checkpoint(&owner(), &id, &identity(), binding())
            .unwrap();
        // A repeated registration cannot reset either file paths or the state
        // retained by an active or failed transfer.
        registry.register(&owner(), &id, &identity()).unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &registry
                .checkpoint(&owner(), &id, &identity(), binding())
                .unwrap()
        ));
        for alter in 0..3 {
            let mut changed = binding();
            match alter {
                0 => changed.direction = Direction::Download,
                1 => changed.source = "/other".into(),
                _ => changed.destination = "/other".into(),
            }
            assert!(registry
                .checkpoint(&owner(), &id, &identity(), changed)
                .is_err());
        }
        let guard = first.lock().await;
        assert_eq!(
            registry.forget(&owner(), &id).unwrap_err().code(),
            "transfer_busy"
        );
        drop(guard);
        // Even an unpolled caller holding a clone prevents removal.
        assert_eq!(
            registry.forget(&owner(), &id).unwrap_err().code(),
            "transfer_busy"
        );
        let previous = Arc::downgrade(&first);
        drop(first);
        registry.forget(&owner(), &id).unwrap();
        assert!(previous.upgrade().is_none());
        assert!(registry
            .checkpoint(&owner(), &id, &identity(), binding())
            .is_err());
        registry.register(&owner(), &id, &identity()).unwrap();
        registry
            .checkpoint(&owner(), &id, &identity(), binding())
            .unwrap();
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
        registry.register(&owner(), &id, &identity()).unwrap();
        let mut different = identity();
        different.host_key = "SHA256:changed".into();
        assert!(registry.validate_rebind(&owner(), &id, &different).is_err());
        assert!(registry.validate_rebind(&owner(), &id, &identity()).is_ok());
    }

    #[test]
    fn early_cancellation_is_owner_scoped_consumed_once_and_forgotten_with_the_row() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (host, _events) = crate::ssh::test_support::headless_host();
        let state = host.state();
        let registry = &state.sftp_transfers;
        let owner = owner();
        let other = SessionOwner::Window("other".into());
        let id = uuid::Uuid::new_v4().to_string();
        assert!(registry.cancel(state, &owner, &id).is_err());
        registry.register(&owner, &id, &identity()).unwrap();
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
