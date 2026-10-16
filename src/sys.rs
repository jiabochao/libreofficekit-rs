//! Raw FFI declarations for the stable part of the LibreOfficeKit C ABI.
//!
//! Mirrors `include/LibreOfficeKit/LibreOfficeKit.h`. The class structs are only ever
//! accessed through pointers handed out by LibreOffice, so they declare a prefix of the
//! real vtable; callers must check `nSize` (see [`has_member`]) before touching a slot that
//! was added after LibreOffice 6.0.

#![allow(non_snake_case, non_camel_case_types)]

use std::ffi::{c_char, c_int, c_ulonglong, c_void};

pub type LibreOfficeKitCallback =
    Option<unsafe extern "C" fn(n_type: c_int, payload: *const c_char, data: *mut c_void)>;

/// `LibreOfficeKit* libreofficekit_hook(const char* install_path)`
pub type HookFn = unsafe extern "C" fn(install_path: *const c_char) -> *mut LibreOfficeKit;

/// `LibreOfficeKit* libreofficekit_hook_2(const char* install_path, const char* user_profile_url)`
pub type Hook2Fn = unsafe extern "C" fn(
    install_path: *const c_char,
    user_profile_url: *const c_char,
) -> *mut LibreOfficeKit;

pub const HOOK_SYMBOL: &[u8] = b"libreofficekit_hook\0";
pub const HOOK_2_SYMBOL: &[u8] = b"libreofficekit_hook_2\0";

#[repr(C)]
pub struct LibreOfficeKit {
    pub pClass: *mut LibreOfficeKitClass,
}

#[repr(C)]
pub struct LibreOfficeKitClass {
    pub nSize: usize,

    pub destroy: Option<unsafe extern "C" fn(this: *mut LibreOfficeKit)>,
    pub documentLoad: Option<
        unsafe extern "C" fn(
            this: *mut LibreOfficeKit,
            url: *const c_char,
        ) -> *mut LibreOfficeKitDocument,
    >,
    pub getError: Option<unsafe extern "C" fn(this: *mut LibreOfficeKit) -> *mut c_char>,
    /// @since LibreOffice 5.0
    pub documentLoadWithOptions: Option<
        unsafe extern "C" fn(
            this: *mut LibreOfficeKit,
            url: *const c_char,
            options: *const c_char,
        ) -> *mut LibreOfficeKitDocument,
    >,
    /// @since LibreOffice 5.2
    pub freeError: Option<unsafe extern "C" fn(free: *mut c_char)>,
    /// @since LibreOffice 6.0
    pub registerCallback: Option<
        unsafe extern "C" fn(
            this: *mut LibreOfficeKit,
            callback: LibreOfficeKitCallback,
            data: *mut c_void,
        ),
    >,
    /// @since LibreOffice 6.0
    pub getFilterTypes: Option<unsafe extern "C" fn(this: *mut LibreOfficeKit) -> *mut c_char>,
    /// @since LibreOffice 6.0
    pub setOptionalFeatures:
        Option<unsafe extern "C" fn(this: *mut LibreOfficeKit, features: c_ulonglong)>,
    /// @since LibreOffice 6.0
    pub setDocumentPassword: Option<
        unsafe extern "C" fn(
            this: *mut LibreOfficeKit,
            url: *const c_char,
            password: *const c_char,
        ),
    >,
    /// @since LibreOffice 6.0
    pub getVersionInfo: Option<unsafe extern "C" fn(this: *mut LibreOfficeKit) -> *mut c_char>,
    /// @since LibreOffice 6.0
    pub runMacro:
        Option<unsafe extern "C" fn(this: *mut LibreOfficeKit, url: *const c_char) -> c_int>,
    /// @since LibreOffice 6.2
    pub signDocument: Option<
        unsafe extern "C" fn(
            this: *mut LibreOfficeKit,
            url: *const c_char,
            certificate: *const u8,
            certificate_size: c_int,
            private_key: *const u8,
            private_key_size: c_int,
        ) -> bool,
    >,
    /// Poll/wake callbacks are not used by this crate; they are opaque function pointers here.
    pub runLoop: Option<
        unsafe extern "C" fn(
            this: *mut LibreOfficeKit,
            poll: *mut c_void,
            wake: *mut c_void,
            data: *mut c_void,
        ),
    >,
    pub sendDialogEvent: Option<
        unsafe extern "C" fn(
            this: *mut LibreOfficeKit,
            window_id: c_ulonglong,
            arguments: *const c_char,
        ),
    >,
    pub setOption: Option<
        unsafe extern "C" fn(
            this: *mut LibreOfficeKit,
            option: *const c_char,
            value: *const c_char,
        ),
    >,
    pub dumpState: Option<
        unsafe extern "C" fn(
            this: *mut LibreOfficeKit,
            options: *const c_char,
            state: *mut *mut c_char,
        ),
    >,
    pub extractRequest: Option<
        unsafe extern "C" fn(this: *mut LibreOfficeKit, file_path: *const c_char) -> *mut c_char,
    >,
    pub trimMemory: Option<unsafe extern "C" fn(this: *mut LibreOfficeKit, target: c_int)>,
    // Later slots (startURP, joinThreads, ...) are intentionally not declared.
}

#[repr(C)]
pub struct LibreOfficeKitDocument {
    pub pClass: *mut LibreOfficeKitDocumentClass,
}

#[repr(C)]
pub struct LibreOfficeKitDocumentClass {
    pub nSize: usize,

    pub destroy: Option<unsafe extern "C" fn(this: *mut LibreOfficeKitDocument)>,
    pub saveAs: Option<
        unsafe extern "C" fn(
            this: *mut LibreOfficeKitDocument,
            url: *const c_char,
            format: *const c_char,
            filter_options: *const c_char,
        ) -> c_int,
    >,
    /// @since LibreOffice 6.0
    pub getDocumentType: Option<unsafe extern "C" fn(this: *mut LibreOfficeKitDocument) -> c_int>,
    // The remaining slots are only available with LOK_USE_UNSTABLE_API.
}

// LibreOfficeKitDocumentType
pub const LOK_DOCTYPE_TEXT: c_int = 0;
pub const LOK_DOCTYPE_SPREADSHEET: c_int = 1;
pub const LOK_DOCTYPE_PRESENTATION: c_int = 2;
pub const LOK_DOCTYPE_DRAWING: c_int = 3;
pub const LOK_DOCTYPE_OTHER: c_int = 4;

// LibreOfficeKitOptionalFeatures
pub const LOK_FEATURE_DOCUMENT_PASSWORD: c_ulonglong = 1 << 0;
pub const LOK_FEATURE_DOCUMENT_PASSWORD_TO_MODIFY: c_ulonglong = 1 << 1;

// LibreOfficeKitCallbackType (subset)
pub const LOK_CALLBACK_DOCUMENT_PASSWORD: c_int = 20;
pub const LOK_CALLBACK_DOCUMENT_PASSWORD_TO_MODIFY: c_int = 21;

/// Equivalent of the `LIBREOFFICEKIT_HAS_MEMBER` macro: whether a vtable slot at `offset`
/// lies inside the `nSize` bytes reported by the loaded LibreOffice.
#[inline]
pub fn has_member(offset: usize, n_size: usize) -> bool {
    offset < n_size
}
