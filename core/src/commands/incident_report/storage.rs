use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{not_found, ReportSummary};
use crate::error::{AppError, AppResult};

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Index {
    pub current: Option<Uuid>,
    pub reports: Vec<Record>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Record {
    pub id: Uuid,
    revision: Uuid,
    pub title: String,
    pub created_at: i64,
    pub last_session_id: Option<String>,
    pub last_heading_offset: Option<usize>,
    pub last_edit_bytes: usize,
}

impl Record {
    pub fn new(title: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            revision: Uuid::nil(),
            title: title.to_owned(),
            created_at: chrono::Utc::now().timestamp_millis(),
            last_session_id: None,
            last_heading_offset: None,
            last_edit_bytes: 0,
        }
    }

    fn filename(&self) -> String {
        format!("{}.{}.md", self.id, self.revision)
    }
}

pub(super) struct Store {
    dir: PathBuf,
    // Replacing index.json changes its inode. Every process locks this stable sibling.
    _lock: File,
    pub index: Index,
}

impl Store {
    pub fn open(data_dir: &Path) -> AppResult<Self> {
        let dir = data_dir.join("incident-reports");
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(&dir)?;

        let lock = private_options()
            .read(true)
            .create(true)
            .truncate(false)
            .open(dir.join("index.lock"))?;
        lock.lock()?;
        let mut store = Self {
            dir,
            _lock: lock,
            index: Index::default(),
        };
        match fs::read(store.dir.join("index.json")) {
            Ok(data) => store.index = serde_json::from_slice(&data).map_err(io::Error::from)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // A missing index is safe to initialize only in a new store. A
                // user restoring a directory incompletely can still recover its Markdown.
                for entry in fs::read_dir(&store.dir)? {
                    let name = entry?.file_name();
                    if name
                        .to_str()
                        .is_some_and(|s| s.ends_with(".md") && owned_file(s))
                    {
                        return Err(AppError::config(
                            "incident_report_missing_index",
                            serde_json::json!({}),
                        ));
                    }
                }
                // Persist the new directory and the empty index before the
                // first body, so even an interrupted first capture can recover.
                sync_dir(data_dir)?;
                store.commit()?;
            }
            Err(error) => return Err(error.into()),
        }
        Ok(store)
    }

    pub fn open_for_write(data_dir: &Path) -> AppResult<Self> {
        let store = Self::open(data_dir)?;
        // A previous publisher may have exited just after rename. Make that
        // index durable before reclaiming any body named by its predecessor.
        sync_dir(&store.dir)?;
        store.clean_up();
        Ok(store)
    }

    pub fn summaries(&self) -> Vec<ReportSummary> {
        self.index
            .reports
            .iter()
            .map(|row| ReportSummary {
                id: row.id.to_string(),
                title: row.title.clone(),
                created_at: row.created_at,
                is_current: Some(row.id) == self.index.current,
            })
            .collect()
    }

    pub fn position(&self, id: &str) -> Option<usize> {
        let id = Uuid::parse_str(id).ok()?;
        self.index.reports.iter().position(|row| row.id == id)
    }

    pub fn record(&self, id: &str) -> AppResult<usize> {
        self.position(id).ok_or_else(not_found)
    }

    pub fn read(&self, position: usize) -> AppResult<String> {
        Ok(fs::read_to_string(
            self.dir.join(self.index.reports[position].filename()),
        )?)
    }

    pub fn write(&mut self, position: usize, content: &str) -> AppResult<()> {
        let row = &mut self.index.reports[position];
        row.revision = Uuid::new_v4();
        let body = self.dir.join(row.filename());
        write_new(&body, content.as_bytes())?;
        // Make the body durable before the index can name it. Until rename
        // succeeds it is safe to discard this new revision on any failure.
        if let Err(error) = sync_dir(&self.dir).and_then(|()| self.publish_index()) {
            remove_unpublished(&body);
            return Err(error.into());
        }
        self.finish_commit()
    }

    fn publish_index(&self) -> io::Result<()> {
        let data = serde_json::to_vec_pretty(&self.index)?;
        let pending = self.dir.join(format!("index.{}.tmp", Uuid::new_v4()));
        write_new(&pending, &data)?;
        if let Err(error) = fs::rename(&pending, self.dir.join("index.json")) {
            remove_unpublished(&pending);
            return Err(error);
        }
        Ok(())
    }

    pub fn commit(&self) -> AppResult<()> {
        self.publish_index()?;
        self.finish_commit()
    }

    fn finish_commit(&self) -> AppResult<()> {
        // A failed directory flush after rename must leave both versions on disk.
        sync_dir(&self.dir)?;
        self.clean_up();
        Ok(())
    }

    fn clean_up(&self) {
        if let Err(error) = self.remove_orphans() {
            log::warn!("Incident report file cleanup failed: {error}");
        }
    }

    fn remove_orphans(&self) -> io::Result<()> {
        let live: HashSet<_> = self.index.reports.iter().map(Record::filename).collect();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if owned_file(name) && !live.contains(name) && entry.file_type()?.is_file() {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
}

fn owned_file(name: &str) -> bool {
    let Some((prefix, rest)) = name.split_once('.') else {
        return false;
    };
    let Some((revision, ext)) = rest.split_once('.') else {
        return false;
    };
    let canonical = |s: &str| Uuid::parse_str(s).is_ok_and(|id| id.to_string() == s);
    canonical(revision)
        && ((prefix == "index" && ext == "tmp") || (canonical(prefix) && ext == "md"))
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
}

fn write_new(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut file = private_options().create_new(true).open(path)?;
    let result = file.write_all(data).and_then(|()| file.sync_all());
    drop(file);
    if result.is_err() {
        remove_unpublished(path);
    }
    result
}

fn remove_unpublished(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        log::warn!("Failed to remove unpublished incident report file: {error}");
    }
}

fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}
