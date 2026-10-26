//! A download owns its temporary file until it commits or aborts.

use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

pub struct AtomicDownload {
    file: Option<tokio::fs::File>,
    runtime: tokio::runtime::Handle,
    temporary: PathBuf,
    destination: PathBuf,
    committed: bool,
}

impl AtomicDownload {
    pub fn create(destination: &Path) -> std::io::Result<Self> {
        let parent = destination.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing download parent")
        })?;
        std::fs::create_dir_all(parent)?;
        // Never share `<name>.part`: concurrent downloads and pre-existing user
        // files must not be truncated or removed by another transfer.
        let temporary = parent.join(format!(".rssh-download-{}.part", uuid::Uuid::new_v4()));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        Ok(Self {
            file: Some(tokio::fs::File::from_std(file)),
            runtime: tokio::runtime::Handle::current(),
            temporary,
            destination: destination.to_owned(),
            committed: false,
        })
    }

    pub fn stream(&mut self) -> &mut tokio::fs::File {
        self.file.as_mut().expect("download is open")
    }

    async fn close(&mut self) -> std::io::Result<()> {
        // Keep ownership in self while awaiting, so dropping this future also
        // waits for any Tokio worker before removing its temporary file.
        self.stream().shutdown().await?;
        drop(self.file.take());
        Ok(())
    }

    pub async fn commit(mut self) -> std::io::Result<()> {
        self.close().await?;
        // std::fs::rename replaces existing files on Unix and Windows. Do not
        // unlink the original first: rename failure must leave it intact.
        std::fs::rename(&self.temporary, &self.destination)?;
        self.committed = true;
        Ok(())
    }

    pub async fn abort(mut self) {
        if self.close().await.is_ok() {
            let _ = std::fs::remove_file(&self.temporary);
        }
    }
}

impl Drop for AtomicDownload {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Some(file) = self.file.take() {
            let temporary = self.temporary.clone();
            // File::drop can leave a blocking write running. Close its actual
            // descriptor before cleanup, including when a transfer is dropped.
            self.runtime.spawn(async move {
                drop(file.into_std().await);
                let _ = std::fs::remove_file(temporary);
            });
        } else {
            let _ = std::fs::remove_file(&self.temporary);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abandoned_download_cleans_up_outside_the_runtime_thread() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("existing.txt");
        std::fs::write(&destination, b"original").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let download = runtime.block_on(async {
            let mut download = AtomicDownload::create(&destination).unwrap();
            download
                .stream()
                .write_all(b"partial replacement")
                .await
                .unwrap();
            download
        });
        let temporary = download.temporary.clone();
        // A window or owner can disappear on a non-Tokio thread.
        drop(download);
        runtime.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while temporary.exists() {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        });
        assert_eq!(std::fs::read(destination).unwrap(), b"original");
    }
}
