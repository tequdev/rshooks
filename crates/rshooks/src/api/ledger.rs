//! Ledger information: fee base, sequence, timestamps, hashes, nonces, and
//! keylet computation.
//!
//! `fee_base`, `ledger_seq`, and `ledger_last_time` never return Hook API
//! error codes, so they are exposed as plain (non-`Result`) values, cast
//! from the `i64` wire type to their natural unsigned widths.

use crate::api::fixed_buf_fn;
use crate::error::{Result, res};
use crate::types::{Hash, Keylet, Nonce};

/// The reference transaction fee (in drops) for the current ledger.
#[inline(always)]
pub fn fee_base() -> u64 {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.fee_base()) {
        return v as u64;
    }
    unsafe { rshooks_core::fee_base() as u64 }
}

/// The sequence number of the current ledger.
#[inline(always)]
pub fn ledger_seq() -> u32 {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.ledger_seq()) {
        return v as u32;
    }
    unsafe { rshooks_core::ledger_seq() as u32 }
}

/// The close time of the previous ledger (seconds since the Ripple epoch).
#[inline(always)]
pub fn ledger_last_time() -> u64 {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.ledger_last_time()) {
        return v as u64;
    }
    unsafe { rshooks_core::ledger_last_time() as u64 }
}

/// Read the hash of the previous (parent) ledger into `out`. Returns the
/// number of bytes written. `pub(crate)`:
/// [`ledger_last_hash`]/[`ledger_last_hash_into`] are the public forms —
/// see the `api` module doc comment's "Naming" section.
#[inline(always)]
pub(crate) fn ledger_last_hash_raw<B: AsMut<[u8]> + ?Sized>(out: &mut B) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.ledger_last_hash()) {
        return crate::testenv_bridge::write_array(out, r);
    }
    res(unsafe { rshooks_core::ledger_last_hash(out.as_mut_ptr() as u32, out.len() as u32) })
        .map(|v| v as usize)
}

fixed_buf_fn! {
    /// The hash of the previous (parent) ledger.
    fn ledger_last_hash() -> Hash = ledger_last_hash_raw,
    ledger_last_hash_into
}

/// Read a ledger-derived nonce value into `out`. Returns the number of
/// bytes written. `pub(crate)`: [`ledger_nonce`]/[`ledger_nonce_into`] are
/// the public forms — see the `api` module doc comment's "Naming" section.
#[inline(always)]
pub(crate) fn ledger_nonce_raw<B: AsMut<[u8]> + ?Sized>(out: &mut B) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.ledger_nonce()) {
        return crate::testenv_bridge::write_array(out, r);
    }
    res(unsafe { rshooks_core::ledger_nonce(out.as_mut_ptr() as u32, out.len() as u32) })
        .map(|v| v as usize)
}

fixed_buf_fn! {
    /// A ledger-derived nonce value (distinct from
    /// [`crate::api::etxn::etxn_nonce`], which is per-emission).
    fn ledger_nonce() -> Nonce = ledger_nonce_raw,
    ledger_nonce_into
}

/// Compute a Keylet from a low/high bound pair, written into `out`. Returns
/// the number of bytes written. `pub(crate)`:
/// [`ledger_keylet`]/[`ledger_keylet_into`] are the public forms — see the
/// `api` module doc comment's "Naming" section.
#[inline(always)]
pub(crate) fn ledger_keylet_raw<B: AsMut<[u8]> + ?Sized>(
    out: &mut B,
    low: &[u8],
    high: &[u8],
) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.ledger_keylet(low, high)) {
        return crate::testenv_bridge::write_bytes(out, r);
    }
    res(unsafe {
        rshooks_core::ledger_keylet(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            low.as_ptr() as u32,
            low.len() as u32,
            high.as_ptr() as u32,
            high.len() as u32,
        )
    })
    .map(|v| v as usize)
}

fixed_buf_fn! {
    /// Compute a Keylet from a low/high bound pair (as used by range-style
    /// ledger entries).
    fn ledger_keylet(low: &[u8], high: &[u8]) -> Keylet = ledger_keylet_raw,
    ledger_keylet_into
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;

    #[test]
    fn smoke_not_implemented_on_host() {
        assert_eq!(fee_base(), rshooks_core::NOT_IMPLEMENTED as u64);
        assert_eq!(ledger_seq(), rshooks_core::NOT_IMPLEMENTED as u32);
        assert_eq!(ledger_last_time(), rshooks_core::NOT_IMPLEMENTED as u64);
        assert_eq!(ledger_last_hash(), Err(HookError::NotImplemented));
        assert_eq!(
            ledger_last_hash_into(&mut Hash::default()),
            Err(HookError::NotImplemented)
        );
        assert_eq!(ledger_nonce(), Err(HookError::NotImplemented));
        assert_eq!(
            ledger_nonce_into(&mut Nonce::default()),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            ledger_keylet(&[0u8; 34], &[0u8; 34]),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            ledger_keylet_into(&mut Keylet::default(), &[0u8; 34], &[0u8; 34]),
            Err(HookError::NotImplemented)
        );
        let mut out = [0u8; 34];
        assert_eq!(
            ledger_last_hash_into(&mut out),
            Err(HookError::NotImplemented)
        );
        assert_eq!(ledger_nonce_into(&mut out), Err(HookError::NotImplemented));
        assert_eq!(
            ledger_keylet_into(&mut out, &[0u8; 34], &[0u8; 34]),
            Err(HookError::NotImplemented)
        );
    }
}
