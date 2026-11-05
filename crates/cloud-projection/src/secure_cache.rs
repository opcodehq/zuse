//! Optional platform credential storage for read-only cached transcript keys.
//! An unavailable/locked store is a cache miss; keys are never written to files.
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
#[cfg(target_os = "macos")]
pub use mac::{clear, read, write};
#[cfg(not(target_os = "macos"))]
pub fn read(_: &str, _: &str) -> Option<Vec<u8>> {
    None
}
#[cfg(not(target_os = "macos"))]
pub fn write(_: &str, _: &str, _: &[u8]) {}
#[cfg(not(target_os = "macos"))]
pub fn clear(_: &str, _: Option<&str>) {}

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
