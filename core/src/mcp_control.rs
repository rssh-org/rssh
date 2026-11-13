//! Discovery metadata and framed JSON over native local MCP control transports.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

mod transport;
mod wire;

pub use transport::{native_transport, BoxStream, LocalListener, LocalStream, LocalTransport};
pub use wire::{read_json, request, write_json, ControlRequest, REQUEST_LIMIT, RESPONSE_LIMIT};

pub const CONTROL_VERSION: u32 = 2;
const MAX_DESCRIPTOR_BYTES: u64 = 4096;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControlDescriptor {
    pub version: u32,
    pub instance_id: String,
    pub pid: u32,
    pub endpoint: String,
    pub host: String,
}

#[derive(Default)]
pub struct DescriptorScan {
    pub descriptors: Vec<ControlDescriptor>,
    pub ignored: Vec<String>,
}

fn invalid(reason: &str) -> AppError {
    AppError::config(
        "mcp_control_descriptor_invalid",
        serde_json::json!({ "reason": reason }),
    )
}

fn validate_instance(instance_id: &str) -> AppResult<()> {
    let id = uuid::Uuid::parse_str(instance_id).map_err(|_| invalid("invalid instance UUID"))?;
    if id.to_string() != instance_id {
        return Err(invalid("instance UUID must use canonical lowercase form"));
    }
    Ok(())
}

impl ControlDescriptor {
    pub fn validate(&self) -> AppResult<()> {
        validate_instance(&self.instance_id)?;
        if self.version != CONTROL_VERSION || self.pid == 0 {
            return Err(invalid("unsupported version or process ID"));
        }
        native_transport()
            .validate_endpoint(&self.endpoint, &self.instance_id)
            .map_err(|_| invalid("endpoint does not match this platform and instance"))?;
        if !matches!(self.host.as_str(), "desktop" | "headless") {
            return Err(invalid("unsupported host"));
        }
        Ok(())
    }
}

pub fn descriptor_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("runtime").join("mcp-control")
}

/// Publish one complete non-secret descriptor, never a partially written JSON file.
pub fn publish_descriptor(data_dir: &Path, descriptor: &ControlDescriptor) -> AppResult<()> {
    descriptor.validate()?;
    let directory = descriptor_dir(data_dir);
    let bytes = serde_json::to_vec(descriptor).map_err(|_| invalid("cannot encode descriptor"))?;
    publish_metadata(&directory, &descriptor.instance_id, &bytes)
}

fn publish_metadata(directory: &Path, instance_id: &str, bytes: &[u8]) -> AppResult<()> {
    fs::create_dir_all(&directory)?;
    #[cfg(any(macos, linux, android, ios, ohos))]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    let target = directory.join(format!("{instance_id}.json"));
    let temporary = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> AppResult<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(any(macos, linux, android, ios, ohos))]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

/// A damaged or stale file must not hide another healthy running instance.
pub fn read_descriptors(data_dir: &Path) -> AppResult<DescriptorScan> {
    let entries = match fs::read_dir(descriptor_dir(data_dir)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DescriptorScan::default())
        }
        Err(error) => return Err(error.into()),
    };
    let mut scan = DescriptorScan::default();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let filename = entry.file_name().to_string_lossy().into_owned();
        let read = (|| -> AppResult<ControlDescriptor> {
            if !entry.file_type()?.is_file() || entry.metadata()?.len() > MAX_DESCRIPTOR_BYTES {
                return Err(invalid("descriptor must be a small regular file"));
            }
            let mut bytes = Vec::new();
            fs::File::open(&path)?
                .take(MAX_DESCRIPTOR_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_DESCRIPTOR_BYTES {
                return Err(invalid("descriptor is too large"));
            }
            let descriptor: ControlDescriptor =
                serde_json::from_slice(&bytes).map_err(|_| invalid("invalid descriptor JSON"))?;
            descriptor.validate()?;
            if filename != format!("{}.json", descriptor.instance_id) {
                return Err(invalid("descriptor filename does not match its instance"));
            }
            Ok(descriptor)
        })();
        match read {
            Ok(descriptor) => scan.descriptors.push(descriptor),
            Err(error) => scan.ignored.push(format!("{filename}: {}", error.code())),
        }
    }
    scan.descriptors
        .sort_by(|a, b| a.instance_id.cmp(&b.instance_id));
    Ok(scan)
}

