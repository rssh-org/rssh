pub mod transfer;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{AppError, AppResult};
use crate::models::Credential;
use crate::ssh::client;

/// i18n code returned when the user cancels a transfer. The frontend
/// (transfers.svelte.ts) matches this literal via `errStr.includes(...)` to
/// flip the status to "cancelled". Keep the constant in sync across both ends.
pub const CANCELLED_CODE: &str = "transfer_cancelled";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    /// unix epoch seconds; 0 means the server did not provide the mtime
    pub mtime: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileStat {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<u32>,
}

/// Flat walk output. `rel_path` is always '/'-separated (even when the host is
/// Windows); the frontend swaps separators when joining the local path.
#[derive(Debug, Clone, Serialize)]
pub struct WalkEntry {
    pub rel_path: String,
    pub size: u64,
}

/// Maximum recursion depth, counting the root as depth 0. With CAP = 32 the
/// walker accepts paths up to 32 segments deep (root through depth 31) and
/// fails the whole command at depth 32 — guards against symlink cycles and
/// pathological server-side trees.
const WALK_DEPTH_CAP: u32 = 32;

fn private_staging_options() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(any(macos, linux, android, ios, ohos))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// Owns an AI download's exclusively created `.part` path. Dropping the future at
/// any await point drops this guard after the open file, so cancellation cannot
/// leave a partial artifact behind.
struct PartialDownloadGuard {
    path: PathBuf,
    committed: bool,
}

