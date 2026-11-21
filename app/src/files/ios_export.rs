use std::io;

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSArray, NSURL};
use objc2_ui_kit::{UIDocumentPickerViewController, UIViewController};

use super::download_library::PickerFile;

define_class!(
    // SAFETY: NSObject supports subclassing. The Rust ivar only owns a file.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = PickerFile]
    #[name = "RSSHExportFileLease"]
    struct ExportFileLease;
);

pub(super) fn present(presenter: &UIViewController, file: PickerFile) -> io::Result<()> {
    let main_thread = MainThreadMarker::new()
        .ok_or_else(|| io::Error::other("iOS picker requires the main thread"))?;
    if presenter.presentedViewController().is_some() {
        return Err(io::Error::other("Another iOS picker is open"));
    }
    let url =
        NSURL::from_file_path(&file.path).ok_or_else(|| io::Error::other("Invalid export URL"))?;
    let picker = UIDocumentPickerViewController::initForExportingURLs_asCopy(
        UIDocumentPickerViewController::alloc(main_thread),
        &NSArray::from_retained_slice(&[url]),
        true,
    );
    let lease = ExportFileLease::alloc(main_thread).set_ivars(file);
    // SAFETY: NSObject's init initializes the allocated instance. UIKit owns
    // the presented picker, whose retained association drops the temporary
    // file when that specific picker is released (including cancellation).
    unsafe {
        let lease: Retained<ExportFileLease> = msg_send![super(lease), init];
        static LEASE_KEY: u8 = 0;
        objc2::ffi::objc_setAssociatedObject(
            Retained::as_ptr(&picker).cast_mut().cast(),
            (&raw const LEASE_KEY).cast(),
            Retained::as_ptr(&lease).cast_mut().cast(),
            objc2::ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
        );
    }
    presenter.presentViewController_animated_completion(&picker, true, None);
    Ok(())
}
