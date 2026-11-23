//! Native iOS scoped file/folder picker. Folder requests pass UTType.folder
//! directly; MIME filters cannot reliably express a directory.

use super::*;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_foundation::{MainThreadMarker, NSArray, NSObject, NSObjectProtocol};
use objc2_ui_kit::{UIDocumentPickerDelegate, UIDocumentPickerViewController, UIViewController};
use objc2_uniform_type_identifiers::UTType;

use tokio::sync::oneshot;
type PickResult = AppResult<Option<Vec<Retained<NSURL>>>>;

struct PickerIvars {
    sender: RefCell<Option<oneshot::Sender<PickResult>>>,
}

define_class!(
    // NSObject imposes no subclass-specific invariants. UIKit owns all calls
    // to this delegate on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = PickerIvars]
    struct ScopedPickerDelegate;

    unsafe impl NSObjectProtocol for ScopedPickerDelegate {}
    unsafe impl UIDocumentPickerDelegate for ScopedPickerDelegate {
        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn selected(&self, _controller: &UIDocumentPickerViewController, urls: &NSArray<NSURL>) {
            if let Some(sender) = self.ivars().sender.borrow_mut().take() {
                let _ = sender.send(Ok(Some(
                    (0..urls.count())
                        .map(|index| urls.objectAtIndex(index))
                        .collect(),
                )));
            }
        }
        #[unsafe(method(documentPickerWasCancelled:))]
        fn cancelled(&self, _controller: &UIDocumentPickerViewController) {
            if let Some(sender) = self.ivars().sender.borrow_mut().take() {
                let _ = sender.send(Ok(None));
            }
        }
    }
);

impl ScopedPickerDelegate {
    fn new(mtm: MainThreadMarker, sender: oneshot::Sender<PickResult>) -> Retained<Self> {
        let instance = Self::alloc(mtm).set_ivars(PickerIvars {
            sender: RefCell::new(Some(sender)),
        });
        // NSObject.init has no additional preconditions.
        unsafe { msg_send![super(instance), init] }
    }
}

thread_local! {
    // UIDocumentPicker retains its controller but its delegate is weak. Keep
    // one delegate until the next picker; after completion it owns no grant.
    static DELEGATE: RefCell<Option<Retained<ScopedPickerDelegate>>> = const { RefCell::new(None) };
}

pub(super) async fn pick(app: &AppHandle, directory: bool, multiple: bool) -> PickResult {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| provider_error("main window is unavailable"))?;
    let (sender, receiver) = oneshot::channel();
    window
        .with_webview(move |webview| {
            let Some(mtm) = MainThreadMarker::new() else {
                let _ = sender.send(Err(provider_error(
                    "folder picker was not dispatched on the main thread",
                )));
                return;
            };
            DELEGATE.with(|slot| {
                if slot
                    .borrow()
                    .as_ref()
                    .is_some_and(|delegate| delegate.ivars().sender.borrow().is_some())
                {
                    let _ = sender.send(Err(provider_error("a folder picker is already open")));
                    return;
                }
                // Tauri returns its live UIViewController inside with_webview.
                let Some(controller) =
                    (unsafe { (webview.view_controller() as *mut UIViewController).as_ref() })
                else {
                    let _ = sender.send(Err(provider_error("view controller is unavailable")));
                    return;
                };
                if controller.presentedViewController().is_some() {
                    let _ = sender.send(Err(provider_error("another native dialog is open")));
                    return;
                }
                let Some(content_type) =
                    UTType::typeWithIdentifier(&NSString::from_str(if directory {
                        "public.folder"
                    } else {
                        "public.item"
                    }))
                else {
                    let _ = sender.send(Err(provider_error("folder selection is unavailable")));
                    return;
                };
                let delegate = ScopedPickerDelegate::new(mtm, sender);
                let picker = UIDocumentPickerViewController::initForOpeningContentTypes_asCopy(
                    UIDocumentPickerViewController::alloc(mtm),
                    &NSArray::from_retained_slice(&[content_type]),
                    false,
                );
                picker.setAllowsMultipleSelection(multiple);
                picker.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
                *slot.borrow_mut() = Some(delegate);
                controller.presentViewController_animated_completion(&picker, true, None);
            });
        })
        .map_err(provider_error)?;
    receiver.await.map_err(provider_error)?
}
