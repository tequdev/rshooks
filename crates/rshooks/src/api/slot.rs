//! Memory slot operations: loading ledger objects into numbered slots and
//! navigating/serializing them — the **raw** layer, mirroring the host API
//! one function per host call.
//!
//! Slot numbers and field/array indices are plain `u32` here (no newtype
//! ceremony, per DESIGN.md §5.2). Functions that report a count or size
//! (`slot_count`, `slot_size`) consistently return `Result<u32>` in this
//! module.
//!
//! # This module vs. [`crate::slot_obj`]
//!
//! [`crate::slot_obj`] is the typed layer over exactly these calls:
//! `SlotObject<T>` carries the slot number, the field constants carry the
//! value types, and the reads are the same host calls behind
//! `#[inline(always)]` wrappers. Measured against raw code making the same
//! calls with the same cleanup policy, the typed version is byte-identical —
//! 197 instructions and 925 bytes either way (see `examples/08_slot-ledger`'s
//! README). Reach for the typed layer by default; reach for this one when a
//! hook genuinely wants to place things in specific numbered slots and
//! manage them itself, which `examples/80_reward` and `examples/81_govern`
//! both do.
//!
//! **Do not mix the two.** Both address the same 255 registers. A
//! `slot_clear(3)` here while a `SlotObject` happens to hold slot 3 leaves
//! that handle looking valid while describing whatever lands there next —
//! a logic hazard, not a memory-safety one, so nothing prevents it. Pick one
//! layer per hook.
//!
//! That is also why these functions are **not in the prelude**: reaching for
//! them takes an explicit `rshooks::api::slot::` path (and
//! `rshooks::api::otxn::otxn_slot` for the transaction loader), so mixing
//! is at least always visible at the call site.

use crate::convert::FixedRead;
use crate::error::{Result, res};

/// Serialize the object in `slot_no` into `out`. Returns the number of
/// bytes written.
#[inline(always)]
pub fn slot<B: AsMut<[u8]> + ?Sized>(out: &mut B, slot_no: u32) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.slot(slot_no)) {
        return crate::testenv_bridge::write_bytes(out, r);
    }
    res(unsafe { rshooks_core::slot(out.as_mut_ptr() as u32, out.len() as u32, slot_no) })
        .map(|v| v as usize)
}

/// [`slot`] into uninitialized scratch. The caller may treat only the prefix
/// reported as written as initialized; the buffer remains `MaybeUninit` across FFI
/// to avoid invalid references and guard-charged zeroing stores.
#[inline(always)]
pub(crate) fn slot_uninit(out: &mut [core::mem::MaybeUninit<u8>], slot_no: u32) -> Result<usize> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.slot(slot_no)) {
        return crate::testenv_bridge::write_bytes_uninit(out, r);
    }
    res(unsafe {
        rshooks_core::slot(
            out.as_mut_ptr().cast::<u8>() as u32,
            out.len() as u32,
            slot_no,
        )
    })
    .map(|v| v as usize)
}

/// Serialize the object in `slot_no` and return it as a big-endian `u64`
/// ("as-int64" mode: `write_ptr = 0, write_len = 0`; only for data of at
/// most 8 bytes with the top bit clear, else
/// [`crate::error::HookError::TooBig`] — see `state_u64` for details).
#[inline(always)]
pub fn slot_u64(slot_no: u32) -> Result<u64> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.slot(slot_no)) {
        return res(crate::testenv_bridge::as_int64_code(r)).map(|v| v as u64);
    }
    res(unsafe { rshooks_core::slot(0, 0, slot_no) }).map(|v| v as u64)
}

/// Serialize the object in `slot_no`, requiring the serialization to be
/// exactly `T`'s length — any [`crate::convert::FixedRead`] type. A
/// serialization longer than `T` fails as
/// [`crate::error::HookError::TooSmall`] from the underlying host call; a
/// serialization shorter is caught by `T::read_exact` itself and mapped to
/// the same variant. No loop, no panic.
///
/// `T` is inferred from context, not a turbofish.
///
/// # Examples
///
/// ```
/// use rshooks::api::slot::slot_exact;
/// use rshooks::error::{HookError, Result};
///
/// let value: Result<[u8; 20]> = slot_exact(1);
/// assert_eq!(value, Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn slot_exact<T: FixedRead>(slot_no: u32) -> Result<T> {
    T::read_exact(|buf| slot(buf, slot_no))
}

/// Free `slot_no`, making it available for reuse.
#[inline(always)]
pub fn slot_clear(slot_no: u32) -> Result<i64> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.slot_clear(slot_no)) {
        return res(v);
    }
    res(unsafe { rshooks_core::slot_clear(slot_no) })
}

/// The number of array elements held in `slot_no` (the slot must hold an
/// array).
#[inline(always)]
pub fn slot_count(slot_no: u32) -> Result<u32> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.slot_count(slot_no)) {
        return res(v).map(|v| v as u32);
    }
    res(unsafe { rshooks_core::slot_count(slot_no) }).map(|v| v as u32)
}

