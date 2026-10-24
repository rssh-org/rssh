//! Cancellation and cleanup contracts for resources whose release can fail.
//!
//! Identity, ownership and activation belong to the lifecycle registry. Platform
//! adapters register cleanup before starting an operation with external effects.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::error::{locked, AppError, AppResult};

pub type CleanupFuture<'a> = Pin<Box<dyn Future<Output = AppResult<()>> + Send + 'a>>;

/// Implementations must allow close to be retried after failure and repeated
/// after success. A successful return means the resource has been released.
pub trait Cleanup: Send + Sync {
    fn close(&self) -> CleanupFuture<'_>;
}

#[derive(Clone)]
pub struct CleanupHandle(Arc<dyn Cleanup>);

impl CleanupHandle {
    pub fn new(cleanup: Arc<dyn Cleanup>) -> Self {
        Self(cleanup)
    }

    async fn close(&self) -> AppResult<()> {
        self.0.close().await
    }
}

impl std::fmt::Debug for CleanupHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CleanupHandle")
    }
}

#[derive(Clone, Debug)]
enum OperationCompletion {
    Complete(AppResult<()>),
    Interrupted,
}

#[derive(Clone, Debug)]
pub(crate) struct PendingOperationState {
    cancel: tokio::sync::watch::Sender<bool>,
    finished: tokio::sync::watch::Receiver<Option<OperationCompletion>>,
    cleanup: Arc<Mutex<Option<CleanupHandle>>>,
}

/// The opening task owns this guard until activation or failed-open cleanup.
pub struct PendingOperation {
    cancelled: tokio::sync::watch::Receiver<bool>,
    finished: tokio::sync::watch::Sender<Option<OperationCompletion>>,
    cleanup: Arc<Mutex<Option<CleanupHandle>>>,
}

impl PendingOperation {
    pub(crate) fn new() -> (Self, PendingOperationState) {
        let (cancel, cancelled) = tokio::sync::watch::channel(false);
        let (finished, completion) = tokio::sync::watch::channel(None);
        let cleanup = Arc::new(Mutex::new(None));
        (
            Self {
                cancelled,
                finished,
                cleanup: cleanup.clone(),
            },
            PendingOperationState {
                cancel,
                finished: completion,
                cleanup,
            },
        )
    }

    pub async fn cancelled(&self) {
        let mut cancelled = self.cancelled.clone();
        while !*cancelled.borrow_and_update() {
            if cancelled.changed().await.is_err() {
                return;
            }
        }
    }

    /// Register before dispatching the native operation, so aborting its future
    /// cannot lose a partially acquired resource.
    pub fn retain_cleanup(&self, handle: CleanupHandle) -> AppResult<()> {
        let mut cleanup = locked(&self.cleanup)?;
        if cleanup.is_some() {
            return Err(AppError::other(
                "session_registry_inconsistent",
                serde_json::json!({}),
            ));
        }
        *cleanup = Some(handle);
        Ok(())
    }

    /// Completion means the task ended; only Ok means cleanup or activation
    /// transferred ownership successfully.
    pub fn complete(self, mut result: AppResult<()>) {
        if result.is_ok() {
            match locked(&self.cleanup) {
                Ok(mut cleanup) => drop(cleanup.take()),
                Err(error) => result = Err(error),
            }
        }
        self.finished
            .send_replace(Some(OperationCompletion::Complete(result)));
    }

    async fn fail(self, error: AppError) -> AppError {
        let cleanup = locked(&self.cleanup).map(|cleanup| cleanup.clone());
        let result = match cleanup {
            Ok(Some(cleanup)) => cleanup.close().await,
            Ok(None) => Ok(()),
            Err(error) => Err(error),
        };
        self.complete(result);
        error
    }

    pub async fn finish_open<T>(
        self,
        result: AppResult<(String, T)>,
    ) -> AppResult<OpenedResource<T>> {
        match result {
            Ok((id, handle)) => Ok(OpenedResource {
                id,
                handle,
                operation: self,
            }),
            Err(error) => Err(self.fail(error).await),
        }
    }
}

impl Drop for PendingOperation {
    fn drop(&mut self) {
        let completed = self.finished.borrow().is_some();
        if !completed {
            self.finished
                .send_replace(Some(OperationCompletion::Interrupted));
        }
    }
}

pub struct OpenedResource<T> {
    id: String,
    handle: T,
    operation: PendingOperation,
}

