use super::*;
use crate::commands::{lifecycle, serial as commands};
use crate::resource::{Cleanup, CleanupFuture, CleanupHandle, PendingOperation};
use crate::state::{AppState, NativeServices, SessionOwner, SessionPhase};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use tokio::sync::Notify;

#[derive(Default)]
struct Probe {
    events: Mutex<Vec<String>>,
    closes: AtomicUsize,
    fail_close: AtomicBool,
    opened: Notify,
    closing: Notify,
    release_open: Option<Notify>,
    release_close: Option<Notify>,
}

#[derive(Clone)]
struct Connection(Arc<Probe>);

#[async_trait::async_trait]
impl SerialConnection for Connection {
    async fn write(&self, data: &[u8]) -> AppResult<()> {
        self.0
            .events
            .lock()
            .unwrap()
            .push(format!("write:{data:?}"));
        Ok(())
    }

    async fn set_dtr(&self, level: bool) -> AppResult<()> {
        self.0.events.lock().unwrap().push(format!("dtr:{level}"));
        Ok(())
    }

    async fn set_rts(&self, level: bool) -> AppResult<()> {
        self.0.events.lock().unwrap().push(format!("rts:{level}"));
        Ok(())
    }

    async fn send_break(&self) -> AppResult<()> {
        self.0.events.lock().unwrap().push("break".into());
        Ok(())
    }

    fn port_name(&self) -> &str {
        "injected-port"
    }

    fn cleanup(&self) -> Option<CleanupHandle> {
        Some(CleanupHandle::new(Arc::new(self.clone())))
    }
}

impl Cleanup for Connection {
    fn close(&self) -> CleanupFuture<'_> {
        Box::pin(async move {
            self.0.closes.fetch_add(1, Ordering::SeqCst);
            self.0.closing.notify_one();
            if self.0.fail_close.swap(false, Ordering::SeqCst) {
                return Err(AppError::other("close_failed", serde_json::json!({})));
            }
            if let Some(release) = &self.0.release_close {
                release.notified().await;
            }
            Ok(())
        })
    }
}

struct Backend {
    probe: Arc<Probe>,
    wait_for_cancel: bool,
}

#[async_trait::async_trait]
impl SerialBackend for Backend {
    async fn capabilities(&self) -> AppResult<SerialCapabilities> {
        Ok(SerialCapabilities {
            flow_control: true,
            xany: false,
            signals: true,
            baud_rates: vec![115200],
        })
    }

    async fn available_ports(&self) -> AppResult<Vec<String>> {
        Ok(vec!["injected-port".into()])
    }

    async fn open(
        &self,
        session_id: String,
        port: &str,
        config: SerialConfig,
        sink: SerialSink,
        operation: &PendingOperation,
    ) -> AppResult<(String, SerialHandle)> {
        assert_eq!(port, "injected-port");
        self.capabilities().await?.validate(&config)?;
        let connection = Connection(self.probe.clone());
        operation.retain_cleanup(connection.cleanup().unwrap())?;
        self.probe.opened.notify_one();
        if let Some(release) = &self.probe.release_open {
            release.notified().await;
        }
        if self.wait_for_cancel {
            operation.cancelled().await;
            return Err(AppError::not_found(
                "session_reservation_lost",
                serde_json::json!({ "id": session_id }),
            ));
        }
        sink(&session_id, SerialOut::Data(vec![1, 2, 3]));
        Ok((session_id, SerialHandle::new(connection)))
    }
}

fn host(probe: Arc<Probe>, wait_for_cancel: bool) -> (tempfile::TempDir, crate::emitter::Host) {
    let directory = tempfile::tempdir().unwrap();
    let state = AppState::open_with_services(
        directory.path().to_owned(),
        NativeServices {
            keyring: None,
            serial: Arc::new(Backend {
                probe,
                wait_for_cancel,
            }),
        },
    )
    .unwrap();
    (
        directory,
        crate::emitter::Host::new(Arc::new(state), Arc::new(|_, _| true)),
    )
}

fn config() -> SerialConfig {
    serde_json::from_value(serde_json::json!({ "baud_rate": 115200 })).unwrap()
}