/// Load an object identified by a Keylet or transaction hash (`data`) into
/// `slot_into` (`0` auto-assigns). Returns the assigned slot number.
#[inline(always)]
pub fn slot_set(data: &[u8], slot_into: u32) -> Result<u32> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.slot_set(data, slot_into)) {
        return res(v).map(|v| v as u32);
    }
    res(unsafe { rshooks_core::slot_set(data.as_ptr() as u32, data.len() as u32, slot_into) })
        .map(|v| v as u32)
}

/// The serialized size, in bytes, of the object held in `slot_no`.
#[inline(always)]
pub fn slot_size(slot_no: u32) -> Result<u32> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.slot_size(slot_no)) {
        return res(v).map(|v| v as u32);
    }
    res(unsafe { rshooks_core::slot_size(slot_no) }).map(|v| v as u32)
}

/// Extract element `array_id` of the array in `parent_slot` into `new_slot`
/// (`0` auto-assigns). Returns the assigned slot number.
#[inline(always)]
pub fn slot_subarray(parent_slot: u32, array_id: u32, new_slot: u32) -> Result<u32> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) =
        rshooks_core::backend::with_backend(|b| b.slot_subarray(parent_slot, array_id, new_slot))
    {
        return res(v).map(|v| v as u32);
    }
    res(unsafe { rshooks_core::slot_subarray(parent_slot, array_id, new_slot) }).map(|v| v as u32)
}

/// Extract field `field_id` of the object in `parent_slot` into `new_slot`
/// (`0` auto-assigns). Returns the assigned slot number.
#[inline(always)]
pub fn slot_subfield(parent_slot: u32, field_id: impl Into<u32>, new_slot: u32) -> Result<u32> {
    let field_id = field_id.into();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) =
        rshooks_core::backend::with_backend(|b| b.slot_subfield(parent_slot, field_id, new_slot))
    {
        return res(v).map(|v| v as u32);
    }
    res(unsafe { rshooks_core::slot_subfield(parent_slot, field_id, new_slot) }).map(|v| v as u32)
}

/// [`slot_subfield`], returning the **undecoded** `i64` the host call
/// produced instead of a decoded [`Result`].
///
/// A missing field is reported here as `DOESNT_EXIST`, so this is where a
/// caller distinguishing "absent" from "failed" has to look — before any
/// [`crate::error::HookError`] is constructed, per `docs/DESIGN.md` §5.6's
/// nesting-depth rule. Backs
/// [`SlotObject::get_opt`](crate::slot_obj::SlotObject::get_opt), which the
/// generated views' optional-field accessors use.
///
/// Body duplicated rather than shared with [`slot_subfield`], for the
/// reason `api::state`'s `state_raw_code` documents.
#[inline(always)]
pub(crate) fn slot_subfield_raw_code(parent_slot: u32, field_id: u32, new_slot: u32) -> i64 {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) =
        rshooks_core::backend::with_backend(|b| b.slot_subfield(parent_slot, field_id, new_slot))
    {
        return v;
    }
    unsafe { rshooks_core::slot_subfield(parent_slot, field_id, new_slot) }
}

/// The type of the object in `slot_no`: with `flags = 0`, the field code;
/// with `flags = 1`, whether it is a native (XRP/XAH) amount.
#[inline(always)]
pub fn slot_type(slot_no: u32, flags: u32) -> Result<u32> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.slot_type(slot_no, flags)) {
        return res(v).map(|v| v as u32);
    }
    res(unsafe { rshooks_core::slot_type(slot_no, flags) }).map(|v| v as u32)
}

/// Load the current transaction's metadata into `slot_into` (`0`
/// auto-assigns). Returns the assigned slot number.
#[inline(always)]
pub fn meta_slot(slot_into: u32) -> Result<u32> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.meta_slot(slot_into)) {
        return res(v).map(|v| v as u32);
    }
    res(unsafe { rshooks_core::meta_slot(slot_into) }).map(|v| v as u32)
}

/// Load an XPOP's transaction and metadata into the given slots.
#[inline(always)]
pub fn xpop_slot(slot_no_tx: u32, slot_no_meta: u32) -> Result<i64> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.xpop_slot(slot_no_tx, slot_no_meta))
    {
        return res(v);
    }
    res(unsafe { rshooks_core::xpop_slot(slot_no_tx, slot_no_meta) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;

    #[test]
    fn smoke_not_implemented_on_host() {
        let mut out = [0u8; 32];
        assert_eq!(slot(&mut out, 1), Err(HookError::NotImplemented));
        assert_eq!(slot_u64(1), Err(HookError::NotImplemented));
        assert_eq!(slot_exact::<[u8; 20]>(1), Err(HookError::NotImplemented));
        assert_eq!(slot_clear(1), Err(HookError::NotImplemented));
        assert_eq!(slot_count(1), Err(HookError::NotImplemented));
        assert_eq!(slot_set(&out, 0), Err(HookError::NotImplemented));
        assert_eq!(slot_size(1), Err(HookError::NotImplemented));
        assert_eq!(slot_subarray(1, 0, 0), Err(HookError::NotImplemented));
        assert_eq!(slot_subfield(1, 0u32, 0), Err(HookError::NotImplemented));
        assert_eq!(slot_type(1, 0), Err(HookError::NotImplemented));
        assert_eq!(meta_slot(0), Err(HookError::NotImplemented));
        assert_eq!(xpop_slot(1, 2), Err(HookError::NotImplemented));
    }
}
