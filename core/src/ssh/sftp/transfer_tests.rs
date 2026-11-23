use super::*;
use std::io::Cursor;

#[tokio::test]
async fn lost_acknowledgement_verifies_extra_bytes_and_only_writes_the_suffix() {
    let original = b"one-two-three";
    let mut checkpoint = TransferCheckpoint {
        submitted: 8,
        acknowledged: 4,
        hash: Sha256::new_with_prefix(&original[..4]),
        ..Default::default()
    };
    let mut source = Cursor::new(original.as_slice());
    let mut part = Cursor::new(original[..8].to_vec());
    let cancel = AtomicBool::new(false);
    verify_prefix(&mut source, &mut part, 8, &mut checkpoint, &cancel)
        .await
        .unwrap();
    let mut ticks = Vec::new();
    copy_remaining(
        &mut source,
        &mut part,
        &mut checkpoint,
        |n, _| ticks.push(n),
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(ticks, vec![8, 13]);
    assert_eq!(part.into_inner(), original);
    assert_eq!(checkpoint.hash.finalize(), Sha256::digest(original));
}

#[tokio::test]
async fn changed_source_or_partial_never_appends_even_if_both_have_matching_replacements() {
    let cancel = AtomicBool::new(false);
    for (source, received, expected) in [
        (b"old", b"NEW", "sftp_partial_changed"),
        (b"NEW", b"NEW", "sftp_source_changed"),
        (b"NEW", b"old", "sftp_source_changed"),
    ] {
        let mut checkpoint = TransferCheckpoint {
            submitted: 3,
            acknowledged: 3,
            hash: Sha256::new_with_prefix(b"old"),
            ..Default::default()
        };
        let mut part = Cursor::new(received.to_vec());
        let err = verify_prefix(
            &mut Cursor::new(source),
            &mut part,
            3,
            &mut checkpoint,
            &cancel,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), expected);
        assert_eq!(part.into_inner(), received);
        assert_eq!(checkpoint.hash.finalize(), Sha256::digest(b"old"));
    }
}

#[tokio::test]
async fn copy_reports_only_flushed_bytes_and_propagates_final_flush_failure() {
    let mut cp = TransferCheckpoint::default();
    let cancel = AtomicBool::new(false);
    let mut writer = tokio::io::BufWriter::with_capacity(128, Vec::new());
    copy_remaining(
        &mut Cursor::new(b"buffered"),
        &mut writer,
        &mut cp,
        |_, _| {},
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(writer.get_ref(), b"buffered");
    assert_eq!(cp.acknowledged, 8);

    let (output, input) = tokio::io::duplex(64);
    drop(input);
    let mut writer = tokio::io::BufWriter::with_capacity(128, output);
    let mut cp = TransferCheckpoint::default();
    let mut reported = vec![];
    let error = copy_remaining(
        &mut Cursor::new(b"buffered"),
        &mut writer,
        &mut cp,
        |n, _| reported.push(n),
        &cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), "io_error");
    assert_eq!(cp.acknowledged, 0);
    assert_eq!(reported, vec![0]);
}

#[tokio::test]
async fn cancelled_copy_keeps_old_target_and_owned_part_for_retry() {
    let root = tempfile::tempdir().unwrap();
    let dir = PathDirectory::new(root.path().into());
    let target_name = "中文 %2F.txt";
    std::fs::write(root.path().join(target_name), b"old").unwrap();
    std::fs::write(root.path().join("中文 %2F.txt.part"), b"private").unwrap();
    let name = private_name("part");
    let mut file = dir.create_part(&name).await.unwrap();
    #[cfg(any(macos, linux, android, ios, ohos))]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            file.metadata().await.unwrap().permissions().mode() & 0o077,
            0
        );
    }
    assert!(dir.create_part(&name).await.is_err());
    let cancel = AtomicBool::new(false);
    let mut cp = TransferCheckpoint::default();
    let data = vec![42u8; 512 * 1024];
    let result = copy_remaining(
        &mut Cursor::new(&data),
        &mut file,
        &mut cp,
        |n, _| {
            if n > 0 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
        &cancel,
    )
    .await;
    assert_eq!(result.unwrap_err().code(), CANCELLED_CODE);
    drop(file.into_std().await);
    assert_eq!(
        std::fs::read(root.path().join(target_name)).unwrap(),
        b"old"
    );
    assert_eq!(
        std::fs::read(root.path().join("中文 %2F.txt.part")).unwrap(),
        b"private"
    );
    cancel.store(false, Ordering::Relaxed);
    let mut file = dir.open_part(&name).await.unwrap();
    let mut source = Cursor::new(&data);
    let length = file.metadata().await.unwrap().len();
    assert!(length > 0 && length < data.len() as u64);
    verify_prefix(&mut source, &mut file, length, &mut cp, &cancel)
        .await
        .unwrap();
    copy_remaining(&mut source, &mut file, &mut cp, |_, _| {}, &cancel)
        .await
        .unwrap();
    drop(file.into_std().await);
    dir.publish(&name, target_name).await.unwrap();
    assert_eq!(std::fs::read(root.path().join(target_name)).unwrap(), data);
    assert!(!root.path().join(name).exists());
}

