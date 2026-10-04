//! The exported C functions. This is the only module in the workspace that uses `unsafe`:
//! each block turns pointers received from C into Rust references, under the contract stated
//! in the function's `# Safety` section.
#![allow(unsafe_code)]

use core::ffi::c_char;
use core::slice;

use kaisatsu::{Extensions, KeyId, Pinpon, TrustedKey, Verifier, base45};

use crate::types::{KaisatsuKey, KaisatsuStatus, KaisatsuTicket, RawKeys};

/// Verifies a binary ticket against a list of trusted keys.
///
/// On success, writes the claims to `*out` and returns `KAISATSU_STATUS_OK`. Pointers inside
/// `*out` borrow from `ticket`.
///
/// # Safety
///
/// - `keys` must point to `key_count` valid `KaisatsuKey`s (or may be null if `key_count` is 0).
/// - `ticket` must point to `ticket_len` readable bytes.
/// - `out` must point to writable memory for one `KaisatsuTicket`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kaisatsu_verify(
    keys: *const KaisatsuKey,
    key_count: usize,
    ticket: *const u8,
    ticket_len: usize,
    out: *mut KaisatsuTicket,
) -> KaisatsuStatus {
    if (keys.is_null() && key_count != 0) || ticket.is_null() || out.is_null() {
        return KaisatsuStatus::NullPointer;
    }
    // SAFETY: non-null, and the caller guarantees `key_count` valid keys.
    let keys = unsafe { slice_or_empty(keys, key_count) };
    // SAFETY: non-null, and the caller guarantees `ticket_len` readable bytes.
    let ticket = unsafe { slice::from_raw_parts(ticket, ticket_len) };

    match Verifier::new(RawKeys(keys)).verify(ticket) {
        Ok(verified) => {
            // SAFETY: non-null, and the caller guarantees it is writable.
            unsafe { out.write(KaisatsuTicket::from(&verified)) };
            KaisatsuStatus::Ok
        }
        Err(pinpon) => pinpon.into(),
    }
}

/// Checks that `now` (Unix seconds) lies within the ticket's validity window.
///
/// Returns `KAISATSU_STATUS_OK`, `KAISATSU_STATUS_NOT_YET_VALID` or `KAISATSU_STATUS_EXPIRED`.
///
/// # Safety
///
/// `ticket` must point to a `KaisatsuTicket` filled in by `kaisatsu_verify`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kaisatsu_ticket_check_time(
    ticket: *const KaisatsuTicket,
    now: u64,
) -> KaisatsuStatus {
    // SAFETY: the caller guarantees the pointer is null or valid.
    let Some(ticket) = (unsafe { ticket.as_ref() }) else {
        return KaisatsuStatus::NullPointer;
    };
    if now < ticket.valid_from {
        Pinpon::NotYetValid.into()
    } else if now >= ticket.valid_until {
        Pinpon::Expired.into()
    } else {
        KaisatsuStatus::Ok
    }
}

/// Looks up the extension with the given tag. Returns `true` and sets `*value` / `*value_len`
/// if the ticket carries it.
///
/// # Safety
///
/// - `ticket` must point to a `KaisatsuTicket` filled in by `kaisatsu_verify`, whose ticket
///   buffer is still alive.
/// - `value` and `value_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kaisatsu_ticket_extension(
    ticket: *const KaisatsuTicket,
    tag: u8,
    value: *mut *const u8,
    value_len: *mut usize,
) -> bool {
    // SAFETY: the caller guarantees the pointer is null or valid.
    let Some(ticket) = (unsafe { ticket.as_ref() }) else {
        return false;
    };
    if value.is_null() || value_len.is_null() {
        return false;
    }
    // SAFETY: `kaisatsu_verify` set these from a live slice, which the caller keeps alive.
    let raw = unsafe { slice_or_empty(ticket.extensions, ticket.extensions_len) };
    let Some(found) = Extensions::from_bytes(raw)
        .ok()
        .and_then(|ext| ext.get(tag))
    else {
        return false;
    };
    // SAFETY: both checked non-null above; the caller guarantees they are writable.
    unsafe {
        value.write(found.as_ptr());
        value_len.write(found.len());
    }
    true
}