impl<T> OpenedResource<T> {
    pub async fn activate(
        self,
        activate: impl FnOnce(&str, T) -> AppResult<()>,
    ) -> AppResult<String> {
        let Self {
            id,
            handle,
            operation,
        } = self;
        match activate(&id, handle) {
            Ok(()) => {
                operation.complete(Ok(()));
                Ok(id)
            }
            Err(error) => Err(operation.fail(error).await),
        }
    }
}

#[derive(Clone, Debug)]
enum CleanupState {
    Pending(PendingOperationState),
    Resource(CleanupHandle),
    Failed(AppError),
    Finished,
}

/// The registry retains this owner until cleanup succeeds. Concurrent callers
/// join its operation; failures leave the resource available for another close.
#[derive(Debug)]
pub(crate) struct ResourceCleanup {
    operation: tokio::sync::Mutex<()>,
    state: Mutex<CleanupState>,
    background_started: AtomicBool,
}

impl ResourceCleanup {
    fn new(state: CleanupState) -> Arc<Self> {
        Arc::new(Self {
            operation: tokio::sync::Mutex::new(()),
            state: Mutex::new(state),
            background_started: AtomicBool::new(false),
        })
    }

    pub(crate) fn pending(operation: PendingOperationState) -> Arc<Self> {
        operation.cancel.send_replace(true);
        Self::new(CleanupState::Pending(operation))
    }