pub fn remove_descriptor(data_dir: &Path, instance_id: &str) -> AppResult<()> {
    remove_metadata(&descriptor_dir(data_dir), instance_id)
}

fn remove_metadata(directory: &Path, instance_id: &str) -> AppResult<()> {
    validate_instance(instance_id)?;
    match fs::remove_file(directory.join(format!("{instance_id}.json"))) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(all(test, any(macos, linux, windows)))]
mod tests {
    use super::*;

    fn descriptor() -> ControlDescriptor {
        let instance_id = uuid::Uuid::new_v4().to_string();
        ControlDescriptor {
            version: CONTROL_VERSION,
            endpoint: native_transport().endpoint(&instance_id).unwrap(),
            instance_id,
            pid: 1,
            host: "desktop".into(),
        }
    }

    #[test]
    fn discovery_round_trip_contains_no_credentials_and_removes_only_its_instance() {
        let dir = tempfile::tempdir().unwrap();
        let first = descriptor();
        let second = descriptor();
        publish_descriptor(dir.path(), &first).unwrap();
        publish_descriptor(dir.path(), &second).unwrap();
        let text = fs::read_to_string(
            descriptor_dir(dir.path()).join(format!("{}.json", first.instance_id)),
        )
        .unwrap();
        assert!(!text.contains("token"));
        assert_eq!(read_descriptors(dir.path()).unwrap().descriptors.len(), 2);
        remove_descriptor(dir.path(), &first.instance_id).unwrap();
        remove_descriptor(dir.path(), &first.instance_id).unwrap();
        let scan = read_descriptors(dir.path()).unwrap();
        assert_eq!(scan.descriptors[0].instance_id, second.instance_id);
        assert_eq!(scan.descriptors.len(), 1);
    }

    #[test]
    fn invalid_metadata_does_not_mask_a_valid_instance_or_escape_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let valid = descriptor();
        publish_descriptor(dir.path(), &valid).unwrap();
        fs::write(descriptor_dir(dir.path()).join("bad.json"), b"{broken").unwrap();
        let mut bad = descriptor();
        bad.version += 1;
        assert!(publish_descriptor(dir.path(), &bad).is_err());
        bad.version = CONTROL_VERSION;
        bad.instance_id = "../../outside".into();
        assert!(publish_descriptor(dir.path(), &bad).is_err());
        assert!(remove_descriptor(dir.path(), "../../outside").is_err());
        let scan = read_descriptors(dir.path()).unwrap();
        assert_eq!(scan.descriptors.len(), 1);
        assert_eq!(scan.ignored.len(), 1);
    }

    #[test]
    fn discovery_rejects_legacy_http_and_mismatched_native_endpoints() {
        let dir = tempfile::tempdir().unwrap();
        let valid = descriptor();
        publish_descriptor(dir.path(), &valid).unwrap();
        let legacy_id = uuid::Uuid::new_v4().to_string();
        fs::write(
            descriptor_dir(dir.path()).join(format!("{legacy_id}.json")),
            serde_json::to_vec(&serde_json::json!({
                "version": 1, "instance_id": legacy_id, "pid": 1,
                "port": 12345, "host": "desktop"
            }))
            .unwrap(),
        )
        .unwrap();
        let mut other = descriptor();
        other.endpoint = valid.endpoint.clone();
        assert!(publish_descriptor(dir.path(), &other).is_err());
        fs::write(
            descriptor_dir(dir.path()).join(format!("{}.json", other.instance_id)),
            serde_json::to_vec(&other).unwrap(),
        )
        .unwrap();
        let scan = read_descriptors(dir.path()).unwrap();
        assert_eq!(scan.descriptors.len(), 1);
        assert_eq!(scan.descriptors[0].instance_id, valid.instance_id);
        assert_eq!(scan.ignored.len(), 2);
    }
}
