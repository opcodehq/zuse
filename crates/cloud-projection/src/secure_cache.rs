//! Storage for the keys of cached cloud transcripts (and small cached
//! metadata). The macOS data-protection Keychain is used when the app is
//! entitled to it. Builds without that entitlement (every unsigned or
//! unprovisioned build: SecItem calls fail with errSecMissingEntitlement) and
//! other platforms keep them in a private per-user directory instead, the
//! same protection local chats have on disk. Without this the encrypted page
//! cache could never be opened again, and every cloud chat loaded blank.
#[cfg(any(target_os = "macos", test))]
mod mac {
    use std::{
        ffi::{CString, c_void},
        ptr,
    };
    type Ref = *const c_void;
    #[cfg_attr(target_os = "macos", link(name = "CoreFoundation", kind = "framework"))]
    unsafe extern "C" {
        fn CFStringCreateWithCString(allocator: Ref, text: *const i8, encoding: u32) -> Ref;
        fn CFDataCreate(allocator: Ref, bytes: *const u8, length: isize) -> Ref;
        fn CFDataGetLength(value: Ref) -> isize;
        fn CFDataGetBytePtr(value: Ref) -> *const u8;
        fn CFDictionaryCreate(
            allocator: Ref,
            keys: *const Ref,
            values: *const Ref,
            count: isize,
            key_callbacks: Ref,
            value_callbacks: Ref,
        ) -> Ref;
        fn CFRelease(value: Ref);
        static kCFBooleanTrue: Ref;
    }
    #[cfg_attr(target_os = "macos", link(name = "Security", kind = "framework"))]
    unsafe extern "C" {
        static kSecClass: Ref;
        static kSecClassGenericPassword: Ref;
        static kSecAttrService: Ref;
        static kSecAttrAccount: Ref;
        static kSecValueData: Ref;
        static kSecReturnData: Ref;
        static kSecMatchLimit: Ref;
        static kSecMatchLimitOne: Ref;
        static kSecUseDataProtectionKeychain: Ref;
        static kSecUseAuthenticationUI: Ref;
        static kSecUseAuthenticationUIFail: Ref;
        static kSecAttrAccessible: Ref;
        static kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly: Ref;
        fn SecItemCopyMatching(query: Ref, result: *mut Ref) -> i32;
        fn SecItemAdd(attributes: Ref, result: *mut Ref) -> i32;
        fn SecItemUpdate(query: Ref, attributes: Ref) -> i32;
        fn SecItemDelete(query: Ref) -> i32;
    }
    struct Owned(Ref);
    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) }
            }
        }
    }
    fn string(value: &str) -> Option<Owned> {
        let value = CString::new(value).ok()?;
        let result = unsafe { CFStringCreateWithCString(ptr::null(), value.as_ptr(), 0x08000100) };
        (!result.is_null()).then_some(Owned(result))
    }
    fn dictionary(entries: &[(Ref, Ref)]) -> Option<Owned> {
        let keys: Vec<_> = entries.iter().map(|v| v.0).collect();
        let values: Vec<_> = entries.iter().map(|v| v.1).collect();
        let value = unsafe {
            CFDictionaryCreate(
                ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                entries.len() as isize,
                ptr::null(),
                ptr::null(),
            )
        };
        (!value.is_null()).then_some(Owned(value))
    }
    fn base(service: Ref, account: Option<Ref>) -> Vec<(Ref, Ref)> {
        unsafe {
            let mut fields = vec![
                (kSecClass, kSecClassGenericPassword),
                (kSecAttrService, service),
                // Never query legacy login-keychain items: their executable
                // ACL can prompt after a rebuild. An unavailable modern store
                // is an optional-cache miss; there is no legacy fallback.
                (kSecUseDataProtectionKeychain, kCFBooleanTrue),
                (kSecUseAuthenticationUI, kSecUseAuthenticationUIFail),
            ];
            if let Some(account) = account {
                fields.push((kSecAttrAccount, account));
            }
            fields
        }
    }
    /// Whether this process may use the data-protection Keychain at all.
    pub fn available() -> bool {
        static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *AVAILABLE.get_or_init(|| {
            let Some(service) = string("sh.zuse.keychain-probe") else {
                return false;
            };
            let mut fields = base(service.0, None);
            unsafe {
                fields.extend([
                    (kSecReturnData, kCFBooleanTrue),
                    (kSecMatchLimit, kSecMatchLimitOne),
                ]);
            }
            let Some(query) = dictionary(&fields) else {
                return false;
            };
            let mut result = ptr::null();
            let status = unsafe { SecItemCopyMatching(query.0, &mut result) };
            if !result.is_null() {
                drop(Owned(result));
            }
            // errSecMissingEntitlement: the process is not provisioned for it.
            status != -34018
        })
    }
    pub fn read(service: &str, account: &str) -> Option<Vec<u8>> {
        let service = string(service)?;
        let account = string(account)?;
        let mut fields = base(service.0, Some(account.0));
        unsafe {
            fields.extend([
                (kSecReturnData, kCFBooleanTrue),
                (kSecMatchLimit, kSecMatchLimitOne),
            ]);
        }
        let query = dictionary(&fields)?;
        let mut result = ptr::null();
        if unsafe { SecItemCopyMatching(query.0, &mut result) } != 0 || result.is_null() {
            return None;
        }
        let result = Owned(result);
        let length = unsafe { CFDataGetLength(result.0) };
        if !(0..=65536).contains(&length) {
            return None;
        }
        if length == 0 {
            return Some(Vec::new());
        }
        Some(
            unsafe { std::slice::from_raw_parts(CFDataGetBytePtr(result.0), length as usize) }
                .to_vec(),
        )
    }
    pub fn write(service: &str, account: &str, bytes: &[u8]) {
        if bytes.len() > 65536 {
            return;
        }
        let Some(service) = string(service) else {
            return;
        };
        let Some(account) = string(account) else {
            return;
        };
        let value =
            Owned(unsafe { CFDataCreate(ptr::null(), bytes.as_ptr(), bytes.len() as isize) });
        if value.0.is_null() {
            return;
        }
        let Some(query) = dictionary(&base(service.0, Some(account.0))) else {
            return;
        };
        let Some(update) = dictionary(&[unsafe { (kSecValueData, value.0) }]) else {
            return;
        };
        let status = unsafe { SecItemUpdate(query.0, update.0) };
        if status == -25300 {
            let mut fields = base(service.0, Some(account.0));
            unsafe {
                fields.extend([
                    (kSecValueData, value.0),
                    (
                        kSecAttrAccessible,
                        kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
                    ),
                ]);
            }
            if let Some(attributes) = dictionary(&fields) {
                unsafe {
                    SecItemAdd(attributes.0, ptr::null_mut());
                }
            }
        }
    }
    pub fn clear(service: &str, account: Option<&str>) {
        let Some(service) = string(service) else {
            return;
        };
        let account = account.and_then(string);
        if let Some(query) = dictionary(&base(service.0, account.as_ref().map(|v| v.0))) {
            unsafe {
                SecItemDelete(query.0);
            }
        }
    }
}
/// The per-user fallback store: one directory per service, one file per
/// account, both named by digest. Owner-only permissions on Unix.
mod file {
    use std::path::PathBuf;
    fn root() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("ZUSE_SECURE_CACHE_DIR") {
            return Some(dir.into());
        }
        #[cfg(test)]
        {
            Some(std::env::temp_dir().join(format!("zuse-secure-cache-{}", std::process::id())))
        }
        #[cfg(not(test))]
        {
            let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
            Some(PathBuf::from(home).join(".zuse/cache/keys"))
        }
    }
    fn service_dir(service: &str) -> Option<PathBuf> {
        Some(root()?.join(crate::digest(service.as_bytes())))
    }
    fn path(service: &str, account: &str) -> Option<PathBuf> {
        Some(service_dir(service)?.join(crate::digest(account.as_bytes())))
    }
    fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for dir in [dir, dir.parent().unwrap_or(dir)] {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(())
    }
    pub fn read(service: &str, account: &str) -> Option<Vec<u8>> {
        let bytes = std::fs::read(path(service, account)?).ok()?;
        (bytes.len() <= 65536).then_some(bytes)
    }
    pub fn write(service: &str, account: &str, bytes: &[u8]) {
        if bytes.len() > 65536 {
            return;
        }
        let (Some(dir), Some(path)) = (service_dir(service), path(service, account)) else {
            return;
        };
        if private_dir(&dir).is_err() {
            return;
        }
        // Write a private temporary file, then rename it into place.
        let temporary = dir.join(format!(".{}.tmp", uuid_like()));
        let written = {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options.open(&temporary).and_then(|mut file| {
                use std::io::Write;
                file.write_all(bytes)?;
                file.sync_all()
            })
        };
        if written.and_then(|()| std::fs::rename(&temporary, &path)).is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
    }
    pub fn clear(service: &str, account: Option<&str>) {
        match account {
            Some(account) => {
                if let Some(path) = path(service, account) {
                    let _ = std::fs::remove_file(path);
                }
            }
            None => {
                if let Some(dir) = service_dir(service) {
                    let _ = std::fs::remove_dir_all(dir);
                }
            }
        }
    }
    fn uuid_like() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        format!(
            "{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|v| v.as_nanos())
                .unwrap_or(0),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }
}