#[tokio::test]
async fn cancelled_before_copy_writes_nothing_and_existing_directory_is_never_replaced() {
    let mut destination = Vec::new();
    let err = copy_remaining(
        &mut Cursor::new(b"bytes"),
        &mut destination,
        &mut Default::default(),
        |_, _| {},
        &AtomicBool::new(true),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), CANCELLED_CODE);
    assert!(destination.is_empty());

    let root = tempfile::tempdir().unwrap();
    let dir = PathDirectory::new(root.path().into());
    std::fs::create_dir(root.path().join("target")).unwrap();
    std::fs::write(root.path().join("target/important"), b"retain").unwrap();
    assert!(dir.target_stamp("target").await.is_err());
    assert!(dir.create_part("../outside").await.is_err());
    assert_eq!(
        std::fs::read(root.path().join("target/important")).unwrap(),
        b"retain"
    );
}

#[test]
fn backup_moves_never_clobber_and_interrupted_commit_preserves_recovery_path() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    let backup = root.path().join("backup.bak");
    std::fs::write(&target, b"old").unwrap();
    std::fs::write(&backup, b"unrelated").unwrap();
    assert!(move_noclobber(&target, &backup).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"old");
    assert_eq!(std::fs::read(&backup).unwrap(), b"unrelated");
    let backup = backup.to_str().unwrap();
    let mut cp = TransferCheckpoint::default();
    cp.commit_started(backup);
    assert_eq!(cp.recovery_path(), Some(backup));
    let error = cp.check(&SourceStamp::unknown(), &None).unwrap_err();
    assert_eq!(error.code(), "sftp_replace_uncertain");
}

// Exercise the v3 protocol error path: an occupied target alone is not proof
// of a collision. A server can reject all renames with the very same FX_FAILURE.
#[derive(Clone, Copy)]
enum RenamePolicy {
    DenyAll,
    GenericFailure,
    NoOverwrite,
    FailPublication,
    Unknown,
    AllowProbeOverwrite,
    LoseProbeReply,
}

struct RenameServer {
    root: PathBuf,
    policy: RenamePolicy,
}
use russh_sftp::protocol::{Attrs, Handle, SftpHandle as FileHandle, Status};

impl russh_sftp::server::Handler for RenameServer {
    type Error = StatusCode;
    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }
    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let meta =
            std::fs::symlink_metadata(self.root.join(path)).map_err(|_| StatusCode::NoSuchFile)?;
        Ok(Attrs {
            id,
            attrs: (&meta).into(),
        })
    }
    async fn fstat(&mut self, id: u32, handle: FileHandle) -> Result<Attrs, Self::Error> {
        self.lstat(id, String::from_utf8(handle.into_bytes()).unwrap())
            .await
    }
    async fn open(
        &mut self,
        id: u32,
        name: String,
        flags: OpenFlags,
        attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        assert_eq!(attrs.permissions, Some(0o600));
        assert!(flags.contains(OpenFlags::CREATE | OpenFlags::EXCLUDE));
        std::fs::File::create_new(self.root.join(&name)).map_err(|_| StatusCode::Failure)?;
        Ok(Handle {
            id,
            handle: name.into(),
        })
    }
    async fn close(&mut self, _: u32, _: FileHandle) -> Result<Status, Self::Error> {
        Err(StatusCode::Ok)
    }
    async fn remove(&mut self, _: u32, path: String) -> Result<Status, Self::Error> {
        std::fs::remove_file(self.root.join(path)).map_err(|_| StatusCode::Failure)?;
        Err(StatusCode::Ok)
    }
    async fn rename(&mut self, _: u32, from: String, to: String) -> Result<Status, Self::Error> {
        match self.policy {
            RenamePolicy::DenyAll => return Err(StatusCode::PermissionDenied),
            RenamePolicy::GenericFailure => return Err(StatusCode::Failure),
            RenamePolicy::Unknown => return Err(StatusCode::ConnectionLost),
            RenamePolicy::AllowProbeOverwrite | RenamePolicy::LoseProbeReply if to == "target" => {
                return Err(StatusCode::Failure)
            }
            RenamePolicy::AllowProbeOverwrite | RenamePolicy::LoseProbeReply
                if to.ends_with(".part") =>
            {
                std::fs::rename(self.root.join(from), self.root.join(to)).unwrap();
                return Err(if matches!(self.policy, RenamePolicy::LoseProbeReply) {
                    StatusCode::ConnectionLost
                } else {
                    StatusCode::Ok
                });
            }
            RenamePolicy::FailPublication if from.ends_with(".part") && to == "target" => {
                return Err(StatusCode::Failure)
            }
            _ => {}
        }
        if self.root.join(&to).exists() {
            return Err(StatusCode::Failure);
        }
        std::fs::rename(self.root.join(from), self.root.join(to))
            .map_err(|_| StatusCode::Failure)?;
        Err(StatusCode::Ok)
    }
}

