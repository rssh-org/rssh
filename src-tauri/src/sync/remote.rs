use async_trait::async_trait;
use std::path::Path;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::secret::SecretStore;
use crate::sync::metadata::{adopt_remote_version, refresh_local_metadata, SyncMetadata};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupNamespace {
    Current,
    Legacy,
}

#[async_trait]
pub trait RemoteBackup: Send + Sync {
    async fn read_payload(&self, namespace: BackupNamespace) -> AppResult<String>;
    async fn read_metadata(&self, namespace: BackupNamespace) -> AppResult<Option<SyncMetadata>>;
    async fn write_payload(&self, namespace: BackupNamespace, content: &str) -> AppResult<()>;
    async fn write_metadata(
        &self,
        namespace: BackupNamespace,
        metadata: &SyncMetadata,
    ) -> AppResult<()>;
}

pub struct PreparedBackup {
    pub json: String,
    pub metadata: SyncMetadata,
}

/// Build the encrypted-backup input and its plaintext metadata through the
/// same path for GUI and CLI. The two remote writes remain deliberately
/// non-transactional: the product explicitly permits concurrent pushes.
pub fn prepare_backup(
    db: &Db,
    secrets: &dyn SecretStore,
    data_dir: &Path,
) -> AppResult<PreparedBackup> {
    let prefs = crate::sync::config::read_sync_prefs(db)?;
    let payload = crate::sync::config::build_payload(
        db,
        secrets,
        data_dir,
        &crate::sync::config::ExportMode::RemotePush(prefs),
    )?;
    let json = serde_json::to_string_pretty(&payload).map_err(|e| {
        AppError::other("serde_failed", serde_json::json!({ "err": e.to_string() }))
    })?;
    let metadata = refresh_local_metadata(db, data_dir)?;
    Ok(PreparedBackup { json, metadata })
}

/// Current clients use an isolated namespace that old clients cannot truncate.
/// A newer legacy snapshot is merged on top of the current snapshot so old-client
/// updates survive without replacing multi-rule data they cannot represent.
pub async fn publish(
    remote: &dyn RemoteBackup,
    encrypted_payload: &str,
    metadata: &SyncMetadata,
) -> AppResult<()> {
    remote
        .write_payload(BackupNamespace::Current, encrypted_payload)
        .await?;
    remote
        .write_metadata(BackupNamespace::Current, metadata)
        .await?;
    remote
        .write_payload(BackupNamespace::Legacy, encrypted_payload)
        .await?;
    remote
        .write_metadata(BackupNamespace::Legacy, metadata)
        .await
}

pub struct FetchedBackup {
    pub encrypted_payloads: Vec<String>,
    pub metadata: Option<SyncMetadata>,
}

pub async fn fetch_metadata(remote: &dyn RemoteBackup) -> AppResult<Option<SyncMetadata>> {
    let current = remote.read_metadata(BackupNamespace::Current).await?;
    let legacy = remote.read_metadata(BackupNamespace::Legacy).await?;
    match (current, legacy) {
        (Some(current), Some(legacy)) => Ok(Some(if legacy.version > current.version {
            legacy
        } else {
            current
        })),
        (current, legacy) => Ok(current.or(legacy)),
    }
}

/// The metadata file is optional for compatibility with old backups. If it is
/// present, malformed content is an error instead of being silently ignored.
pub async fn fetch(remote: &dyn RemoteBackup) -> AppResult<FetchedBackup> {
    let current = remote.read_metadata(BackupNamespace::Current).await?;
    let legacy = remote.read_metadata(BackupNamespace::Legacy).await?;
    match (current, legacy) {
        (Some(current), Some(legacy)) if legacy.version > current.version => Ok(FetchedBackup {
            encrypted_payloads: vec![
                remote.read_payload(BackupNamespace::Current).await?,
                remote.read_payload(BackupNamespace::Legacy).await?,
            ],
            metadata: Some(legacy),
        }),
        (Some(current), _) => Ok(FetchedBackup {
            encrypted_payloads: vec![remote.read_payload(BackupNamespace::Current).await?],
            metadata: Some(current),
        }),
        (None, legacy) => Ok(FetchedBackup {
            encrypted_payloads: vec![remote.read_payload(BackupNamespace::Legacy).await?],
            metadata: legacy,
        }),
    }
}

