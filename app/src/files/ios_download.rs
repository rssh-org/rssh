//! The directory bookmark stays in the originating webview's resource table.
//! Each attempt uses the provider's coordinated URL for the entire transfer.
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::Arc;

use block2::StackBlock;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{Bool, NSObject, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadOnly};
use objc2_foundation::{
    NSArray, NSData, NSFileCoordinator, NSFileCoordinatorWritingOptions, NSObjectProtocol,
    NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions, NSURL,
};
use objc2_ui_kit::{UIDocumentPickerDelegate, UIDocumentPickerViewController, UIViewController};
use objc2_uniform_type_identifiers::UTTypeFolder;
use rssh_core::commands::sftp::transfer::Attempt;
use rssh_core::emitter::Host;
use rssh_core::error::{AppError, AppResult};
use rssh_core::files::atomic::{
    discard_owned_file, normalized_destination, DownloadDirectory, FileId, NativeDownloadDirectory,
};
use serde_json::json;
use tauri::{Manager, WebviewWindow};
use tokio::sync::oneshot;

fn unsupported() -> AppError {
    AppError::sftp("sftp_folder_operations_unsupported", json!({}))
}

struct Scope<'a>(&'a NSURL);

impl<'a> Scope<'a> {
    fn start(url: &'a NSURL) -> AppResult<Self> {
        if !url.isFileURL() {
            return Err(unsupported());
        }
        // The caller obtained this URL from the picker or its scoped bookmark.
        unsafe { url.startAccessingSecurityScopedResource() }
            .then_some(())
            .ok_or_else(super::download_siblings_unavailable)?;
        Ok(Self(url))
    }
}

impl Drop for Scope<'_> {
    fn drop(&mut self) {
        // Scope is only constructed after a successful start on this exact URL.
        unsafe { self.0.stopAccessingSecurityScopedResource() };
    }
}

pub(crate) struct FolderGrant {
    bookmark: Vec<u8>,
    destination: PathBuf,
}

// The bookmark contains the user's directory permission.
impl std::fmt::Debug for FolderGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FolderGrant")
            .field("destination", &self.destination)
            .finish_non_exhaustive()
    }
}

impl tauri::Resource for FolderGrant {}

impl FolderGrant {
    fn new(url: &NSURL, name: &str) -> AppResult<Self> {
        let _scope = Scope::start(url)?;
        let directory = url
            .to_file_path()
            .filter(|p| p.is_absolute())
            .ok_or_else(unsupported)?;
        let bookmark = url
            .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
                NSURLBookmarkCreationOptions::MinimalBookmark,
                None,
                None,
            )
            .map_err(|_| super::download_siblings_unavailable())?;
        Ok(Self {
            bookmark: bookmark.to_vec(),
            destination: directory.join(name),
        })
    }

    fn coordinated<T>(&self, operation: impl FnOnce(&Path) -> AppResult<T>) -> AppResult<T> {
        let data = NSData::with_bytes(&self.bookmark);
        let mut stale = Bool::NO;
        // Foundation writes one BOOL synchronously into the supplied out parameter.
        let directory = unsafe {
            NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
                &data,
                NSURLBookmarkResolutionOptions::WithoutUI,
                None,
                &mut stale,
            )
        }
        .map_err(|_| super::download_siblings_unavailable())?;
        if stale.as_bool() {
            return Err(super::download_siblings_unavailable());
        }
        let _scope = Scope::start(&directory)?;
        let resolved = directory.to_file_path().ok_or_else(unsupported)?;
        let selected = self.destination.parent().ok_or_else(unsupported)?;
        coordinate_write(&directory, |coordinated| {
            // Bookmarks may normalize /var to /private/var. Reject a moved
            // destination while accepting aliases for the selected directory.
            if resolved != selected
                && std::fs::canonicalize(&resolved)? != std::fs::canonicalize(selected)?
            {
                return Err(super::download_siblings_unavailable());
            }
            operation(coordinated)
        })
    }
}

fn coordinate_write<T>(
    directory: &NSURL,
    operation: impl FnOnce(&Path) -> AppResult<T>,
) -> AppResult<T> {
    let coordinator = NSFileCoordinator::initWithFilePresenter(NSFileCoordinator::alloc(), None);
    let result = RefCell::new(None);
    let operation = Cell::new(Some(operation));
    let accessor = StackBlock::new(|url: NonNull<NSURL>| {
        // Foundation lends the accessor its coordinated URL until it returns.
        if let Some(operation) = operation.take() {
            let path = unsafe { url.as_ref() }.to_file_path();
            result.replace(Some(
                path.ok_or_else(unsupported).and_then(|p| operation(&p)),
            ));
        }
    });
    let mut error = None;
    coordinator.coordinateWritingItemAtURL_options_error_byAccessor(
        directory,
        NSFileCoordinatorWritingOptions::empty(),
        Some(&mut error),
        &accessor,
    );
    // The accessor can already have committed a replacement. Preserve its
    // actual outcome rather than inviting a retry after publication.
    result.into_inner().unwrap_or_else(|| match error {
        Some(error) => Err(AppError::sftp(
            "sftp_io_failed",
            json!({
                "op": "coordinate", "err": error.localizedDescription().to_string()
            }),
        )),
        None => Err(unsupported()),
    })
}

