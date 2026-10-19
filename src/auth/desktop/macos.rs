//! Read-only Security.framework adapter for the native app's classic Keychain item.
//! No command-line utility, prompts, access-group changes or ACL modification.
use super::{ACCOUNT, Failure, MAX_CREDENTIAL_BYTES, SERVICE};
use std::{ffi::c_void, ptr};
type CFRef = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithBytes(
        allocator: CFRef,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        external: u8,
    ) -> CFRef;
    fn CFDictionaryCreate(
        allocator: CFRef,
        keys: *const CFRef,
        values: *const CFRef,
        count: isize,
        key_callbacks: CFRef,
        value_callbacks: CFRef,
    ) -> CFRef;
    fn CFRelease(value: CFRef);
    fn CFGetTypeID(value: CFRef) -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFDataGetLength(value: CFRef) -> isize;
    fn CFDataGetBytePtr(value: CFRef) -> *const u8;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    static kCFBooleanTrue: CFRef;
    static kCFBooleanFalse: CFRef;
}
#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecItemCopyMatching(query: CFRef, result: *mut CFRef) -> i32;
    static kSecClass: CFRef;
    static kSecClassGenericPassword: CFRef;
    static kSecAttrService: CFRef;
    static kSecAttrAccount: CFRef;
    static kSecReturnData: CFRef;
    static kSecMatchLimit: CFRef;
    static kSecMatchLimitOne: CFRef;
    static kSecUseAuthenticationUI: CFRef;
    static kSecUseAuthenticationUIFail: CFRef;
    static kSecUseDataProtectionKeychain: CFRef;
}

struct Owned(CFRef);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: Owned holds only a non-null CF object returned by a Create/Copy API.
            unsafe { CFRelease(self.0) };
        }
    }
}
fn string(value: &str) -> std::result::Result<Owned, Failure> {
    // SAFETY: The UTF-8 slice remains valid for the synchronous copying call;
    // null allocator selects the system allocator, and UTF8 is a public CF constant.
    let value = unsafe {
        CFStringCreateWithBytes(
            ptr::null(),
            value.as_ptr(),
            value.len() as isize,
            0x0800_0100,
            0,
        )
    };
    if value.is_null() {
        Err(Failure::Unavailable)
    } else {
        Ok(Owned(value))
    }
}

pub(super) fn read() -> std::result::Result<Vec<u8>, Failure> {
    let service = string(SERVICE)?;
    let account = string(ACCOUNT)?;
    // SAFETY: All constants are exported CF objects. CFDictionaryCreate retains
    // keys/values through the standard callbacks; owned strings outlive the query.
    let query = unsafe {
        let keys = [
            kSecClass,
            kSecAttrService,
            kSecAttrAccount,
            kSecReturnData,
            kSecMatchLimit,
            kSecUseAuthenticationUI,
            kSecUseDataProtectionKeychain,
        ];
        let values = [
            kSecClassGenericPassword,
            service.0,
            account.0,
            kCFBooleanTrue,
            kSecMatchLimitOne,
            kSecUseAuthenticationUIFail,
            kCFBooleanFalse,
        ];
        Owned(CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as isize,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        ))
    };
    if query.0.is_null() {
        return Err(Failure::Unavailable);
    }
    let mut result = ptr::null();
    // SAFETY: Query is a valid CFDictionary and result points to writable storage.
    // The call reads only this exact item, failing rather than asking for UI access.
    let status = unsafe { SecItemCopyMatching(query.0, &mut result) };
    let result = Owned(result);
    if status == -25300 {
        return Err(Failure::Missing);
    }
    if status != 0 || result.0.is_null() {
        return Err(Failure::Unavailable);
    }
    // SAFETY: A successful CopyMatching result is a CF object. Verify CFData
    // before its typed accessors, bound the length, then copy while retained.
    unsafe {
        if CFGetTypeID(result.0) != CFDataGetTypeID() {
            return Err(Failure::Invalid);
        }
        let length = CFDataGetLength(result.0);
        if length <= 0 || length as usize > MAX_CREDENTIAL_BYTES {
            return Err(Failure::Invalid);
        }
        let bytes = CFDataGetBytePtr(result.0);
        if bytes.is_null() {
            return Err(Failure::Invalid);
        }
        Ok(std::slice::from_raw_parts(bytes, length as usize).to_vec())
    }
}
