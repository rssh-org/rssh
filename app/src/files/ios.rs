use super::directory_required;
use crate::error::{AppError, AppResult};
use rssh_core::ssh::sftp::transfer::{DownloadDirectory, PathDirectory, SourceStamp};
use std::ffi::{c_char, c_void, CStr, CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::sync::oneshot;

type PickReply = extern "C" fn(*mut c_void, *const c_char, *const c_char, bool);
type OpenReply = extern "C" fn(*mut c_void, *const c_char, *mut c_void, *const c_char);

extern "C" {
    fn rssh_pick_directory(context: *mut c_void, reply: PickReply);
    fn rssh_access_directory(bookmark: *const c_char, context: *mut c_void, reply: OpenReply);
    fn rssh_release_directory(token: *mut c_void);
}

fn picker_error(error: *const c_char) -> AppError {
    let detail = unsafe { CStr::from_ptr(error) }.to_string_lossy();
    AppError::sftp(
        "sftp_directory_unsupported",
        serde_json::json!({"err": detail}),
    )
}

extern "C" fn picked(
    context: *mut c_void,
    bookmark: *const c_char,
    error: *const c_char,
    needs_directory: bool,
) {
    // One callback consumes the sender for each picker. Its receiver can be dropped.
    let reply =
        unsafe { Box::from_raw(context.cast::<oneshot::Sender<AppResult<Option<String>>>>()) };
    let result = if needs_directory {
        Err(directory_required())
    } else if !error.is_null() {
        Err(picker_error(error))
    } else if bookmark.is_null() {
        Ok(None)
    } else {
        Ok(Some(
            unsafe { CStr::from_ptr(bookmark) }
                .to_string_lossy()
                .into_owned(),
        ))
    };
    let _ = reply.send(result);
}

pub(super) async fn pick(_app: &tauri::AppHandle) -> AppResult<Option<String>> {
    let (reply, result) = oneshot::channel::<AppResult<Option<String>>>();
    unsafe {
        rssh_pick_directory(Box::into_raw(Box::new(reply)).cast(), picked);
    }
    result.await.map_err(|_| directory_required())?
}

// Objective-C's retained semaphore handle may be signalled on any thread.
struct DirectoryScope(usize);
impl Drop for DirectoryScope {
    fn drop(&mut self) {
        unsafe {
            rssh_release_directory(self.0 as *mut c_void);
        }
    }
}

type ScopeResult = AppResult<(PathBuf, DirectoryScope)>;
extern "C" fn accessed(
    context: *mut c_void,
    path: *const c_char,
    token: *mut c_void,
    error: *const c_char,
) {
    let reply = unsafe { Box::from_raw(context.cast::<oneshot::Sender<ScopeResult>>()) };
    let scope = (!token.is_null()).then(|| DirectoryScope(token as usize));
    let result = if !error.is_null() {
        Err(picker_error(error))
    } else if let Some(scope) = scope.filter(|_| !path.is_null()) {
        let bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
        Ok((PathBuf::from(OsStr::from_bytes(bytes)), scope))
    } else {
        Err(directory_required())
    };
    let _ = reply.send(result);
}

struct ScopedDirectory {
    directory: PathDirectory,
    _scope: DirectoryScope,
}

pub(super) async fn directory(
    _app: &tauri::AppHandle,
    root: &str,
    relative_parent: &str,
) -> AppResult<Box<dyn DownloadDirectory>> {
    let bookmark = CString::new(root).map_err(|_| directory_required())?;
    let (reply, received) = oneshot::channel::<ScopeResult>();
    unsafe {
        rssh_access_directory(
            bookmark.as_ptr(),
            Box::into_raw(Box::new(reply)).cast(),
            accessed,
        );
    }
    let (root, scope) = received.await.map_err(|_| directory_required())??;
    let mut path = tokio::fs::canonicalize(root).await?;
    for child in relative_parent.split('/').filter(|p| !p.is_empty()) {
        path.push(child);
        match tokio::fs::create_dir(&path).await {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            result => result?,
        }
        if !tokio::fs::symlink_metadata(&path).await?.is_dir() {
            return Err(directory_required());
        }
    }
    Ok(Box::new(ScopedDirectory {
        directory: PathDirectory::granted(path),
        _scope: scope,
    }))
}

#[async_trait::async_trait]
impl DownloadDirectory for ScopedDirectory {
    async fn create_part(&self, name: &str) -> AppResult<File> {
        self.directory.create_part(name).await
    }
    async fn open_part(&self, name: &str) -> AppResult<File> {
        self.directory.open_part(name).await
    }
    async fn target_stamp(&self, name: &str) -> AppResult<Option<SourceStamp>> {
        self.directory.target_stamp(name).await
    }
    async fn publish(&self, part: &str, name: &str) -> AppResult<()> {
        self.directory.publish(part, name).await
    }
    fn display_path(&self, name: &str) -> String {
        self.directory.display_path(name)
    }
}
