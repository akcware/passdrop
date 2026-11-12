//! `keychain` sink: a generic password in the macOS login keychain.
//!
//! Existence is checked with an attributes-only search, which never triggers a
//! keychain access prompt. The value is read back only for items passdrop just created
//! (passdrop is then in the item's access list, so no prompt either).

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Mutex;

use zeroize::Zeroizing;

use super::{SinkError, WriteOutcome};

pub trait KeychainBackend: Send + Sync + Debug {
    /// Does an item exist? Must not read the secret.
    fn exists(&self, service: &str, account: &str) -> Result<bool, String>;
    fn get(&self, service: &str, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String>;
    fn set(&self, service: &str, account: &str, value: &[u8]) -> Result<(), String>;
}

#[derive(Debug, Default)]
pub struct NativeKeychain;

#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

#[cfg(target_os = "macos")]
impl KeychainBackend for NativeKeychain {
    fn exists(&self, service: &str, account: &str) -> Result<bool, String> {
        use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
        let found = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(service)
            .account(account)
            .load_attributes(true)
            .limit(Limit::Max(1))
            .search();
        match found {
            Ok(items) => Ok(!items.is_empty()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    }

    fn get(&self, service: &str, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        match security_framework::passwords::get_generic_password(service, account) {
            Ok(v) => Ok(Some(Zeroizing::new(v))),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, service: &str, account: &str, value: &[u8]) -> Result<(), String> {
        security_framework::passwords::set_generic_password(service, account, value).map_err(|e| e.to_string())
    }
}

#[cfg(not(target_os = "macos"))]
impl KeychainBackend for NativeKeychain {
    fn exists(&self, _: &str, _: &str) -> Result<bool, String> {
        Err("keychain sinks are only supported on macOS in v1".into())
    }
    fn get(&self, _: &str, _: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        Err("keychain sinks are only supported on macOS in v1".into())
    }
    fn set(&self, _: &str, _: &str, _: &[u8]) -> Result<(), String> {
        Err("keychain sinks are only supported on macOS in v1".into())
    }
}

/// In-memory keychain for tests, so they never touch the user's real keychain.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct MemoryKeychain {
    items: Mutex<HashMap<(String, String), Vec<u8>>>,
}

impl KeychainBackend for MemoryKeychain {
    fn exists(&self, service: &str, account: &str) -> Result<bool, String> {
        Ok(self.items.lock().unwrap().contains_key(&(service.into(), account.into())))
    }
    fn get(&self, service: &str, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        Ok(self.items.lock().unwrap().get(&(service.into(), account.into())).cloned().map(Zeroizing::new))
    }
    fn set(&self, service: &str, account: &str, value: &[u8]) -> Result<(), String> {
        self.items.lock().unwrap().insert((service.into(), account.into()), value.to_vec());
        Ok(())
    }
}

fn kc_err(message: String) -> SinkError {
    SinkError::Tool { tool: "keychain", message }
}

pub fn is_set(backend: &dyn KeychainBackend, service: &str, account: &str) -> Result<bool, SinkError> {
    backend.exists(service, account).map_err(kc_err)
}

/// Returns the outcome and whether the write was verified by reading it back.
pub fn write(
    backend: &dyn KeychainBackend,
    service: &str,
    account: &str,
    value: &[u8],
    overwrite: bool,
) -> Result<(WriteOutcome, bool), SinkError> {
    let existed = backend.exists(service, account).map_err(kc_err)?;
    if existed && !overwrite {
        return Err(SinkError::Exists);
    }
    backend.set(service, account, value).map_err(kc_err)?;
    if existed {
        // Reading an item created elsewhere could show a keychain prompt; skip it.
        return Ok((WriteOutcome::Updated, false));
    }
    match backend.get(service, account).map_err(kc_err)? {
        Some(back) if back.as_slice() == value => Ok((WriteOutcome::Created, true)),
        _ => Err(SinkError::VerifyFailed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_refuse_then_replace() {
        let kc = MemoryKeychain::default();
        assert_eq!(write(&kc, "svc", "acct", b"v1", false).unwrap(), (WriteOutcome::Created, true));
        assert_eq!(write(&kc, "svc", "acct", b"v2", false).unwrap_err().code(), "exists");
        assert_eq!(write(&kc, "svc", "acct", b"v2", true).unwrap(), (WriteOutcome::Updated, false));
        assert_eq!(kc.get("svc", "acct").unwrap().unwrap().as_slice(), b"v2");
        assert!(is_set(&kc, "svc", "acct").unwrap());
        assert!(!is_set(&kc, "svc", "other").unwrap());
    }
}
