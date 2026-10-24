//! HarmonyOS Asset Store Kit backend for the existing master-key envelope.
//!
//! The ABI below is the API 11 subset of <asset/asset_api.h> and
//! <asset/asset_type.h>, available before our minimum SDK 12. Normal assets
//! require no extra permission. We never request uninstall-persistent storage,
//! user authentication, or cross-device/cloud synchronization.

use crate::error::{AppError, AppResult};

#[cfg(ohos)]
use super::SecretStore;

const SUCCESS: i32 = 0;
const NOT_FOUND: i32 = 24_000_002;
const TAG_SECRET: u32 = 0x3000_0001;
const TAG_ALIAS: u32 = 0x3000_0002;
const TAG_ACCESSIBILITY: u32 = 0x2000_0003;
const TAG_AUTH_TYPE: u32 = 0x2000_0005;
const TAG_SYNC_TYPE: u32 = 0x2000_0010;
#[cfg(ohos)]
const TAG_RETURN_TYPE: u32 = 0x2000_0040;
const TAG_CONFLICT_RESOLUTION: u32 = 0x2000_0044;
const DEVICE_FIRST_UNLOCKED: u32 = 1;
const AUTH_NONE: u32 = 0;
const SYNC_THIS_DEVICE: u32 = 1;
#[cfg(ohos)]
const RETURN_ALL: u32 = 0;
const CONFLICT_OVERWRITE: u32 = 0;
const MAX_ALIAS_BYTES: usize = 256;
const MAX_SECRET_BYTES: usize = 1024;

// Asset_Attr.tag is explicitly uint32_t in the SDK, rather than a C enum.
// Native pointers make these structures local to each synchronous call; none
// are stored in the Send + Sync backend or retained past the owning buffers.
#[repr(C)]
#[derive(Clone, Copy)]
struct Blob {
    size: u32,
    data: *mut u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
union Value {
    boolean: bool,
    number: u32,
    blob: Blob,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Attr {
    tag: u32,
    value: Value,
}

impl Attr {
    fn blob(tag: u32, bytes: &mut [u8]) -> Self {
        Self {
            tag,
            value: Value {
                blob: Blob {
                    size: bytes.len() as u32,
                    data: bytes.as_mut_ptr(),
                },
            },
        }
    }

