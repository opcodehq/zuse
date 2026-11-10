//! Exercise the actual macOS query builder on non-macOS CI. These shims record
//! Security.framework calls; they do not claim to emulate macOS authorization.
use std::{
    collections::HashMap,
    ffi::c_void,
    ptr,
    sync::{
        Mutex,
        atomic::{AtomicI32, AtomicUsize, Ordering},
    },
};
type Ref = *const c_void;
static NEXT: AtomicUsize = AtomicUsize::new(4096);
static DICTIONARIES: Mutex<Option<HashMap<usize, Vec<(usize, usize)>>>> = Mutex::new(None);
static CALLS: Mutex<Vec<(&'static str, Vec<(usize, usize)>)>> = Mutex::new(Vec::new());
static UPDATE_STATUS: AtomicI32 = AtomicI32::new(-25300);
fn object() -> Ref {
    NEXT.fetch_add(1, Ordering::Relaxed) as Ref
}
macro_rules! symbol {
    ($name:ident, $value:expr) => {
        #[unsafe(no_mangle)]
        static $name: usize = $value;
    };
}
symbol!(kCFBooleanTrue, 1);
symbol!(kSecClass, 2);
symbol!(kSecClassGenericPassword, 3);
symbol!(kSecAttrService, 4);
symbol!(kSecAttrAccount, 5);
symbol!(kSecValueData, 6);
symbol!(kSecReturnData, 7);
symbol!(kSecMatchLimit, 8);
symbol!(kSecMatchLimitOne, 9);
symbol!(kSecUseAuthenticationUI, 10);
symbol!(kSecUseAuthenticationUIFail, 11);
symbol!(kSecAttrAccessible, 12);
symbol!(kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly, 13);
symbol!(kSecUseDataProtectionKeychain, 14);
#[unsafe(no_mangle)]
extern "C" fn CFStringCreateWithCString(_: Ref, _: *const i8, _: u32) -> Ref {
    object()
}
#[unsafe(no_mangle)]
extern "C" fn CFDataCreate(_: Ref, _: *const u8, _: isize) -> Ref {
    object()
}
#[unsafe(no_mangle)]
extern "C" fn CFDataGetLength(_: Ref) -> isize {
    0
}
#[unsafe(no_mangle)]
extern "C" fn CFDataGetBytePtr(_: Ref) -> *const u8 {
    ptr::null()
}
#[unsafe(no_mangle)]
unsafe extern "C" fn CFDictionaryCreate(
    _: Ref,
    keys: *const Ref,
    values: *const Ref,
    count: isize,
    _: Ref,
    _: Ref,
) -> Ref {
    let keys = unsafe { std::slice::from_raw_parts(keys, count as usize) };
    let values = unsafe { std::slice::from_raw_parts(values, count as usize) };
    let entries = keys
        .iter()
        .zip(values)
        .map(|(k, v)| (*k as usize, *v as usize))
        .collect();
    let result = object();
    DICTIONARIES
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(result as usize, entries);
    result
}
#[unsafe(no_mangle)]
extern "C" fn CFRelease(value: Ref) {
    if let Some(dicts) = DICTIONARIES.lock().unwrap().as_mut() {
        dicts.remove(&(value as usize));
    }
}
fn record(name: &'static str, query: Ref) {
    let fields = DICTIONARIES.lock().unwrap().as_ref().unwrap()[&(query as usize)].clone();
    CALLS.lock().unwrap().push((name, fields));
}
#[unsafe(no_mangle)]
extern "C" fn SecItemCopyMatching(query: Ref, _: *mut Ref) -> i32 {
    record("read", query);
    -25308
}
#[unsafe(no_mangle)]
extern "C" fn SecItemUpdate(query: Ref, _: Ref) -> i32 {
    record("update", query);
    UPDATE_STATUS.load(Ordering::Relaxed)
}
#[unsafe(no_mangle)]
extern "C" fn SecItemAdd(query: Ref, _: *mut Ref) -> i32 {
    record("add", query);
    0
}
#[unsafe(no_mangle)]
extern "C" fn SecItemDelete(query: Ref) -> i32 {
    record("delete", query);
    -25308
}

#[test]
fn cache_key_operations_never_query_legacy_keychain_or_enable_authentication_ui() {
    let mac_read = super::mac::read;
    assert!(
        mac_read("cache-test", "account").is_none(),
        "locked cache is optional"
    );
    super::mac::write("cache-test", "account", b"synthetic-cache-key");
    super::mac::clear("cache-test", Some("account"));
    UPDATE_STATUS.store(-25308, Ordering::Relaxed);
    super::mac::write("cache-test", "account", b"must-not-overwrite-denied-key");
    let calls = CALLS.lock().unwrap();
    assert_eq!(
        calls.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        ["read", "update", "add", "delete", "update"],
        "denied update must not fall back to creating a replacement"
    );
    for (operation, fields) in calls.iter() {
        assert!(
            fields.contains(&(kSecUseDataProtectionKeychain, kCFBooleanTrue)),
            "{operation} must never search the legacy login keychain"
        );
        assert!(
            fields.contains(&(kSecUseAuthenticationUI, kSecUseAuthenticationUIFail)),
            "{operation} must not enable permission dialogs"
        );
    }
}