/// Apply the existing additive import and then rebase local metadata. A valid
/// remote version is adopted verbatim, including downgrades, as required by
/// the existing push/pull contract.
pub fn apply_fetched_backup(
    db: &Db,
    secrets: &dyn SecretStore,
    data_dir: &Path,
    fetched: FetchedBackup,
    password: &str,
) -> AppResult<SyncMetadata> {
    for encrypted_payload in fetched.encrypted_payloads {
        let json = crate::crypto::decrypt(&encrypted_payload, password)?;
        let payload: serde_json::Value = serde_json::from_str(&json).map_err(|e| {
            AppError::config(
                "json_parse_failed",
                serde_json::json!({ "err": e.to_string() }),
            )
        })?;

        if let Err(err) = crate::sync::config::merge_import(db, secrets, data_dir, &payload) {
            // merge_import can retain successful rows before returning its
            // aggregate error. Record that actual partial state without granting a
            // failed pull permission to adopt the remote version.
            if let Err(refresh_err) = refresh_local_metadata(db, data_dir) {
                log::warn!("failed to refresh sync metadata after pull error: {refresh_err}");
            }
            return Err(err);
        }
    }

    match fetched.metadata {
        Some(metadata) => adopt_remote_version(db, data_dir, metadata.version),
        None => refresh_local_metadata(db, data_dir),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppResult;
    use crate::secret::SecretStore;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemSecrets(Mutex<HashMap<String, String>>);

    impl SecretStore for MemSecrets {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            self.0
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_owned());
            Ok(())
        }

        fn delete(&self, key: &str) -> AppResult<()> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }

        fn backend_name(&self) -> &'static str {
            "mem"
        }
    }

    #[derive(Default)]
    struct FakeRemote {
        writes: Mutex<Vec<(BackupNamespace, &'static str)>>,
        current_payload: Mutex<Option<String>>,
        current_metadata: Mutex<Option<SyncMetadata>>,
        legacy_payload: Mutex<Option<String>>,
        legacy_metadata: Mutex<Option<SyncMetadata>>,
    }

    #[async_trait::async_trait]
    impl RemoteBackup for FakeRemote {
        async fn read_payload(&self, namespace: BackupNamespace) -> AppResult<String> {
            let payload = match namespace {
                BackupNamespace::Current => &self.current_payload,
                BackupNamespace::Legacy => &self.legacy_payload,
            };
            payload
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| AppError::other("test_payload_missing", serde_json::json!({})))
        }

        async fn read_metadata(
            &self,
            namespace: BackupNamespace,
        ) -> AppResult<Option<SyncMetadata>> {
            let metadata = match namespace {
                BackupNamespace::Current => &self.current_metadata,
                BackupNamespace::Legacy => &self.legacy_metadata,
            };
            Ok(metadata.lock().unwrap().clone())
        }

        async fn write_payload(&self, namespace: BackupNamespace, content: &str) -> AppResult<()> {
            self.writes.lock().unwrap().push((namespace, "payload"));
            let payload = match namespace {
                BackupNamespace::Current => &self.current_payload,
                BackupNamespace::Legacy => &self.legacy_payload,
            };
            *payload.lock().unwrap() = Some(content.into());
            Ok(())
        }

        async fn write_metadata(
            &self,
            namespace: BackupNamespace,
            metadata: &SyncMetadata,
        ) -> AppResult<()> {
            self.writes.lock().unwrap().push((namespace, "metadata"));
            let target = match namespace {
                BackupNamespace::Current => &self.current_metadata,
                BackupNamespace::Legacy => &self.legacy_metadata,
            };
            *target.lock().unwrap() = Some(metadata.clone());
            Ok(())
        }
    }

    fn metadata(version: u64) -> SyncMetadata {
        SyncMetadata {
            version,
            config_digest: format!("sha256:{}", "a".repeat(64)),
        }
    }

    #[tokio::test]
    async fn publish_writes_current_before_legacy_mirror() {
        let remote = FakeRemote::default();
        let metadata = metadata(7);

        publish(&remote, "encrypted", &metadata).await.unwrap();

        assert_eq!(
            *remote.writes.lock().unwrap(),
            vec![
                (BackupNamespace::Current, "payload"),
                (BackupNamespace::Current, "metadata"),
                (BackupNamespace::Legacy, "payload"),
                (BackupNamespace::Legacy, "metadata"),
            ]
        );
        assert_eq!(
            remote.current_metadata.lock().unwrap().as_ref(),
            Some(&metadata)
        );
    }

    #[tokio::test]
    async fn fetch_allows_missing_metadata() {
        let remote = FakeRemote::default();
        *remote.legacy_payload.lock().unwrap() = Some("legacy-encrypted-payload".into());

        let fetched = fetch(&remote).await.unwrap();

        assert_eq!(fetched.encrypted_payloads, ["legacy-encrypted-payload"]);
        assert!(fetched.metadata.is_none());
    }

    #[tokio::test]
    async fn fetch_applies_current_then_newer_legacy_data() {
        let remote = FakeRemote::default();
        *remote.current_payload.lock().unwrap() = Some("current".into());
        *remote.current_metadata.lock().unwrap() = Some(metadata(2));
        *remote.legacy_payload.lock().unwrap() = Some("truncated-by-old-client".into());
        *remote.legacy_metadata.lock().unwrap() = Some(metadata(99));

        let fetched = fetch(&remote).await.unwrap();

        assert_eq!(
            fetched.encrypted_payloads,
            ["current", "truncated-by-old-client"]
        );
        assert_eq!(fetched.metadata, Some(metadata(99)));
    }

    #[test]
    fn applying_newer_legacy_payload_preserves_current_extra_rules() {
        let db = Db::open_in_memory().unwrap();
        let secrets = MemSecrets::default();
        let data_dir = tempfile::tempdir().unwrap();
        let current = serde_json::json!({
            "version": 1,
            "forwards": [{
                "id": "f1", "name": "mixed", "profile_id": "p1",
                "type": "local", "local_port": 8080,
                "remote_host": "db.internal", "remote_port": 5432,
                "rules": [
                    { "type": "local", "local_port": 8080, "remote_host": "db.internal", "remote_port": 5432 },
                    { "type": "dynamic", "local_port": 1080, "remote_host": "127.0.0.1", "remote_port": 0 }
                ]
            }]
        });
        let legacy = serde_json::json!({
            "version": 1,
            "forwards": [{
                "id": "f1", "name": "renamed", "profile_id": "p1",
                "type": "local", "local_port": 8081,
                "remote_host": "new-db.internal", "remote_port": 5432
            }]
        });

        apply_fetched_backup(
            &db,
            &secrets,
            data_dir.path(),
            FetchedBackup {
                encrypted_payloads: vec![
                    crate::crypto::encrypt(&current.to_string(), "pw").unwrap(),
                    crate::crypto::encrypt(&legacy.to_string(), "pw").unwrap(),
                ],
                metadata: Some(metadata(2)),
            },
            "pw",
        )
        .unwrap();

        let forward = crate::db::forward::get(&db, "f1").unwrap();
        assert_eq!(forward.name, "renamed");
        assert_eq!(forward.rules.len(), 2);
        assert_eq!(forward.rules[0].local_port, 8081);
        assert_eq!(
            forward.rules[1].forward_type,
            crate::models::ForwardType::Dynamic
        );
        assert_eq!(forward.rules[1].local_port, 1080);
    }

    #[test]
    fn successful_pull_adopts_lower_remote_version_even_after_additive_merge() {
        let db = Db::open_in_memory().unwrap();
        let secrets = MemSecrets::default();
        let data_dir = tempfile::tempdir().unwrap();
        let payload = crate::sync::config::build_payload(
            &db,
            &secrets,
            data_dir.path(),
            &crate::sync::config::ExportMode::LocalBackup,
        )
        .unwrap();
        let encrypted = crate::crypto::encrypt(&payload.to_string(), "pw").unwrap();
        adopt_remote_version(&db, data_dir.path(), 9).unwrap();

        let local = apply_fetched_backup(
            &db,
            &secrets,
            data_dir.path(),
            FetchedBackup {
                encrypted_payloads: vec![encrypted],
                metadata: Some(metadata(3)),
            },
            "pw",
        )
        .unwrap();

        assert_eq!(local.version, 3);
    }

    #[test]
    fn partial_pull_failure_keeps_the_import_error_and_refreshes_local_metadata() {
        let db = Db::open_in_memory().unwrap();
        let secrets = MemSecrets::default();
        let data_dir = tempfile::tempdir().unwrap();
        let before = refresh_local_metadata(&db, data_dir.path()).unwrap();
        let payload = serde_json::json!({
            "version": 1,
            "highlights": [{ "invalid": true }],
            "snippets": [{ "name": "remote", "command": "echo imported" }],
        });
        let encrypted = crate::crypto::encrypt(&payload.to_string(), "pw").unwrap();

        let err = apply_fetched_backup(
            &db,
            &secrets,
            data_dir.path(),
            FetchedBackup {
                encrypted_payloads: vec![encrypted],
                metadata: Some(metadata(9)),
            },
            "pw",
        )
        .unwrap_err();

        assert_eq!(err.code(), "import_partial_failed");
        let after = crate::sync::metadata::load_local_metadata(&db)
            .unwrap()
            .unwrap();
        assert_eq!(after.version, before.version + 1);
        assert!(crate::db::snippet::load(data_dir.path())
            .unwrap()
            .iter()
            .any(|snippet| snippet.name == "remote"));
    }
}
