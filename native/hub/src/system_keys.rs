//! Android、Linux 使用系统凭证存储；不提供 iCloud 同步或明文回退。

use std::collections::HashMap;
use std::sync::Arc;

use keyring_core::{CredentialStore, Entry, Error};
use zeroize::Zeroizing;

use crate::keys::{Item, KeyStore, StoreError};

pub struct SystemKeyStore {
    store: Arc<CredentialStore>,
}

impl SystemKeyStore {
    #[cfg(any(target_os = "android", target_os = "linux"))]
    pub fn new() -> Result<Self, String> {
        // 只创建条目句柄以初始化默认后端，不写探测凭证。
        let _ = keyring::Entry::new("guosshell.initialization", "store")
            .map_err(|error| error.to_string())?;
        let store =
            keyring_core::get_default_store().ok_or_else(|| "系统凭证存储不可用".to_owned())?;
        Ok(Self { store })
    }

    fn entry(&self, item: Item, id: &str, synchronized: bool) -> Result<Entry, StoreError> {
        if synchronized {
            return Err(StoreError("此平台不支持 iCloud 同步".to_owned()));
        }
        self.store.build(item.service(), id, None).map_err(failure)
    }
}

fn failure(error: Error) -> StoreError {
    StoreError(error.to_string())
}

impl KeyStore for SystemKeyStore {
    fn put(
        &self,
        item: Item,
        id: &str,
        secret: &[u8],
        synchronized: bool,
    ) -> Result<(), StoreError> {
        self.entry(item, id, synchronized)?
            .set_secret(secret)
            .map_err(failure)
    }

    fn get_in(
        &self,
        item: Item,
        id: &str,
        synchronized: bool,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
        match self.entry(item, id, synchronized)?.get_secret() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(Error::NoEntry) => Ok(None),
            Err(error) => Err(failure(error)),
        }
    }

    fn delete(&self, item: Item, id: &str, synchronized: bool) -> Result<(), StoreError> {
        match self.entry(item, id, synchronized)?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(error) => Err(failure(error)),
        }
    }

    fn list(&self, item: Item, synchronized: bool) -> Result<Vec<String>, StoreError> {
        if synchronized {
            return Err(StoreError("此平台不支持 iCloud 同步".to_owned()));
        }
        let spec = HashMap::from([("service", item.service())]);
        let entries = self.store.search(&spec).map_err(failure)?;
        Ok(entries
            .iter()
            .filter_map(|entry| entry.get_specifiers())
            .filter(|(service, _)| service == item.service())
            .map(|(_, id)| id)
            .collect())
    }

    fn sync_available(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::SystemKeyStore;
    use crate::keys::{Item, KeyStore};

    #[test]
    fn system_store_preserves_binary_keys_and_separates_passphrases() {
        let store = SystemKeyStore {
            store: keyring_core::sample::Store::new().expect("内存系统存储"),
        };
        let bytes = [0, 255, 1, 2];
        store
            .put(Item::Key, "fixture", &bytes, false)
            .expect("存私钥");
        store
            .put(Item::Passphrase, "fixture", b"passphrase", false)
            .expect("存口令");
        assert_eq!(
            store
                .get_in(Item::Key, "fixture", false)
                .expect("读私钥")
                .expect("存在")
                .as_slice(),
            bytes
        );
        assert_eq!(store.list(Item::Key, false).expect("私钥列表"), ["fixture"]);
        store.delete(Item::Key, "fixture", false).expect("删除");
        store.delete(Item::Key, "fixture", false).expect("幂等删除");
        assert!(
            store
                .get_in(Item::Key, "fixture", false)
                .expect("已删除")
                .is_none()
        );
        assert!(
            store
                .get_in(Item::Passphrase, "fixture", false)
                .expect("口令独立")
                .is_some()
        );
        assert!(!store.sync_available());
        assert!(store.put(Item::Key, "cloud", &bytes, true).is_err());
        assert!(store.list(Item::Key, true).is_err());
    }
}