    fn number(tag: u32, number: u32) -> Self {
        Self {
            tag,
            value: Value { number },
        }
    }
}

#[repr(C)]
struct ResultEntry {
    count: u32,
    attrs: *mut Attr,
}

#[repr(C)]
struct ResultSet {
    count: u32,
    results: *mut ResultEntry,
}

#[cfg(ohos)]
#[link(name = "asset_ndk.z")]
extern "C" {
    fn OH_Asset_Add(attributes: *const Attr, attr_cnt: u32) -> i32;
    fn OH_Asset_Remove(query: *const Attr, query_cnt: u32) -> i32;
    fn OH_Asset_Query(query: *const Attr, query_cnt: u32, result_set: *mut ResultSet) -> i32;
    fn OH_Asset_FreeResultSet(result_set: *mut ResultSet);
}

fn error(op: &str, detail: impl std::fmt::Display) -> AppError {
    AppError::other(
        "keyring_failed",
        serde_json::json!({ "op": op, "err": format!("HarmonyOS Asset: {detail}") }),
    )
}

fn alias(key: &str) -> AppResult<Vec<u8>> {
    let alias = format!("{}:{key}", super::SERVICE).into_bytes();
    if key.is_empty() || alias.len() > MAX_ALIAS_BYTES {
        return Err(error(
            "entry",
            "account is empty or its alias exceeds 256 bytes",
        ));
    }
    Ok(alias)
}

fn found(op: &str, code: i32) -> AppResult<bool> {
    match code {
        SUCCESS => Ok(true),
        NOT_FOUND => Ok(false),
        _ => Err(error(op, format_args!("error code {code}"))),
    }
}

fn add_attributes(alias: &mut [u8], secret: &mut [u8]) -> AppResult<[Attr; 6]> {
    if secret.is_empty() || secret.len() > MAX_SECRET_BYTES {
        return Err(error("set", "secret must contain 1 to 1024 bytes"));
    }
    Ok([
        Attr::blob(TAG_ALIAS, alias),
        Attr::blob(TAG_SECRET, secret),
        Attr::number(TAG_ACCESSIBILITY, DEVICE_FIRST_UNLOCKED),
        Attr::number(TAG_AUTH_TYPE, AUTH_NONE),
        // Restore with this device's system backup, without account/cloud or
        // trusted-device cloning. RSSH's own config export re-encrypts secrets.
        Attr::number(TAG_SYNC_TYPE, SYNC_THIS_DEVICE),
        // Official atomic upsert: avoids Add -> DUPLICATED -> Update races.
        Attr::number(TAG_CONFLICT_RESOLUTION, CONFLICT_OVERWRITE),
    ])
}

/// Read one alias's plaintext before the owning native result is freed.
///
/// # Safety
/// Every non-null pointer/count pair must describe live, initialized SDK result
/// memory (or equivalent owned buffers in unit tests). The caller keeps that
/// memory alive throughout this function. Blob fields are read only after
/// checking the secret's bytes-typed tag.
unsafe fn read_secret(result: &ResultSet) -> AppResult<String> {
    if result.count != 1 || result.results.is_null() {
        return Err(error("get", "query returned an invalid number of assets"));
    }
    // SAFETY: The caller owns a live SDK result; one result was checked above.
    let entry = unsafe { &*result.results };
    if entry.count == 0 || entry.attrs.is_null() {
        return Err(error("get", "query returned no attributes"));
    }
    // SAFETY: The SDK result owns entry.count initialized attributes.
    let attrs = unsafe { std::slice::from_raw_parts(entry.attrs, entry.count as usize) };
    let attr = attrs
        .iter()
        .find(|attr| attr.tag == TAG_SECRET)
        .ok_or_else(|| error("get", "query did not return the secret"))?;
    // SAFETY: TAG_SECRET's defined union member is Asset_Blob.
    let blob = unsafe { attr.value.blob };
    if blob.size == 0 || blob.size as usize > MAX_SECRET_BYTES || blob.data.is_null() {
        return Err(error("get", "query returned an invalid secret buffer"));
    }
    // SAFETY: The SDK owns blob.size bytes until ResultSet is freed. Copy to
    // Rust-owned storage; no native pointer escapes this function.
    let bytes = unsafe { std::slice::from_raw_parts(blob.data, blob.size as usize) };
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| error("get", "stored value is not UTF-8"))
}

#[cfg(ohos)]
struct OwnedResult(ResultSet);

#[cfg(ohos)]
impl Drop for OwnedResult {
    fn drop(&mut self) {
        // SAFETY: This zero-initialized result was passed to OH_Asset_Query.
        // The SDK frees its allocations on both success and error. This owner
        // is never copied, so the result is released exactly once.
        unsafe { OH_Asset_FreeResultSet(&mut self.0) };
    }
}

#[cfg(ohos)]
pub struct AssetStore;

#[cfg(ohos)]
pub fn try_open() -> Option<AssetStore> {
    let store = AssetStore;
    // Separate concurrent probes so one process cannot delete another's entry.
    let key = format!("__rssh_probe__:{}", uuid::Uuid::new_v4());
    let read = store.set(&key, "ok").and_then(|()| store.get(&key));
    let deleted = store.delete(&key);
    if matches!(read, Ok(Some(value)) if value == "ok") && deleted.is_ok() {
        Some(store)
    } else {
        None
    }
}

#[cfg(ohos)]
impl SecretStore for AssetStore {
    fn get(&self, key: &str) -> AppResult<Option<String>> {
        let mut alias = alias(key)?;
        let query = [
            Attr::blob(TAG_ALIAS, &mut alias),
            Attr::number(TAG_RETURN_TYPE, RETURN_ALL),
        ];
        let mut result = OwnedResult(ResultSet {
            count: 0,
            results: std::ptr::null_mut(),
        });
        // SAFETY: Query and alias buffers remain live for this synchronous API;
        // result is writable, initialized, and owned by the RAII guard.
        let code = unsafe { OH_Asset_Query(query.as_ptr(), query.len() as u32, &mut result.0) };
        if !found("get", code)? {
            return Ok(None);
        }
        // SAFETY: The query succeeded and the guard owns all returned buffers.
        unsafe { read_secret(&result.0) }.map(Some)
    }

    fn set(&self, key: &str, value: &str) -> AppResult<()> {
        let mut alias = alias(key)?;
        let mut secret = zeroize::Zeroizing::new(value.as_bytes().to_vec());
        let attrs = add_attributes(&mut alias, &mut secret)?;
        // SAFETY: All attributes and backing buffers remain live and writable
        // for this synchronous call. The SDK copies the bytes into its store.
        let code = unsafe { OH_Asset_Add(attrs.as_ptr(), attrs.len() as u32) };
        if code != SUCCESS {
            return Err(error("set", format_args!("error code {code}")));
        }
        Ok(())
    }

