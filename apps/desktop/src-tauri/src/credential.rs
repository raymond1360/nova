#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Mutex;

use crate::error::{NovaError, NovaResult};

pub trait CredentialStore: Send + Sync {
    fn save_api_key(&self, provider_id: &str, api_key: &str) -> NovaResult<()>;
    fn read_api_key(&self, provider_id: &str) -> NovaResult<Option<String>>;
    fn delete_api_key(&self, provider_id: &str) -> NovaResult<()>;
}

pub struct SystemCredentialStore;

impl SystemCredentialStore {
    pub fn new() -> Self {
        Self
    }
}

fn target_name(provider_id: &str) -> String {
    format!("com.sdkmax.nova/{provider_id}/api-key")
}

#[cfg(windows)]
mod windows_store {
    use std::ffi::OsStr;
    use std::iter;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };

    use super::{target_name, CredentialStore, NovaError, NovaResult, SystemCredentialStore};

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value)
            .encode_wide()
            .chain(iter::once(0))
            .collect()
    }

    fn encode_blob(value: &str) -> Vec<u8> {
        value.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    fn decode_blob(bytes: &[u8]) -> NovaResult<String> {
        if !bytes.len().is_multiple_of(2) {
            return Err(NovaError::Credential("凭据数据格式异常".to_string()));
        }
        let units = bytes
            .chunks(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        String::from_utf16(&units)
            .map_err(|_| NovaError::Credential("凭据数据不是有效 UTF-16".to_string()))
    }

    impl CredentialStore for SystemCredentialStore {
        fn save_api_key(&self, provider_id: &str, api_key: &str) -> NovaResult<()> {
            let target = wide(&target_name(provider_id));
            let username = wide(provider_id);
            let mut blob = encode_blob(api_key);
            let credential = CREDENTIALW {
                Flags: 0,
                Type: CRED_TYPE_GENERIC,
                TargetName: target.as_ptr() as *mut u16,
                Comment: null_mut(),
                LastWritten: FILETIME {
                    dwLowDateTime: 0,
                    dwHighDateTime: 0,
                },
                CredentialBlobSize: blob.len() as u32,
                CredentialBlob: blob.as_mut_ptr(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                AttributeCount: 0,
                Attributes: null_mut(),
                TargetAlias: null_mut(),
                UserName: username.as_ptr() as *mut u16,
            };
            let ok = unsafe { CredWriteW(&credential, 0) };
            if ok == 0 {
                return Err(NovaError::Credential(
                    "Windows Credential Manager 写入失败".to_string(),
                ));
            }
            Ok(())
        }

        fn read_api_key(&self, provider_id: &str) -> NovaResult<Option<String>> {
            let target = wide(&target_name(provider_id));
            let mut credential_ptr: *mut CREDENTIALW = null_mut();
            let ok =
                unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential_ptr) };
            if ok == 0 {
                return Ok(None);
            }
            if credential_ptr.is_null() {
                return Ok(None);
            }
            let credential = unsafe { &*credential_ptr };
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    credential.CredentialBlob,
                    credential.CredentialBlobSize as usize,
                )
            };
            let result = decode_blob(bytes);
            unsafe { CredFree(credential_ptr as *const _) };
            result.map(Some)
        }

        fn delete_api_key(&self, provider_id: &str) -> NovaResult<()> {
            let target = wide(&target_name(provider_id));
            unsafe {
                CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0);
            }
            Ok(())
        }
    }
}

#[cfg(not(windows))]
impl CredentialStore for SystemCredentialStore {
    fn save_api_key(&self, _provider_id: &str, _api_key: &str) -> NovaResult<()> {
        Err(NovaError::Credential(
            "当前平台未实现系统凭据存储".to_string(),
        ))
    }

    fn read_api_key(&self, _provider_id: &str) -> NovaResult<Option<String>> {
        Ok(None)
    }

    fn delete_api_key(&self, _provider_id: &str) -> NovaResult<()> {
        Ok(())
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct MemoryCredentialStore {
    values: Mutex<HashMap<String, String>>,
}

#[cfg(test)]
impl CredentialStore for MemoryCredentialStore {
    fn save_api_key(&self, provider_id: &str, api_key: &str) -> NovaResult<()> {
        self.values
            .lock()
            .map_err(|_| NovaError::Credential("测试凭据锁定失败".to_string()))?
            .insert(target_name(provider_id), api_key.to_string());
        Ok(())
    }

    fn read_api_key(&self, provider_id: &str) -> NovaResult<Option<String>> {
        Ok(self
            .values
            .lock()
            .map_err(|_| NovaError::Credential("测试凭据锁定失败".to_string()))?
            .get(&target_name(provider_id))
            .cloned())
    }

    fn delete_api_key(&self, provider_id: &str) -> NovaResult<()> {
        self.values
            .lock()
            .map_err(|_| NovaError::Credential("测试凭据锁定失败".to_string()))?
            .remove(&target_name(provider_id));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_saves_reads_and_deletes() {
        let store = MemoryCredentialStore::default();
        let fake_key = ["sk", "test-value"].join("-");
        store.save_api_key("sdkmax", &fake_key).unwrap();
        assert_eq!(store.read_api_key("sdkmax").unwrap(), Some(fake_key));
        store.delete_api_key("sdkmax").unwrap();
        assert_eq!(store.read_api_key("sdkmax").unwrap(), None);
    }
}
