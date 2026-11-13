use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::net::{UnixListener, UnixStream};

use super::{validate_instance, BoxStream, LocalListener, LocalTransport};

pub(super) static TRANSPORT: UnixTransport = UnixTransport;
pub(super) struct UnixTransport;

fn effective_uid() -> u32 {
    // getuid/geteuid have no pointer arguments or preconditions.
    unsafe { libc::geteuid() }
}

fn runtime_dir() -> PathBuf {
    // A fixed short root avoids macOS's 104-byte sun_path limit regardless of
    // the user's data directory or TMPDIR length. Never share this directory
    // with another user or accept a pre-created symlink.
    PathBuf::from(format!("/tmp/rssh-{}", effective_uid()))
}

fn validate_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "MCP runtime directory must be a private directory owned by this user",
        ));
    }
    Ok(())
}

fn ensure_directory(path: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    // Do not repair permissions of an existing path: a broad directory may
    // already have allowed somebody else to replace its contents.
    validate_directory(path)
}

fn validate_path(endpoint: &str) -> io::Result<()> {
    let path = Path::new(endpoint);
    let instance_id = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("mcp-"))
        .and_then(|name| name.strip_suffix(".sock"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid MCP socket path"))?;
    TRANSPORT.validate_endpoint(endpoint, instance_id)
}

#[async_trait]
impl LocalTransport for UnixTransport {
    async fn bind(&self, endpoint: &str) -> io::Result<Box<dyn LocalListener>> {
        validate_path(endpoint)?;
        ensure_directory(&runtime_dir())?;
        // A failed bind never removes an existing socket, which may belong to
        // another live RSSH process. Per-instance UUIDs avoid stale-name reuse.
        let listener = UnixListener::bind(endpoint)?;
        let metadata = fs::symlink_metadata(endpoint)?;
        let listener = SocketListener {
            listener,
            path: endpoint.into(),
            identity: (metadata.dev(), metadata.ino()),
        };
        fs::set_permissions(endpoint, fs::Permissions::from_mode(0o600))?;
        Ok(Box::new(listener))
    }

    async fn connect(&self, endpoint: &str) -> io::Result<BoxStream> {
        validate_path(endpoint)?;
        validate_directory(&runtime_dir())?;
        let metadata = fs::symlink_metadata(endpoint)?;
        if !metadata.file_type().is_socket()
            || metadata.uid() != effective_uid()
            || metadata.mode() & 0o777 != 0o600
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "MCP endpoint must be a private socket owned by this user",
            ));
        }
        Ok(Box::new(UnixStream::connect(endpoint).await?))
    }

    fn endpoint(&self, instance_id: &str) -> io::Result<String> {
        validate_instance(instance_id)?;
        Ok(runtime_dir()
            .join(format!("mcp-{instance_id}.sock"))
            .to_string_lossy()
            .into_owned())
    }

    fn validate_endpoint(&self, endpoint: &str, instance_id: &str) -> io::Result<()> {
        if endpoint != self.endpoint(instance_id)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "MCP socket does not match its instance",
            ));
        }
        Ok(())
    }
}

struct SocketListener {
    listener: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
}

#[async_trait]
impl LocalListener for SocketListener {
    async fn accept(&mut self) -> io::Result<BoxStream> {
        Ok(Box::new(self.listener.accept().await?.0))
    }
}

impl Drop for SocketListener {
    fn drop(&mut self) {
        // If the pathname was removed/replaced, this old listener must not
        // unlink the replacement's socket on its way out.
        if let Ok(metadata) = fs::symlink_metadata(&self.path) {
            if metadata.file_type().is_socket() && (metadata.dev(), metadata.ino()) == self.identity
            {
                if let Err(error) = fs::remove_file(&self.path) {
                    if error.kind() != io::ErrorKind::NotFound {
                        log::warn!("MCP socket cleanup failed: {error}");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_validation_rejects_symlinks_and_broad_permissions() {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join("private");
        ensure_directory(&private).unwrap();
        assert_eq!(fs::metadata(&private).unwrap().mode() & 0o777, 0o700);
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&private, &link).unwrap();
        assert_eq!(
            ensure_directory(&link).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        fs::set_permissions(&private, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            ensure_directory(&private).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[tokio::test]
    async fn socket_is_short_private_and_only_its_own_generation_is_removed() {
        let id = uuid::Uuid::new_v4().to_string();
        let endpoint = TRANSPORT.endpoint(&id).unwrap();
        assert!(endpoint.len() < 104);
        let first = TRANSPORT.bind(&endpoint).await.unwrap();
        assert_eq!(fs::metadata(&endpoint).unwrap().mode() & 0o777, 0o600);
        assert!(TRANSPORT.bind(&endpoint).await.is_err());
        assert!(Path::new(&endpoint).exists());
        fs::remove_file(&endpoint).unwrap();
        let replacement = TRANSPORT.bind(&endpoint).await.unwrap();
        drop(first);
        assert!(Path::new(&endpoint).exists());
        drop(replacement);
        assert!(!Path::new(&endpoint).exists());
    }
}