#[tokio::test]
async fn only_verified_collision_moves_the_old_file_and_failed_publication_preserves_its_backup() {
    for policy in [
        RenamePolicy::DenyAll,
        RenamePolicy::GenericFailure,
        RenamePolicy::NoOverwrite,
        RenamePolicy::FailPublication,
        RenamePolicy::Unknown,
        RenamePolicy::AllowProbeOverwrite,
        RenamePolicy::LoseProbeReply,
    ] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("mine.part"), b"new").unwrap();
        std::fs::write(root.path().join("user.bak"), b"private").unwrap();
        std::fs::write(root.path().join("target"), b"old").unwrap();
        let (client, server) = tokio::io::duplex(4096);
        russh_sftp::server::run(
            server,
            RenameServer {
                root: root.path().into(),
                policy,
            },
        )
        .await;
        let h = SftpHandle {
            sftp: russh_sftp::client::SftpSession::new(client).await.unwrap(),
            peer: Default::default(),
            parent_ssh_id: None,
        };
        let mut cp = TransferCheckpoint {
            acknowledged: 3,
            destination: Some(h.remote_target("target").await.unwrap()),
            ..Default::default()
        };
        let written = h.remote_target("mine.part").await.unwrap().unwrap();
        let result = h
            .publish_remote("mine.part", "target", &written, &mut cp)
            .await;
        let result = cp.published(result);
        match policy {
            RenamePolicy::NoOverwrite => {
                result.unwrap();
                assert_eq!(std::fs::read(root.path().join("target")).unwrap(), b"new");
                assert!(!root.path().join("mine.bak").exists());
            }
            RenamePolicy::FailPublication => {
                let err = result.unwrap_err();
                assert_eq!(err.code(), "sftp_replace_backup");
                assert_eq!(cp.recovery_path(), Some("mine.bak"));
                assert_eq!(std::fs::read(root.path().join("mine.bak")).unwrap(), b"old");
                assert_eq!(
                    std::fs::read(root.path().join(cp.part_path().unwrap())).unwrap(),
                    b"new"
                );
            }
            RenamePolicy::AllowProbeOverwrite | RenamePolicy::LoseProbeReply => {
                let error = result.unwrap_err();
                let part = std::fs::read_dir(root.path())
                    .unwrap()
                    .map(Result::unwrap)
                    .find(|p| p.path().extension().is_some_and(|ext| ext == "part"))
                    .unwrap();
                assert_ne!(part.file_name(), "mine.part");
                assert_eq!(std::fs::read(part.path()).unwrap(), b"new");
                assert!(cp
                    .recovery_path()
                    .unwrap()
                    .contains(part.file_name().to_str().unwrap()));
                if matches!(policy, RenamePolicy::LoseProbeReply) {
                    assert_eq!(error.code(), "sftp_replace_uncertain");
                }
                assert_eq!(std::fs::read(root.path().join("target")).unwrap(), b"old");
            }
            _ => {
                assert!(result.is_err());
                assert_eq!(std::fs::read(root.path().join("target")).unwrap(), b"old");
                assert!(!root.path().join("mine.bak").exists());
            }
        }
        assert_eq!(
            std::fs::read(root.path().join("user.bak")).unwrap(),
            b"private"
        );
        assert_eq!(
            std::fs::read_dir(root.path()).unwrap().count(),
            if matches!(policy, RenamePolicy::NoOverwrite) {
                2
            } else {
                3
            }
        );
    }
}

#[tokio::test]
async fn source_io_failure_leaves_the_original_and_acknowledged_partial_bytes_intact() {
    struct FailedRead;
    impl AsyncRead for FailedRead {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Err(std::io::Error::other("interrupted read")))
        }
    }
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("target"), b"original").unwrap();
    let dir = PathDirectory::new(root.path().into());
    let part = private_name("part");
    let mut file = dir.create_part(&part).await.unwrap();
    let bytes = vec![9; 256 * 1024];
    let mut source = Cursor::new(&bytes).chain(FailedRead);
    let mut checkpoint = TransferCheckpoint::default();
    let result = copy_remaining(
        &mut source,
        &mut file,
        &mut checkpoint,
        |_, _| {},
        &AtomicBool::new(false),
    )
    .await;
    drop(file.into_std().await);
    assert_eq!(result.unwrap_err().code(), "io_error");
    assert_eq!(checkpoint.acknowledged, bytes.len() as u64);
    assert_eq!(std::fs::read(root.path().join(part)).unwrap(), bytes);
    assert_eq!(
        std::fs::read(root.path().join("target")).unwrap(),
        b"original"
    );
}