    fn delete(&self, key: &str) -> AppResult<()> {
        let mut alias = alias(key)?;
        let query = [Attr::blob(TAG_ALIAS, &mut alias)];
        // SAFETY: Alias/query buffers remain live for this synchronous call.
        let code = unsafe { OH_Asset_Remove(query.as_ptr(), query.len() as u32) };
        found("delete", code).map(|_| ())
    }

    fn backend_name(&self) -> &'static str {
        "harmonyos-asset-store"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_separate_service_and_accounts_and_enforce_sdk_limit() {
        assert_eq!(
            alias("rssh_master_key_v1").unwrap(),
            b"rssh:rssh_master_key_v1"
        );
        assert_ne!(
            alias("cred:one:secret").unwrap(),
            alias("cred:two:secret").unwrap()
        );
        assert!(alias("").is_err());
        assert!(alias(&"x".repeat(251)).is_ok());
        assert!(alias(&"x".repeat(252)).is_err());
    }

    #[test]
    fn only_missing_assets_are_treated_as_absent() {
        assert!(found("get", SUCCESS).unwrap());
        assert!(!found("get", NOT_FOUND).unwrap());
        assert!(!found("delete", NOT_FOUND).unwrap());
        for code in [201, 401, 24_000_001, 24_000_003, 24_000_004, 24_000_005] {
            assert_eq!(found("get", code).unwrap_err().code(), "keyring_failed");
        }
    }

    #[test]
    fn upsert_uses_atomic_overwrite_and_only_this_device_backup() {
        let mut alias = alias("master").unwrap();
        let mut secret = b"base64-master-key".to_vec();
        let attrs = add_attributes(&mut alias, &mut secret).unwrap();
        for (tag, expected) in [
            (TAG_CONFLICT_RESOLUTION, CONFLICT_OVERWRITE),
            (TAG_SYNC_TYPE, SYNC_THIS_DEVICE),
            (TAG_ACCESSIBILITY, DEVICE_FIRST_UNLOCKED),
            (TAG_AUTH_TYPE, AUTH_NONE),
        ] {
            let attr = attrs.iter().find(|attr| attr.tag == tag).unwrap();
            // SAFETY: Each selected attribute was constructed as a number.
            assert_eq!(unsafe { attr.value.number }, expected);
        }
        assert!(add_attributes(&mut alias, &mut []).is_err());
        assert!(add_attributes(&mut alias, &mut [0; 1025]).is_err());
    }

    #[test]
    fn native_result_is_copied_and_malformed_results_fail_closed() {
        let mut bytes = b"stored-secret".to_vec();
        let mut attrs = [Attr::blob(TAG_SECRET, &mut bytes)];
        let mut entry = ResultEntry {
            count: 1,
            attrs: attrs.as_mut_ptr(),
        };
        let mut result = ResultSet {
            count: 1,
            results: &mut entry,
        };
        // SAFETY: All pointers refer to live owned buffers above.
        let value = unsafe { read_secret(&result) }.unwrap();
        bytes.fill(b'x');
        assert_eq!(value, "stored-secret");
        attrs[0].tag = TAG_ALIAS;
        // SAFETY: Same live buffers; the secret tag is deliberately absent.
        assert!(unsafe { read_secret(&result) }.is_err());
        attrs[0] = Attr::blob(TAG_SECRET, &mut bytes);
        bytes[0] = 0xff;
        // SAFETY: Same live buffers; the data is deliberately invalid UTF-8.
        assert!(unsafe { read_secret(&result) }.is_err());
        result.count = 0;
        // SAFETY: No pointer is dereferenced when count is invalid.
        assert!(unsafe { read_secret(&result) }.is_err());
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn ffi_layout_matches_the_64_bit_sdk_structures() {
        assert_eq!(std::mem::size_of::<Blob>(), 16);
        assert_eq!(std::mem::size_of::<Value>(), 16);
        assert_eq!(std::mem::size_of::<Attr>(), 24);
        assert_eq!(std::mem::offset_of!(Attr, value), 8);
        assert_eq!(std::mem::size_of::<ResultEntry>(), 16);
        assert_eq!(std::mem::size_of::<ResultSet>(), 16);
    }
}
