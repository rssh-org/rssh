//! Storage operations used by the shared resumable transfer engine.
//! A location is opaque: document providers resolve it within a selected tree.

use crate::error::{AppError, AppResult};
use serde_json::json;
use std::path::Path;
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite};

#[async_trait::async_trait]
pub trait TransferStream: AsyncRead + AsyncWrite + AsyncSeek + Unpin + Send {
    /// Wait for outstanding work and close the actual descriptor before rename.
    async fn close(self: Box<Self>) -> AppResult<()>;
}

#[async_trait::async_trait]
impl TransferStream for tokio::fs::File {
    async fn close(mut self: Box<Self>) -> AppResult<()> {
        use tokio::io::AsyncWriteExt;
        let result = self.flush().await;
        drop(self.into_std().await);
        result.map_err(Into::into)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    pub size: u64,
    /// Nanoseconds since the Unix epoch, when supplied by the storage backend.
    pub modified: Option<u64>,
    pub stable_id: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub enum TransferMode {
    Read,
    ReadWrite,
    CreateNew,
}

#[derive(Debug)]
pub enum RenameError {
    /// The backend proved a destination-name conflict. Permission, transport,
    /// and ambiguous provider errors must remain Other.
    Conflict(AppError),
    Other(AppError),
}

impl RenameError {
    pub fn into_error(self) -> AppError {
        match self {
            Self::Conflict(error) | Self::Other(error) => error,
        }
    }
}

#[async_trait::async_trait]
pub trait TransferEndpoint: Send + Sync {
    /// Resolve a read-only source without changing the receiver's strict
    /// no-symlink rules. Provider locations remain opaque by default.
    async fn resolve_source(&self, path: &str) -> AppResult<String> {
        Ok(path.to_owned())
    }
    async fn canonicalize_target(&self, path: &str) -> AppResult<String>;
    async fn sibling(&self, path: &str, name: &str) -> AppResult<String>;
    async fn stat(&self, path: &str) -> AppResult<Option<FileIdentity>>;
    async fn open(&self, path: &str, mode: TransferMode) -> AppResult<Box<dyn TransferStream>>;
    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<(), RenameError>;
    async fn remove(&self, path: &str) -> AppResult<()>;
}

#[derive(Default)]
pub struct NativeEndpoint;

fn invalid_file(path: &str) -> AppError {
    AppError::sftp("sftp_transfer_invalid_file", json!({ "path": path }))
}

#[async_trait::async_trait]
impl TransferEndpoint for NativeEndpoint {
    async fn resolve_source(&self, path: &str) -> AppResult<String> {
        Ok(tokio::fs::canonicalize(path)
            .await?
            .to_string_lossy()
            .into_owned())
    }

    async fn canonicalize_target(&self, path: &str) -> AppResult<String> {
        let path = Path::new(path);
        let name = path
            .file_name()
            .ok_or_else(|| invalid_file(&path.to_string_lossy()))?;
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = tokio::fs::canonicalize(parent).await?;
        Ok(parent.join(name).to_string_lossy().into_owned())
    }

    async fn sibling(&self, path: &str, name: &str) -> AppResult<String> {
        if name.is_empty() || name.contains(['/', '\\', '\0']) || matches!(name, "." | "..") {
            return Err(invalid_file(name));
        }
        Ok(Path::new(path)
            .with_file_name(name)
            .to_string_lossy()
            .into_owned())
    }

    async fn stat(&self, path: &str) -> AppResult<Option<FileIdentity>> {
        let meta = match tokio::fs::symlink_metadata(path).await {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(invalid_file(path));
        }
        #[cfg(any(linux, macos, android, ios, ohos))]
        let stable_id = {
            use std::os::unix::fs::MetadataExt;
            Some(format!("{}:{}", meta.dev(), meta.ino()))
        };
        #[cfg(windows)]
        let stable_id = None;
        Ok(Some(FileIdentity {
            size: meta.len(),
            modified: meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|time| time.as_nanos().try_into().ok()),
            stable_id,
        }))
    }

    async fn open(&self, path: &str, mode: TransferMode) -> AppResult<Box<dyn TransferStream>> {
        let mut options = tokio::fs::OpenOptions::new();
        options.read(true);
        match mode {
            TransferMode::Read => {}
            TransferMode::ReadWrite => {
                options.write(true);
            }
            TransferMode::CreateNew => {
                options.write(true).create_new(true);
            }
        }
        #[cfg(any(linux, macos, android, ios, ohos))]
        options.custom_flags(libc::O_NOFOLLOW);
        let file = options.open(path).await?;
        if !file.metadata().await?.is_file() {
            return Err(invalid_file(path));
        }
        Ok(Box::new(file))
    }

    async fn rename(&self, from: &str, to: &str, replace: bool) -> Result<(), RenameError> {
        let result = if replace {
            tokio::fs::rename(from, to).await
        } else {
            let (from, to) = (from.to_owned(), to.to_owned());
            tokio::task::spawn_blocking(move || rename_exclusive(Path::new(&from), Path::new(&to)))
                .await
                .map_err(|error| {
                    RenameError::Other(AppError::other(
                        "io_error",
                        json!({"err": error.to_string()}),
                    ))
                })?
        };
        result.map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                RenameError::Conflict(error.into())
            } else {
                RenameError::Other(error.into())
            }
        })
    }

    async fn remove(&self, path: &str) -> AppResult<()> {
        tokio::fs::remove_file(path).await.map_err(Into::into)
    }
}

/// An existence check followed by rename races with other processes. Use the
/// native no-replace operation; unsupported filesystems fail without mutation.
fn rename_exclusive(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(any(linux, macos, android, ios, ohos))]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
        #[cfg(any(macos, ios))]
        let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(any(linux, android, ohos))]
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                1u32,
            ) as libc::c_int
        };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
        }
        let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

    #[tokio::test]
    async fn exclusive_creation_and_rename_preserve_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("target.bak").to_string_lossy().into_owned();
        let part = dir
            .path()
            .join("target.part")
            .to_string_lossy()
            .into_owned();
        std::fs::write(&original, b"old").unwrap();
        let endpoint = NativeEndpoint;
        let mut stream = endpoint.open(&part, TransferMode::CreateNew).await.unwrap();
        stream.write_all(b"new").await.unwrap();
        stream.shutdown().await.unwrap();
        drop(stream);
        assert!(endpoint.open(&part, TransferMode::CreateNew).await.is_err());
        assert!(matches!(
            endpoint.rename(&part, &original, false).await,
            Err(RenameError::Conflict(_))
        ));
        assert_eq!(std::fs::read(&original).unwrap(), b"old");
        assert_eq!(std::fs::read(&part).unwrap(), b"new");
        endpoint.rename(&part, &original, true).await.unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), b"new");
    }

    #[tokio::test]
    async fn reopened_stream_supports_prefix_readback_and_exact_resume_offset() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("owned.part").to_string_lossy().into_owned();
        std::fs::write(&part, b"prefix").unwrap();
        let mut stream = NativeEndpoint
            .open(&part, TransferMode::ReadWrite)
            .await
            .unwrap();
        let mut prefix = [0; 6];
        stream.read_exact(&mut prefix).await.unwrap();
        assert_eq!(&prefix, b"prefix");
        stream.seek(std::io::SeekFrom::Start(6)).await.unwrap();
        stream.write_all(b"tail").await.unwrap();
        stream.shutdown().await.unwrap();
        drop(stream);
        assert_eq!(std::fs::read(&part).unwrap(), b"prefixtail");
    }
}