    pub(crate) fn ready(cleanup: CleanupHandle) -> Arc<Self> {
        Self::new(CleanupState::Resource(cleanup))
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| matches!(*state, CleanupState::Finished))
    }

    pub(crate) async fn finish(&self) -> AppResult<()> {
        let _operation = self.operation.lock().await;
        // The stored state retains ownership throughout the await. Phase reads
        // do not depend on whether another closer currently holds the gate.
        let attempt = locked(&self.state)?.clone();
        match attempt {
            CleanupState::Pending(mut operation) => {
                let completion = loop {
                    if let Some(result) = operation.finished.borrow_and_update().clone() {
                        break result;
                    }
                    if operation.finished.changed().await.is_err() {
                        break OperationCompletion::Interrupted;
                    }
                };
                let retained = locked(&operation.cleanup)?.take();
                // An interrupted opener never acknowledged a cleanup attempt.
                // Adopt its resource before awaiting close, so cancellation of
                // this closer still leaves a retriable owner in the registry.
                let result = match completion {
                    OperationCompletion::Complete(result) => result,
                    OperationCompletion::Interrupted => {
                        if let Some(resource) = retained {
                            *locked(&self.state)? = CleanupState::Resource(resource.clone());
                            resource.close().await?;
                        }
                        // No registration means no external operation was dispatched.
                        *locked(&self.state)? = CleanupState::Finished;
                        return Ok(());
                    }
                };
                *locked(&self.state)? = match (&result, retained) {
                    (Ok(()), _) => CleanupState::Finished,
                    (Err(_), Some(resource)) => CleanupState::Resource(resource),
                    (Err(error), None) => CleanupState::Failed(error.clone()),
                };
                result
            }
            CleanupState::Resource(resource) => {
                resource.close().await?;
                *locked(&self.state)? = CleanupState::Finished;
                Ok(())
            }
            CleanupState::Failed(error) => Err(error),
            CleanupState::Finished => Ok(()),
        }
    }

    pub(crate) fn start(self: &Arc<Self>) {
        if self.background_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let cleanup = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut delay = Duration::from_millis(100);
            while let Err(error) = cleanup.finish().await {
                log::warn!("session cleanup failed; retained for retry: {error}");
                // Only an actual retained resource can be retried. A failed
                // operation with no handle has nothing for a worker to release.
                if !cleanup.state.lock().is_ok_and(|state| {
                    matches!(*state, CleanupState::Pending(_) | CleanupState::Resource(_))
                }) {
                    break;
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(30));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct Probe {
        calls: AtomicUsize,
        fail: AtomicBool,
        started: tokio::sync::Notify,
        release: Option<tokio::sync::Notify>,
    }

    impl Cleanup for Probe {
        fn close(&self) -> CleanupFuture<'_> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.started.notify_one();
                if self.fail.swap(false, Ordering::SeqCst) {
                    return Err(error("cleanup_failed"));
                }
                if let Some(release) = &self.release {
                    release.notified().await;
                }
                Ok(())
            })
        }
    }

    fn error(code: &'static str) -> AppError {
        AppError::other(code, serde_json::json!({}))
    }

    fn opening(probe: &Arc<Probe>) -> (PendingOperation, PendingOperationState) {
        let (operation, state) = PendingOperation::new();
        operation
            .retain_cleanup(CleanupHandle::new(probe.clone()))
            .unwrap();
        (operation, state)
    }

    #[tokio::test]
    async fn background_cleanup_retries_without_a_live_frontend() {
        let probe = Arc::new(Probe::default());
        probe.fail.store(true, Ordering::SeqCst);
        let cleanup = ResourceCleanup::ready(CleanupHandle::new(probe.clone()));
        cleanup.start();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !cleanup.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("destroyed windows have nobody left to request another close");
        assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn repeated_background_starts_share_one_retry_worker() {
        let probe = Arc::new(Probe {
            release: Some(tokio::sync::Notify::new()),
            ..Default::default()
        });
        let cleanup = ResourceCleanup::ready(CleanupHandle::new(probe.clone()));
        cleanup.start();
        cleanup.start();
        probe.started.notified().await;
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
        probe.release.as_ref().unwrap().notify_one();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !cleanup.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cleanup.finish().await.unwrap();
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_open_waits_for_release_and_keeps_its_original_error() {
        let probe = Arc::new(Probe {
            release: Some(tokio::sync::Notify::new()),
            ..Default::default()
        });
        let (operation, state) = opening(&probe);
        let opening = operation.finish_open::<()>(Err(error("open_failed")));
        tokio::pin!(opening);
        tokio::select! {
            biased;
            _ = &mut opening => panic!("open returned before cleanup"),
            _ = probe.started.notified() => {},
        }
        let cleanup = ResourceCleanup::pending(state);
        assert!(!cleanup.is_finished());
        probe.release.as_ref().unwrap().notify_one();
        assert_eq!(opening.await.err().unwrap().code(), "open_failed");
        cleanup.finish().await.unwrap();
        assert!(cleanup.is_finished());
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_open_and_failed_cleanup_keep_independent_errors_and_retry_ownership() {
        let probe = Arc::new(Probe::default());
        probe.fail.store(true, Ordering::SeqCst);
        let (operation, state) = opening(&probe);
        assert_eq!(
            operation
                .finish_open::<()>(Err(error("open_failed")))
                .await
                .err()
                .unwrap()
                .code(),
            "open_failed"
        );
        let cleanup = ResourceCleanup::pending(state);
        assert_eq!(cleanup.finish().await.unwrap_err().code(), "cleanup_failed");
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
        cleanup.finish().await.unwrap();
        assert!(cleanup.is_finished());
        assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn activation_failure_releases_registered_resource_but_success_transfers_it() {
        for succeeds in [false, true] {
            let probe = Arc::new(Probe::default());
            let (operation, state) = opening(&probe);
            let opened = operation
                .finish_open(Ok(("attempt".into(), 17)))
                .await
                .unwrap();
            let result = opened
                .activate(|id, handle| {
                    assert_eq!(id, "attempt");
                    assert_eq!(handle, 17);
                    if succeeds {
                        Ok(())
                    } else {
                        Err(error("reservation_lost"))
                    }
                })
                .await;
            if succeeds {
                assert_eq!(result.unwrap(), "attempt");
            } else {
                assert_eq!(result.unwrap_err().code(), "reservation_lost");
            }
            let cleanup = ResourceCleanup::pending(state);
            cleanup.finish().await.unwrap();
            assert!(cleanup.is_finished());
            assert_eq!(probe.calls.load(Ordering::SeqCst), usize::from(!succeeds));
        }
    }

    #[tokio::test]
    async fn abandoned_opened_resource_is_released_by_its_owner() {
        let probe = Arc::new(Probe::default());
        let (operation, state) = opening(&probe);
        let opened = operation
            .finish_open(Ok(("attempt".into(), ())))
            .await
            .unwrap();
        drop(opened);
        let cleanup = ResourceCleanup::pending(state);
        cleanup.finish().await.unwrap();
        assert!(cleanup.is_finished());
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelled_closer_leaves_the_resource_owned_for_retry() {
        let probe = Arc::new(Probe {
            release: Some(tokio::sync::Notify::new()),
            ..Default::default()
        });
        let (operation, state) = opening(&probe);
        drop(operation);
        let cleanup = ResourceCleanup::pending(state);
        let task_cleanup = cleanup.clone();
        let close = tokio::spawn(async move { task_cleanup.finish().await });
        probe.started.notified().await;
        close.abort();
        assert!(close.await.unwrap_err().is_cancelled());
        assert!(!cleanup.is_finished());
        probe.release.as_ref().unwrap().notify_one();
        cleanup.finish().await.unwrap();
        assert!(cleanup.is_finished());
        assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
    }
}