#[tokio::test]
async fn injected_serial_uses_shared_commands_and_owner_scoped_cleanup() {
    let probe = Arc::new(Probe::default());
    let (_directory, host) = host(probe.clone(), false);
    let owner = SessionOwner::Window("one".into());
    assert_eq!(
        commands::serial_list_ports(host.state()).await.unwrap(),
        vec!["injected-port"]
    );
    assert_eq!(
        commands::serial_get_capabilities(host.state())
            .await
            .unwrap()
            .baud_rates,
        vec![115200]
    );
    let id = commands::serial_open(
        host.clone(),
        owner.clone(),
        host.state(),
        "injected-port".into(),
        config(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        host.state().serial_sessions.lock().unwrap()[&id].port_name(),
        "injected-port"
    );
    commands::serial_write(host.state(), id.clone(), vec![4, 5])
        .await
        .unwrap();
    commands::serial_set_dtr(host.state(), id.clone(), true)
        .await
        .unwrap();
    commands::serial_set_rts(host.state(), id.clone(), false)
        .await
        .unwrap();
    commands::serial_send_break(host.state(), id.clone())
        .await
        .unwrap();
    assert_eq!(
        *probe.events.lock().unwrap(),
        vec!["write:[4, 5]", "dtr:true", "rts:false", "break"]
    );
    assert!(commands::serial_close(
        SessionOwner::Window("other".into()),
        host.state(),
        id.clone()
    )
    .await
    .is_err());
    assert_eq!(probe.closes.load(Ordering::SeqCst), 0);
    commands::serial_close(owner, host.state(), id.clone())
        .await
        .unwrap();
    assert_eq!(probe.closes.load(Ordering::SeqCst), 1);
    assert!(host.state().serial_sessions.lock().unwrap().is_empty());
    assert_eq!(
        host.state().lifecycle_sessions.lock().unwrap()[&id].phase(),
        SessionPhase::Closed
    );
}

#[tokio::test]
async fn injected_serial_failed_close_remains_owned_for_retry() {
    let probe = Arc::new(Probe::default());
    probe.fail_close.store(true, Ordering::SeqCst);
    let (_directory, host) = host(probe.clone(), false);
    let owner = SessionOwner::Window("one".into());
    let id = commands::serial_open(
        host.clone(),
        owner.clone(),
        host.state(),
        "injected-port".into(),
        config(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        commands::serial_close(owner.clone(), host.state(), id.clone())
            .await
            .unwrap_err()
            .code(),
        "close_failed"
    );
    assert_eq!(
        host.state().lifecycle_sessions.lock().unwrap()[&id].phase(),
        SessionPhase::Closing
    );
    commands::serial_close(owner, host.state(), id.clone())
        .await
        .unwrap();
    assert_eq!(probe.closes.load(Ordering::SeqCst), 2);
    assert_eq!(
        host.state().lifecycle_sessions.lock().unwrap()[&id].phase(),
        SessionPhase::Closed
    );
}

#[tokio::test]
async fn injected_serial_cancelled_open_waits_for_native_cleanup_acknowledgement() {
    let probe = Arc::new(Probe {
        release_close: Some(Notify::new()),
        ..Default::default()
    });
    let (_directory, host) = host(probe.clone(), true);
    let owner = SessionOwner::Window("one".into());
    let id = uuid::Uuid::new_v4().to_string();
    let opening = {
        let host = host.clone();
        let owner = owner.clone();
        let id = id.clone();
        tokio::spawn(async move {
            commands::serial_open(
                host.clone(),
                owner,
                host.state(),
                "injected-port".into(),
                config(),
                Some(id),
            )
            .await
        })
    };
    probe.opened.notified().await;
    let closing = {
        let host = host.clone();
        let id = id.clone();
        tokio::spawn(async move {
            lifecycle::close_resource_and_wait(
                host.state(),
                &id,
                crate::state::SessionKind::Serial,
                &owner,
            )
            .await
        })
    };
    probe.closing.notified().await;
    assert!(!opening.is_finished());
    assert!(!closing.is_finished());
    assert_eq!(
        host.state().lifecycle_sessions.lock().unwrap()[&id].phase(),
        SessionPhase::Closing
    );
    probe.release_close.as_ref().unwrap().notify_one();
    assert_eq!(
        opening.await.unwrap().unwrap_err().code(),
        "session_reservation_lost"
    );
    closing.await.unwrap().unwrap();
    assert_eq!(probe.closes.load(Ordering::SeqCst), 1);
    assert_eq!(
        host.state().lifecycle_sessions.lock().unwrap()[&id].phase(),
        SessionPhase::Closed
    );
}

#[tokio::test]
async fn injected_serial_aborted_caller_cannot_abandon_a_native_open() {
    let probe = Arc::new(Probe {
        release_open: Some(Notify::new()),
        ..Default::default()
    });
    let (_directory, host) = host(probe.clone(), false);
    let owner = SessionOwner::Window("one".into());
    let id = uuid::Uuid::new_v4().to_string();
    let opening = {
        let host = host.clone();
        let owner = owner.clone();
        let id = id.clone();
        tokio::spawn(async move {
            commands::serial_open(
                host.clone(),
                owner,
                host.state(),
                "injected-port".into(),
                config(),
                Some(id),
            )
            .await
        })
    };
    probe.opened.notified().await;
    opening.abort();
    assert!(opening.await.unwrap_err().is_cancelled());
    assert_eq!(
        host.state().lifecycle_sessions.lock().unwrap()[&id].phase(),
        SessionPhase::Closing
    );
    assert_eq!(probe.closes.load(Ordering::SeqCst), 0);
    probe.release_open.as_ref().unwrap().notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if host.state().lifecycle_sessions.lock().unwrap()[&id].phase() == SessionPhase::Closed
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the registry must retain and release the late native handle");
    assert_eq!(probe.closes.load(Ordering::SeqCst), 1);
    assert!(host.state().serial_sessions.lock().unwrap().is_empty());
}