impl PartialDownloadGuard {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            committed: false,
        }
    }

    fn commit(mut self, local_path: &Path) -> std::io::Result<()> {
        std::fs::rename(&self.path, local_path)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PartialDownloadGuard {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Join a remote path segment. Special-cases dir == "/" so the result never
/// contains "//foo"; matches the convention used by the existing callers.
fn join_remote(dir: &str, name: &str) -> String {
    if dir == "/" {
        format!("/{}", name)
    } else {
        format!("{}/{}", dir.trim_end_matches('/'), name)
    }
}

pub struct SftpHandle {
    pub(crate) peer: client::VerifiedPeer,
    sftp: russh_sftp::client::SftpSession,
    /// 父 SSH 会话 id；None 表示这条 SFTP 走的是独立 TCP（`SftpHandle::connect`）。
    /// SSH 关闭时会按这个字段反向找到所有 children 并清理。
    parent_ssh_id: Option<String>,
}

impl SftpHandle {
    pub fn parent_ssh_id(&self) -> Option<&str> {
        self.parent_ssh_id.as_deref()
    }

    /// Open SFTP subsystem on an existing SSH connection.
    pub async fn from_handle(
        ssh_handle: &crate::ssh::client::SshHandle,
        parent_ssh_id: String,
    ) -> AppResult<Self> {
        let channel = {
            let h = ssh_handle.lock().await;
            h.channel_open_session().await.map_err(|e| {
                AppError::sftp(
                    "sftp_io_failed",
                    json!({ "op": "open channel", "err": e.to_string() }),
                )
            })?
        };

        channel.request_subsystem(true, "sftp").await.map_err(|e| {
            AppError::sftp(
                "sftp_io_failed",
                json!({ "op": "request_subsystem", "err": e.to_string() }),
            )
        })?;

        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| AppError::sftp("sftp_init_failed", json!({ "err": e.to_string() })))?;

        Ok(Self {
            sftp,
            parent_ssh_id: Some(parent_ssh_id),
            peer: ssh_handle.peer.clone(),
        })
    }

    pub async fn connect(
        host: String,
        port: u16,
        credential: Credential,
        known_hosts_path: PathBuf,
        timeout_secs: u64,
    ) -> AppResult<Self> {
        let dial = client::DialCtx {
            config: crate::ssh::client::default_client_config(),
            known_hosts_path,
            timeout_secs,
            log: crate::ssh::client::null_logger(),
            prompt_ctx: None,
        };
        let mut connection = client::ssh_connect(dial, host, port).await?;

        let username = credential.username.clone();
        client::authenticate(&mut connection.handle, credential, None).await?;

        let channel = connection
            .handle
            .channel_open_session()
            .await
            .map_err(|e| {
                AppError::sftp(
                    "sftp_io_failed",
                    json!({ "op": "open channel", "err": e.to_string() }),
                )
            })?;

        channel.request_subsystem(true, "sftp").await.map_err(|e| {
            AppError::sftp(
                "sftp_io_failed",
                json!({ "op": "request_subsystem", "err": e.to_string() }),
            )
        })?;

        let sftp = russh_sftp::client::SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| AppError::sftp("sftp_init_failed", json!({ "err": e.to_string() })))?;

        Ok(Self {
            sftp,
            peer: client::VerifiedPeer {
                server: connection.server,
                username,
            },
            parent_ssh_id: None,
        })
    }

    pub async fn home_dir(&self) -> AppResult<String> {
        self.sftp.canonicalize(".").await.map_err(|e| {
            AppError::sftp(
                "sftp_io_failed",
                json!({ "op": "canonicalize", "err": e.to_string() }),
            )
        })
    }

    pub async fn list_dir(&self, path: &str) -> AppResult<Vec<RemoteEntry>> {
        let entries = self.sftp.read_dir(path).await.map_err(|e| {
            AppError::sftp(
                "sftp_io_failed",
                json!({ "op": "read_dir", "err": e.to_string() }),
            )
        })?;

        let mut result: Vec<RemoteEntry> = entries
            .map(|e| {
                let name = e.file_name();
                let ft = e.file_type();
                let meta = e.metadata();
                RemoteEntry {
                    name,
                    is_dir: ft.is_dir(),
                    is_symlink: ft.is_symlink(),
                    size: meta.size.unwrap_or(0),
                    mtime: meta.mtime.map(u64::from).unwrap_or(0),
                }
            })
            .collect();

        result.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        Ok(result)
    }

    /// Recursively list every downloadable file under `root`, returning a flat
    /// list of (relative path, size).
    ///
    /// - BFS to avoid blowing the stack on deep trees.
    /// - Symlink-to-file: follow once for size, treat as a regular file.
    ///   Symlink-to-dir: skipped to prevent loops.
    /// - Depth exceeding `WALK_DEPTH_CAP` fails the whole command.
    /// - Per-file failures are not handled here: each file is later dispatched
    ///   as an independent Transfer, which owns its own retry/cancel surface.
    ///   This function only builds the work list.
    pub async fn walk_files(&self, root: &str) -> AppResult<Vec<WalkEntry>> {
        let root_norm = root.trim_end_matches('/').to_string();
        let mut queue: VecDeque<(String, u32)> = VecDeque::new();
        queue.push_back((root.to_string(), 0));
        let mut result: Vec<WalkEntry> = Vec::new();

        while let Some((dir, depth)) = queue.pop_front() {
            if depth >= WALK_DEPTH_CAP {
                return Err(AppError::sftp(
                    "sftp_tree_too_deep",
                    json!({ "path": dir, "depth": depth, "limit": WALK_DEPTH_CAP }),
                ));
            }
            let entries = self.sftp.read_dir(&dir).await.map_err(|e| {
                AppError::sftp(
                    "sftp_io_failed",
                    json!({ "op": "read_dir", "path": dir, "err": e.to_string() }),
                )
            })?;
            for e in entries {
                let name = e.file_name();
                let full = join_remote(&dir, &name);
                let rel = full
                    .strip_prefix(&root_norm)
                    .unwrap_or(&full)
                    .trim_start_matches('/')
                    .to_string();
                let ft = e.file_type();
                if ft.is_dir() {
                    queue.push_back((full, depth + 1));
                } else if ft.is_file() {
                    result.push(WalkEntry {
                        rel_path: rel,
                        size: e.metadata().size.unwrap_or(0),
                    });
                } else if ft.is_symlink() {
                    // Follow once via STAT to learn what the target is.
                    if let Ok(meta) = self.sftp.metadata(&full).await {
                        if meta.file_type().is_file() {
                            result.push(WalkEntry {
                                rel_path: rel,
                                size: meta.size.unwrap_or(0),
                            });
                        }
                        // symlink-to-dir: skip to avoid cycles.
                        // anything else (block/char/fifo): skip.
                    }
                    // metadata failure (e.g. broken symlink): silently skip.
                }
                // Other types (block/char/fifo) are skipped.
            }
        }
        Ok(result)
    }

    pub async fn download(&self, remote_path: &str) -> AppResult<Vec<u8>> {
        self.sftp.read(remote_path).await.map_err(|e| {
            AppError::sftp(
                "sftp_io_failed",
                json!({ "op": "read", "err": e.to_string() }),
            )
        })
    }

    pub async fn upload(&self, remote_path: &str, data: &[u8]) -> AppResult<()> {
        self.resume_upload(
            remote_path,
            &mut std::io::Cursor::new(data),
            transfer::SourceStamp::new(Some(data.len() as u64), String::new()),
            &mut transfer::TransferCheckpoint::default(),
            |_, _| {},
            &AtomicBool::new(false),
        )
        .await
    }

    pub async fn mkdir(&self, path: &str) -> AppResult<()> {
        self.sftp.create_dir(path).await.map_err(|e| {
            AppError::sftp(
                "sftp_io_failed",
                json!({ "op": "create_dir", "err": e.to_string() }),
            )
        })
    }

    pub async fn remove_file(&self, path: &str) -> AppResult<()> {
        self.sftp.remove_file(path).await.map_err(|e| {
            AppError::sftp(
                "sftp_io_failed",
                json!({ "op": "remove_file", "err": e.to_string() }),
            )
        })
    }

    pub async fn remove_dir(&self, path: &str) -> AppResult<()> {
        self.sftp.remove_dir(path).await.map_err(|e| {
            AppError::sftp(
                "sftp_io_failed",
                json!({ "op": "remove_dir", "err": e.to_string() }),
            )
        })
    }

    /// Delete a file or a directory tree. LSTAT decides which — the frontend's
    /// listing can be stale (e.g. a deploy swapped a real dir for a symlink
    /// since the last refresh), and recursing through a symlink would delete
    /// the *target's* contents. Anything that is not a real directory — file,
    /// symlink, special — is removed by name.
    pub async fn remove(&self, path: &str) -> AppResult<()> {
        let meta = self.sftp.symlink_metadata(path).await.map_err(|e| {
            AppError::sftp(
                "sftp_io_failed",
                json!({ "op": "lstat", "err": e.to_string() }),
            )
        })?;
        if meta.file_type().is_dir() {
            self.remove_dir_all(path).await
        } else {
            self.remove_file(path).await
        }
    }

    /// Recursively delete a directory tree.
    ///
    /// Single BFS over `read_dir`: real directories are queued for traversal
    /// and recorded for bottom-up removal; everything else — regular files,
    /// symlinks (even symlinks to directories) and special files — is removed
    /// on the spot, because SFTP REMOVE deletes the name itself, never the
    /// target. Depth is capped like `walk_files`, so a hostile server (or a
    /// bind-mount cycle) cannot make the walk run forever.
    async fn remove_dir_all(&self, root: &str) -> AppResult<()> {
        let mut dirs: Vec<String> = Vec::new();
        let mut queue: VecDeque<(String, u32)> = VecDeque::new();
        queue.push_back((root.trim_end_matches('/').to_string(), 0));
        while let Some((dir, depth)) = queue.pop_front() {
            if depth >= WALK_DEPTH_CAP {
                return Err(AppError::sftp(
                    "sftp_tree_too_deep",
                    json!({ "path": dir, "depth": depth, "limit": WALK_DEPTH_CAP }),
                ));
            }
            let entries = self.sftp.read_dir(&dir).await.map_err(|e| {
                AppError::sftp(
                    "sftp_io_failed",
                    json!({ "op": "read_dir", "path": dir, "err": e.to_string() }),
                )
            })?;
            for e in entries {
                let full = join_remote(&dir, &e.file_name());
                if e.file_type().is_dir() {
                    queue.push_back((full.clone(), depth + 1));
                    dirs.push(full);
                } else {
                    self.remove_file(&full).await?;
                }
            }
        }
        // BFS order puts parents before children, so reverse = deepest-first.
        for d in dirs.iter().rev() {
            self.remove_dir(d).await?;
        }
        self.remove_dir(root).await
    }

    pub async fn rename(&self, old: &str, new: &str) -> AppResult<()> {
        self.sftp.rename(old, new).await.map_err(|e| {
            AppError::sftp(
                "sftp_io_failed",
                json!({ "op": "rename", "err": e.to_string() }),
            )
        })
    }

    pub async fn stat(&self, path: &str) -> AppResult<FileStat> {
        let meta = self.sftp.metadata(path).await.map_err(|e| {
            AppError::sftp(
                "sftp_io_failed",
                json!({ "op": "metadata", "err": e.to_string() }),
            )
        })?;
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        Ok(FileStat {
            name,
            is_dir: meta.file_type().is_dir(),
            size: meta.size.unwrap_or(0),
            mtime: meta.mtime.map(u64::from).unwrap_or(0),
            uid: meta.uid,
            gid: meta.gid,
            user: meta.user.clone(),
            group: meta.group.clone(),
            permissions: meta.permissions,
        })
    }

    /// Stream-download a remote file to a local path with a hard size cap.
    /// 优先用 metadata.size 早期 bail；metadata.size=None 或服务器撒谎时，
    /// 流式 cap 也会在超限的瞬间中止下载。
    ///
    /// 原子性：下载先写入 `<local_path>.part`，全部成功后 rename 到 local_path。
    /// 任何失败路径（size cap、IO error、metadata 错误）都会清理 .part，
    /// **不会污染 local_path**。无前端进度事件——AI 排障流程用。
    pub async fn download_to_path(
        &self,
        remote_path: &str,
        local_path: &Path,
        max_bytes: u64,
    ) -> AppResult<u64> {
        // 预检：metadata 已知大小且超限就早 bail，省一次 open。size=None
        // 时不能假装它是 0（之前的 unwrap_or(0) 会让任何"未声明大小"的文件
        // 直接绕过 max_bytes）—— 交给下面的 streaming cap 兜底。
        let meta = self.sftp.metadata(remote_path).await.map_err(|e| {
            AppError::sftp(
                "sftp_io_failed",
                json!({ "op": "metadata", "err": e.to_string() }),
            )
        })?;
        if let Some(size) = meta.size {
            if size > max_bytes {
                return Err(AppError::sftp(
                    "sftp_file_too_large",
                    json!({ "path": remote_path, "size": size, "limit": max_bytes }),
                ));
            }
        }

        // tmp_path = "<local_path>.part"。用 with_file_name 保留目录前缀。
        let file_name = local_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                AppError::sftp(
                    "sftp_io_failed",
                    json!({ "op": "create", "err": "invalid local path" }),
                )
            })?;
        let mut tmp_path = local_path.with_file_name(format!("{file_name}.part"));
        let mut remote_file = self.sftp.open(remote_path).await.map_err(|e| {
            AppError::sftp(
                "sftp_io_failed",
                json!({ "op": "open", "err": e.to_string() }),
            )
        })?;
        // `tokio::fs::File::create` runs on the blocking pool; dropping its
        // future cannot cancel an already-started create, so it could recreate
        // `.part` after the guard had removed it. The syscall is tiny: perform
        // it in this poll, then hand the open descriptor to Tokio for writes.
        let options = private_staging_options();
        let local_file = match options.open(&tmp_path) {
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                tmp_path =
                    local_path.with_file_name(format!(".rssh-{}.part", uuid::Uuid::new_v4()));
                options.open(&tmp_path)?
            }
            result => result?,
        };
        let partial = PartialDownloadGuard::new(tmp_path);
        let mut local_file = tokio::fs::File::from_std(local_file);

        let mut transferred: u64 = 0;
        let mut buf = vec![0u8; 32768];
        loop {
            let n = remote_file.read(&mut buf).await.map_err(|e| {
                AppError::sftp(
                    "sftp_io_failed",
                    json!({ "op": "read", "err": e.to_string() }),
                )
            })?;
            if n == 0 {
                break;
            }
            // 运行时 cap：metadata 缺失 / 撒谎 / 下载途中文件增长都靠这里兜底。
            // 这是 max_bytes 的唯一权威检查点，预检只是优化。
            let next = transferred + n as u64;
            if next > max_bytes {
                // 主动关闭 server-side handle，免得让远端 fd 等到 session drop 才回收。
                let _ = remote_file.shutdown().await;
                return Err(AppError::sftp(
                    "sftp_file_too_large",
                    json!({ "path": remote_path, "size": next, "limit": max_bytes }),
                ));
            }
            local_file.write_all(&buf[..n]).await?;
            transferred = next;
        }

        remote_file.shutdown().await.map_err(|e| {
            AppError::sftp(
                "sftp_io_failed",
                json!({ "op": "close", "err": e.to_string() }),
            )
        })?;
        local_file.shutdown().await?;
        drop(local_file);
        partial.commit(local_path)?;
        Ok(transferred)
    }

    /// Remote `mkdir -p`: ensure every ancestor directory of `path` exists.
    ///
    /// Concurrency note: under MAX_CONCURRENT uploads several tasks may race
    /// to create the same shared subdirectory. We therefore *blindly attempt*
    /// `create_dir` per segment and, on failure, fall back to a metadata probe.
    /// If a directory is now present, treat it as success (a peer created it);
    /// otherwise propagate the original create error. This trades a few extra
    /// SFTP round-trips on the cold path for race-freeness — `mkdir_p` is
    /// already off the hot single-file path because callers short-circuit
    /// when the parent already exists.
    async fn mkdir_p(&self, path: &str) -> AppResult<()> {
        if path.is_empty() || path == "/" {
            return Ok(());
        }
        if let Ok(meta) = self.sftp.metadata(path).await {
            // A non-directory occupying the target path would cause obscure
            // failures later when the upload tries to write under it.
            if !meta.is_dir() {
                return Err(AppError::sftp(
                    "sftp_io_failed",
                    json!({ "op": "mkdir_p", "path": path, "err": "path exists but is not a directory" }),
                ));
            }
            return Ok(());
        }
        let mut current = if path.starts_with('/') {
            "/".to_string()
        } else {
            String::new()
        };
        for part in path.split('/').filter(|s| !s.is_empty()) {
            if !current.is_empty() && !current.ends_with('/') {
                current.push('/');
            }
            current.push_str(part);
            if let Err(create_err) = self.sftp.create_dir(&current).await {
                // Race-safe fallback: verify a directory now exists.
                match self.sftp.metadata(&current).await {
                    Ok(meta) if meta.is_dir() => {} // created by a concurrent peer
                    _ => {
                        return Err(AppError::sftp(
                            "sftp_io_failed",
                            json!({ "op": "mkdir_p", "path": &current, "err": create_err.to_string() }),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_partial_download_removes_the_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let tmp_path = dir.path().join("artifact.part");
        std::fs::write(&tmp_path, b"partial").unwrap();

        {
            let _partial = PartialDownloadGuard::new(tmp_path.clone());
        }

        assert!(!tmp_path.exists());
    }

    #[test]
    fn committing_partial_download_replaces_the_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let local_path = dir.path().join("artifact");
        let tmp_path = dir.path().join("artifact.part");
        std::fs::write(&local_path, b"old").unwrap();
        std::fs::write(&tmp_path, b"new").unwrap();

        PartialDownloadGuard::new(tmp_path.clone())
            .commit(&local_path)
            .unwrap();

        assert_eq!(std::fs::read(&local_path).unwrap(), b"new");
        assert!(!tmp_path.exists());
    }

    // Exercise the real SFTP metadata/open protocol without an SSH server,
    // network port or application state. A successful stat followed by a denied
    // open is the case that previously destroyed the user's local file.
    struct DeniedDownload;
    impl russh_sftp::server::Handler for DeniedDownload {
        type Error = russh_sftp::protocol::StatusCode;
        fn unimplemented(&self) -> Self::Error {
            russh_sftp::protocol::StatusCode::PermissionDenied
        }
        async fn stat(
            &mut self,
            id: u32,
            _: String,
        ) -> Result<russh_sftp::protocol::Attrs, Self::Error> {
            Ok(russh_sftp::protocol::Attrs {
                id,
                attrs: Default::default(),
            })
        }
    }

    #[tokio::test]
    async fn remote_open_failure_never_opens_or_truncates_the_provider() {
        let (client, server) = tokio::io::duplex(4096);
        russh_sftp::server::run(server, DeniedDownload).await;
        let handle = SftpHandle {
            sftp: russh_sftp::client::SftpSession::new(client).await.unwrap(),
            peer: Default::default(),
            parent_ssh_id: None,
        };
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("existing");
        std::fs::write(&target, b"existing contents").unwrap();
        let result = handle
            .resume_download(
                "/denied",
                &transfer::PathDirectory::new(directory.path().into()),
                "existing",
                &mut transfer::TransferCheckpoint::default(),
                |_, _| {},
                &AtomicBool::new(false),
            )
            .await;
        assert!(result.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"existing contents");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