#[cfg(target_os = "macos")]
pub fn read(service: &str, account: &str) -> Option<Vec<u8>> {
    if mac::available() {
        mac::read(service, account)
    } else {
        file::read(service, account)
    }
}
#[cfg(target_os = "macos")]
pub fn write(service: &str, account: &str, bytes: &[u8]) {
    if mac::available() {
        mac::write(service, account, bytes)
    } else {
        file::write(service, account, bytes)
    }
}
#[cfg(target_os = "macos")]
pub fn clear(service: &str, account: Option<&str>) {
    if mac::available() {
        mac::clear(service, account)
    } else {
        file::clear(service, account)
    }
}
#[cfg(not(target_os = "macos"))]
pub use file::{clear, read, write};

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    /// This runs on macOS instead of treating an optional Keychain miss as proof
    /// that encrypted cached heads can be opened after the app process exits.
    #[test]
    fn macos_keychain_roundtrip_is_scoped_updated_and_cleared() {
        let service = format!(
            "sh.zuse.cloud.cache-canary.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                clear(&self.0, None);
            }
        }
        let _cleanup = Cleanup(service.clone());
        assert!(read(&service, "account-a").is_none());
        write(&service, "account-a", b"synthetic-cached-key-first");
        assert_eq!(
            read(&service, "account-a").as_deref(),
            Some(b"synthetic-cached-key-first".as_slice()),
            "macOS Keychain write/read must work for the provisional cache gate"
        );
        assert!(read(&service, "account-b").is_none());
        write(&service, "account-a", b"synthetic-cached-key-rotated");
        assert_eq!(
            read(&service, "account-a").as_deref(),
            Some(b"synthetic-cached-key-rotated".as_slice())
        );
        write(&service, "account-b", b"synthetic-other-account");
        clear(&service, Some("account-a"));
        assert!(read(&service, "account-a").is_none());
        assert_eq!(
            read(&service, "account-b").as_deref(),
            Some(b"synthetic-other-account".as_slice())
        );
        clear(&service, None);
        assert!(read(&service, "account-b").is_none());
    }
}

#[cfg(all(test, not(target_os = "macos")))]
#[path = "secure_cache_tests.rs"]
mod ffi_tests;

#[cfg(test)]
mod file_store_tests {
    use super::file;
    #[test]
    fn keys_roundtrip_per_account_and_clear_by_account_or_service() {
        let service = format!("sh.zuse.file-store-test.{}", std::process::id());
        assert!(file::read(&service, "a").is_none());
        file::write(&service, "a", b"first");
        file::write(&service, "b", b"other");
        assert_eq!(file::read(&service, "a").as_deref(), Some(b"first".as_slice()));
        file::write(&service, "a", b"rotated");
        assert_eq!(file::read(&service, "a").as_deref(), Some(b"rotated".as_slice()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let root = std::env::temp_dir().join(format!("zuse-secure-cache-{}", std::process::id()));
            let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "the key store is private to its user");
        }
        file::clear(&service, Some("a"));
        assert!(file::read(&service, "a").is_none());
        assert_eq!(file::read(&service, "b").as_deref(), Some(b"other".as_slice()));
        file::clear(&service, None);
        assert!(file::read(&service, "b").is_none());
    }
}