/// Decodes the Base45 text of a QR code into `out`.
///
/// `KAISATSU_MAX_TICKET_LEN` bytes of output always suffice.
///
/// # Safety
///
/// - `text` must point to `text_len` readable bytes.
/// - `out` must point to `out_capacity` writable bytes.
/// - `out_len` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kaisatsu_base45_decode(
    text: *const c_char,
    text_len: usize,
    out: *mut u8,
    out_capacity: usize,
    out_len: *mut usize,
) -> KaisatsuStatus {
    if text.is_null() || out.is_null() || out_len.is_null() {
        return KaisatsuStatus::NullPointer;
    }
    // SAFETY: non-null; the caller guarantees `text_len` readable bytes.
    let text = unsafe { slice::from_raw_parts(text.cast::<u8>(), text_len) };
    // SAFETY: non-null; the caller guarantees `out_capacity` writable bytes.
    let out = unsafe { slice::from_raw_parts_mut(out, out_capacity) };
    match base45::decode_into(text, out) {
        Ok(decoded) => {
            // SAFETY: non-null; the caller guarantees it is writable.
            unsafe { out_len.write(decoded.len()) };
            KaisatsuStatus::Ok
        }
        Err(defect) => defect.into(),
    }
}

/// Checks that a public key is a usable Ed25519 key, and writes its key id to `out_key_id`
/// (if not null). Call it once at startup to catch misconfigured keys early: `kaisatsu_verify`
/// silently ignores unusable keys.
///
/// # Safety
///
/// `key` must point to a `KaisatsuKey`; `out_key_id` must be null or point to 8 writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kaisatsu_key_check(
    key: *const KaisatsuKey,
    out_key_id: *mut u8,
) -> KaisatsuStatus {
    // SAFETY: the caller guarantees the pointer is null or valid.
    let Some(key) = (unsafe { key.as_ref() }) else {
        return KaisatsuStatus::NullPointer;
    };
    if TrustedKey::from_bytes(&key.public_key).is_err() {
        return KaisatsuStatus::InvalidKey;
    }
    if !out_key_id.is_null() {
        // SAFETY: non-null; the caller guarantees 8 writable bytes.
        unsafe {
            out_key_id
                .cast::<[u8; 8]>()
                .write_unaligned(KeyId::of(&key.public_key).to_bytes());
        };
    }
    KaisatsuStatus::Ok
}

/// A static, NUL-terminated description of a status, e.g. `"ピンポーン🔔 BadSignature"`.
/// Never returns null.
#[unsafe(no_mangle)]
pub extern "C" fn kaisatsu_status_message(status: i32) -> *const c_char {
    let message = match status {
        0 => c"OK",
        1 => c"ピンポーン🔔 Malformed",
        2 => c"ピンポーン🔔 UnsupportedVersion",
        3 => c"ピンポーン🔔 UnsupportedAlgorithm",
        4 => c"ピンポーン🔔 UnknownKey",
        5 => c"ピンポーン🔔 BadSignature",
        6 => c"ピンポーン🔔 NotYetValid",
        7 => c"ピンポーン🔔 Expired",
        8 => c"invalid public key",
        9 => c"null pointer argument",
        10 => c"buffer too small",
        _ => c"unknown status",
    };
    message.as_ptr()
}

/// Builds a slice from a pointer that may be null when the length is zero.
///
/// # Safety
///
/// If `len > 0`, `data` must point to `len` valid, initialized `T`s.
unsafe fn slice_or_empty<'a, T>(data: *const T, len: usize) -> &'a [T] {
    if len == 0 {
        &[]
    } else {
        // SAFETY: upheld by the caller.
        unsafe { slice::from_raw_parts(data, len) }
    }
}
