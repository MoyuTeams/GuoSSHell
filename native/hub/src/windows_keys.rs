//! Windows 私钥使用当前用户的 DPAPI 加密后持久化，支持超过凭证管理器单条上限的私钥。
//! SSH 密码仍由上游的 Windows 凭证管理器保存；私钥和口令不会写成明文文件。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use zeroize::{Zeroize, Zeroizing};

use crate::keys::{Item, KeyStore, StoreError};

const MAGIC: &[u8] = b"GUOSSDP1";

pub struct WindowsKeyStore {
    root: PathBuf,
}

fn failure(error: impl std::fmt::Display) -> StoreError {
    StoreError(error.to_string())
}

impl WindowsKeyStore {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        for item in [Item::Key, Item::Passphrase] {
            std::fs::create_dir_all(root.join(item.service()))
                .map_err(|error| error.to_string())?;
        }
        Ok(Self { root })
    }

    fn path(&self, item: Item, id: &str, synchronized: bool) -> Result<PathBuf, StoreError> {
        if synchronized {
            return Err(failure("Windows 不支持 iCloud 同步"));
        }
        let id = uuid::Uuid::parse_str(id).map_err(failure)?;
        Ok(self.root.join(item.service()).join(format!("{id}.dpapi")))
    }

    fn entropy(item: Item, path: &Path) -> Result<String, StoreError> {
        let name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or_else(|| failure("无效私钥标识"))?;
        Ok(format!("GuoSSHell:{}:{name}", item.service()))
    }
}

struct Output(CRYPT_INTEGER_BLOB);

impl Drop for Output {
    fn drop(&mut self) {
        if !self.0.pbData.is_null() {
            // DPAPI 返回的缓冲区属于 LocalAlloc；清零后必须交回 LocalFree。
            unsafe {
                std::slice::from_raw_parts_mut(self.0.pbData, self.0.cbData as usize).zeroize();
                LocalFree(self.0.pbData.cast());
            }
        }
    }
}

fn crypt(data: &[u8], entropy: &[u8], decrypt: bool) -> Result<Zeroizing<Vec<u8>>, StoreError> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len().try_into().map_err(failure)?,
        pbData: data.as_ptr().cast_mut(),
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: entropy.len().try_into().map_err(failure)?,
        pbData: entropy.as_ptr().cast_mut(),
    };
    let mut output = Output(CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    });
    // 两个输入缓冲区在调用期间有效且不被修改；输出由上面的 RAII 对象管理。
    let succeeded = unsafe {
        if decrypt {
            CryptUnprotectData(
                &input,
                null_mut(),
                &entropy,
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output.0,
            )
        } else {
            CryptProtectData(
                &input,
                null(),
                &entropy,
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output.0,
            )
        }
    };
    if succeeded == 0 {
        return Err(failure(std::io::Error::last_os_error()));
    }
    if output.0.cbData == 0 {
        return Ok(Zeroizing::new(Vec::new()));
    }
    if output.0.pbData.is_null() {
        return Err(failure("DPAPI 返回空缓冲区"));
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.0.pbData, output.0.cbData as usize) };
    Ok(Zeroizing::new(bytes.to_vec()))
}

impl KeyStore for WindowsKeyStore {
    fn put(
        &self,
        item: Item,
        id: &str,
        secret: &[u8],
        synchronized: bool,
    ) -> Result<(), StoreError> {
        let path = self.path(item, id, synchronized)?;
        let encrypted = crypt(secret, Self::entropy(item, &path)?.as_bytes(), false)?;
        let temporary = path.with_extension("tmp");
        let mut output = std::fs::File::create(&temporary).map_err(failure)?;
        output.write_all(MAGIC).map_err(failure)?;
        output.write_all(&encrypted).map_err(failure)?;
        output.sync_all().map_err(failure)?;
        drop(output);
        std::fs::rename(&temporary, &path).map_err(failure)
    }

    fn get_in(
        &self,
        item: Item,
        id: &str,
        synchronized: bool,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
        let path = self.path(item, id, synchronized)?;
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(failure(error)),
        };
        let encrypted = bytes
            .strip_prefix(MAGIC)
            .ok_or_else(|| failure("不支持的私钥存储格式"))?;
        crypt(encrypted, Self::entropy(item, &path)?.as_bytes(), true).map(Some)
    }

    fn delete(&self, item: Item, id: &str, synchronized: bool) -> Result<(), StoreError> {
        match std::fs::remove_file(self.path(item, id, synchronized)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(failure(error)),
        }
    }

    fn list(&self, item: Item, synchronized: bool) -> Result<Vec<String>, StoreError> {
        if synchronized {
            return Err(failure("Windows 不支持 iCloud 同步"));
        }
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(self.root.join(item.service())).map_err(failure)? {
            let path = entry.map_err(failure)?.path();
            if path.extension().is_some_and(|value| value == "dpapi") {
                let id = path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| failure("私钥文件名无效"))?;
                ids.push(uuid::Uuid::parse_str(id).map_err(failure)?.to_string());
            }
        }
        Ok(ids)
    }

    fn sync_available(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::WindowsKeyStore;
    use crate::keys::{Item, KeyStore};

    #[test]
    fn dpapi_preserves_large_keys_and_rejects_tampering_and_other_ids() {
        let root = std::env::temp_dir().join(format!("guosshell-dpapi-{}", uuid::Uuid::new_v4()));
        let store = WindowsKeyStore::new(root.clone()).expect("测试存储");
        let id = uuid::Uuid::new_v4().to_string();
        let other = uuid::Uuid::new_v4().to_string();
        let secret = vec![42; 64 * 1024];
        store
            .put(Item::Key, &id, &secret, false)
            .expect("保存大私钥");
        assert_eq!(
            *store
                .get_in(Item::Key, &id, false)
                .expect("读取")
                .expect("存在"),
            secret
        );
        let path = store.path(Item::Key, &id, false).expect("路径");
        std::fs::copy(
            &path,
            store.path(Item::Key, &other, false).expect("其他路径"),
        )
        .expect("交换密文");
        assert!(store.get_in(Item::Key, &other, false).is_err());
        let mut bytes = std::fs::read(&path).expect("加密文件");
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(&path, bytes).expect("篡改密文");
        assert!(store.get_in(Item::Key, &id, false).is_err());
        assert!(store.put(Item::Key, "../escape", b"x", false).is_err());
        store
            .put(Item::Key, &id, b"replacement", false)
            .expect("原子替换");
        store.delete(Item::Key, &id, false).expect("删除");
        store.delete(Item::Key, &id, false).expect("幂等删除");
        std::fs::remove_dir_all(root).expect("清理测试存储");
    }
}