impl DownloadDirectory for FolderGrant {
    fn discard(self: Arc<Self>, path: PathBuf, id: FileId) {
        // A checkpoint can outlive the webview. Its last owner reacquires
        // access and checks identity before removing this transfer's part.
        tauri::async_runtime::spawn_blocking(move || {
            autoreleasepool(|_| {
                let result = self.coordinated(|directory| {
                    if path.parent() == Some(std::fs::canonicalize(directory)?.as_path()) {
                        discard_owned_file(&NativeDownloadDirectory, &path, &id)?;
                    }
                    Ok(())
                });
                if let Err(error) = result {
                    log::warn!(
                        "could not clean up SFTP partial at {}: {error}",
                        path.display()
                    );
                }
            })
        });
    }
}

type PickerResult = AppResult<Option<FolderGrant>>;
struct PickerState {
    name: String,
    done: RefCell<Option<oneshot::Sender<PickerResult>>>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[name = "RSSHSFTPFolderPickerDelegate"]
    #[thread_kind = MainThreadOnly]
    #[ivars = PickerState]
    struct PickerDelegate;
    unsafe impl NSObjectProtocol for PickerDelegate {}
    unsafe impl UIDocumentPickerDelegate for PickerDelegate {
        #[unsafe(method(documentPicker:didPickDocumentsAtURLs:))]
        fn picked(&self, _: &UIDocumentPickerViewController, urls: &NSArray<NSURL>) {
            if let Some(done) = self.ivars().done.take() {
                let result = urls
                    .firstObject()
                    .ok_or_else(unsupported)
                    .and_then(|url| FolderGrant::new(&url, &self.ivars().name).map(Some));
                let _ = done.send(result);
            }
        }
        #[unsafe(method(documentPickerWasCancelled:))]
        fn cancelled(&self, _: &UIDocumentPickerViewController) {
            if let Some(done) = self.ivars().done.take() {
                let _ = done.send(Ok(None));
            }
        }
    }
);

fn present(presenter: &UIViewController, name: String, done: oneshot::Sender<PickerResult>) {
    if presenter.presentedViewController().is_some()
        || presenter
            .viewIfLoaded()
            .and_then(|view| view.window())
            .is_none()
    {
        let _ = done.send(Err(AppError::sftp("sftp_folder_picker_busy", json!({}))));
        return;
    }
    let mtm = presenter.mtm();
    // UTTypeFolder and in-place opening are available at the app's iOS 14 minimum.
    let types = NSArray::from_slice(&[unsafe { UTTypeFolder }]);
    let picker = UIDocumentPickerViewController::initForOpeningContentTypes_asCopy(
        UIDocumentPickerViewController::alloc(mtm),
        &types,
        false,
    );
    let delegate = PickerDelegate::alloc(mtm).set_ivars(PickerState {
        name,
        done: RefCell::new(Some(done)),
    });
    let delegate: Retained<PickerDelegate> = unsafe { msg_send![super(delegate), init] };
    // UIKit's delegate is weak. Retain it on this picker, including when the
    // presenting window is destroyed without delivering a selection callback.
    static DELEGATE_KEY: u8 = 0;
    unsafe {
        objc2::ffi::objc_setAssociatedObject(
            Retained::as_ptr(&picker).cast_mut().cast(),
            (&raw const DELEGATE_KEY).cast(),
            Retained::as_ptr(&delegate).cast_mut().cast(),
            objc2::ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
        );
    }
    picker.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // The system Cancel button reports cancellation through the delegate.
    picker.setModalInPresentation(true);
    presenter.presentViewController_animated_completion(&picker, true, None);
}

pub(crate) async fn pick(
    window: &WebviewWindow,
    name: String,
) -> AppResult<Option<super::PickedDownload>> {
    if !super::valid_display_name(&name) {
        return Err(AppError::other("file_path_invalid", json!({"err": name})));
    }
    let (done, result) = oneshot::channel();
    window
        .with_webview(move |webview| {
            // Tauri runs this callback on the UI thread and owns the controller.
            let presenter = unsafe {
                webview
                    .view_controller()
                    .cast::<UIViewController>()
                    .as_ref()
            };
            if let Some(presenter) = presenter {
                present(presenter, name, done);
            }
        })
        .map_err(std::io::Error::other)?;
    let Some(grant) = result.await.map_err(std::io::Error::other)?? else {
        return Ok(None);
    };
    let location = grant
        .destination
        .to_str()
        .ok_or_else(unsupported)?
        .to_owned();
    let grant_id = window.resources_table().add(grant);
    Ok(Some(super::PickedDownload::Folder { location, grant_id }))
}

pub(crate) async fn download(
    window: &WebviewWindow,
    grant_id: u32,
    location: &str,
    attempt: Attempt,
    host: Host,
) -> AppResult<u64> {
    let grant = window
        .resources_table()
        .get::<FolderGrant>(grant_id)
        .map_err(|_| super::download_siblings_unavailable())?;
    if grant.destination != Path::new(location) {
        return Err(super::download_siblings_unavailable());
    }
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        autoreleasepool(|_| {
            grant.coordinated(|directory| {
                let name = grant.destination.file_name().ok_or_else(unsupported)?;
                let target = normalized_destination(&directory.join(name))?;
                runtime.block_on(attempt.download_in(&host, target, grant.clone()))
            })
        })
    })
    .await
    .map_err(std::io::Error::other)?
}
