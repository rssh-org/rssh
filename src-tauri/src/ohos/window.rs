//! ArkUI window creation and rollback. Shared callers see one completed operation.

use crate::platform::window::AppWindowPurpose;

fn check_window_capabilities(
    desktop_device: bool,
    multi_window: bool,
    local_pty: bool,
    purpose: AppWindowPurpose,
) -> Result<(), String> {
    if !desktop_device || !multi_window {
        return Err("Additional windows are unavailable on this device".into());
    }
    if matches!(purpose, AppWindowPurpose::LocalAnalysis) && !local_pty {
        return Err("Local analysis requires a working local terminal".into());
    }
    Ok(())
}

#[cfg(ohos)]
pub(crate) async fn ensure_available(purpose: AppWindowPurpose) -> Result<(), String> {
    let device = crate::ohos::device::query().await.map_err(|error| {
        log::warn!("Cannot query native window capabilities: {error}");
        "HarmonyOS system services are unavailable; restart RSSH and try again".to_string()
    })?;
    let desktop_device = device.class() == crate::ohos::device::DeviceClass::Desktop;
    let local_pty = desktop_device
        && device.multi_window
        && matches!(purpose, AppWindowPurpose::LocalAnalysis)
        && crate::ohos::device::local_pty_available().await;
    check_window_capabilities(desktop_device, device.multi_window, local_pty, purpose)
}

#[cfg(ohos)]
pub(crate) async fn confirm_creation(window: tauri::WebviewWindow) -> Result<(), String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let registered = window.with_webview(move |native| {
        native.on_created(move |result| {
            let _ = sender.send(result);
        });
    });
    confirm_native_creation(
        registered.map_err(|error| error.to_string()),
        receiver,
        move || {
            // build() reserves the logical Tauri window before ArkUI/ArkWeb finish.
            // Failure or cancellation must remove that reservation and its surface.
            if let Err(error) = window.destroy() {
                log::error!("Failed to remove an unsuccessful native window: {error}");
            }
        },
    )
    .await
}

struct PendingWindow<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Drop for PendingWindow<F> {
    fn drop(&mut self) {
        if let Some(rollback) = self.0.take() {
            rollback();
        }
    }
}

async fn confirm_native_creation(
    registered: Result<(), String>,
    created: tokio::sync::oneshot::Receiver<Result<(), String>>,
    rollback: impl FnOnce(),
) -> Result<(), String> {
    let mut pending = PendingWindow(Some(rollback));
    registered?;
    created
        .await
        .map_err(|_| "Window closed before native creation completed".to_string())??;
    pending.0.take();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_creation_waits_and_rolls_back_native_failure() {
        use std::{
            cell::Cell,
            future::Future,
            task::{Context, Poll, Waker},
        };
        let closed = Cell::new(0);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut pending = Box::pin(confirm_native_creation(Ok(()), receiver, || {
            closed.set(closed.get() + 1)
        }));
        let mut context = Context::from_waker(Waker::noop());
        assert!(pending.as_mut().poll(&mut context).is_pending());
        assert_eq!(closed.get(), 0);
        sender.send(Err("OS window limit".into())).unwrap();
        assert_eq!(
            pending.as_mut().poll(&mut context),
            Poll::Ready(Err("OS window limit".into()))
        );
        drop(pending);
        assert_eq!(closed.get(), 1);
    }

    #[test]
    fn cancelled_native_creation_removes_its_logical_reservation() {
        use std::{
            cell::Cell,
            future::Future,
            task::{Context, Waker},
        };
        let closed = Cell::new(false);
        let (_sender, receiver) = tokio::sync::oneshot::channel();
        let mut pending = Box::pin(confirm_native_creation(Ok(()), receiver, || {
            closed.set(true)
        }));
        assert!(pending
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending());
        drop(pending);
        assert!(closed.get());
    }

    #[tokio::test]
    async fn successful_native_creation_transfers_ownership_without_closing() {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        sender.send(Ok(())).unwrap();
        assert!(confirm_native_creation(Ok(()), receiver, || panic!(
            "ready window must stay open"
        ))
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn missing_native_callback_and_registration_failure_both_roll_back() {
        use std::cell::Cell;
        for registered in [Ok(()), Err("dispatcher closed".into())] {
            let closed = Cell::new(false);
            let (sender, receiver) = tokio::sync::oneshot::channel();
            drop(sender);
            assert!(
                confirm_native_creation(registered, receiver, || closed.set(true))
                    .await
                    .is_err()
            );
            assert!(closed.get());
        }
    }

    #[test]
    fn phone_tablet_and_unavailable_window_service_reject_both_entry_points() {
        for (desktop, multi_window) in [(false, false), (false, true), (true, false)] {
            for purpose in [AppWindowPurpose::Tab, AppWindowPurpose::LocalAnalysis] {
                assert_eq!(
                    check_window_capabilities(desktop, multi_window, true, purpose),
                    Err("Additional windows are unavailable on this device".into()),
                );
            }
        }
    }

    #[test]
    fn pty_restrictions_block_analysis_but_not_remote_tab_windows() {
        assert!(check_window_capabilities(true, true, false, AppWindowPurpose::Tab).is_ok());
        assert_eq!(
            check_window_capabilities(true, true, false, AppWindowPurpose::LocalAnalysis),
            Err("Local analysis requires a working local terminal".into()),
        );
        assert!(
            check_window_capabilities(true, true, true, AppWindowPurpose::LocalAnalysis).is_ok()
        );
    }
}
