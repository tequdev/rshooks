//! Ledger information: fee base, sequence, timestamps, hashes, nonces, and
//! keylet computation.
//!
//! `fee_base`, `ledger_seq`, and `ledger_last_time` never return Hook API
//! error codes, so they are exposed as plain (non-`Result`) values, cast
//! from the `i64` wire type to their natural unsigned widths.

use crate::convert::FixedRead;
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

/// The hash of the previous (parent) ledger, written into `out`. Returns the
/// number of bytes written. [`ledger_last_hash_buf`] is the fixed-size
/// convenience twin.
#[inline(always)]
pub fn ledger_last_hash<B: AsMut<[u8]> + ?Sized>(out: &mut B) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.ledger_last_hash()) {
        return crate::testenv_bridge::write_array(out, r);
    }
    res(unsafe { rshooks_core::ledger_last_hash(out.as_mut_ptr() as u32, out.len() as u32) })
        .map(|v| v as usize)
}

/// The hash of the previous (parent) ledger.
#[inline(always)]
pub fn ledger_last_hash_buf() -> Result<Hash> {
    Hash::read_exact(ledger_last_hash)
}

/// A ledger-derived nonce value, written into `out`. Returns the number of
/// bytes written. [`ledger_nonce_buf`] is the fixed-size convenience twin.
#[inline(always)]
pub fn ledger_nonce<B: AsMut<[u8]> + ?Sized>(out: &mut B) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.ledger_nonce()) {
        return crate::testenv_bridge::write_array(out, r);
    }
    res(unsafe { rshooks_core::ledger_nonce(out.as_mut_ptr() as u32, out.len() as u32) })
        .map(|v| v as usize)
}

/// A ledger-derived nonce value (distinct from [`crate::api::etxn::etxn_nonce`],
/// which is per-emission).
#[inline(always)]
pub fn ledger_nonce_buf() -> Result<Nonce> {
    Nonce::read_exact(ledger_nonce)
}

/// Compute a Keylet from a low/high bound pair, written into `out`. Returns
/// the number of bytes written. [`ledger_keylet_buf`] is the fixed-size
/// convenience twin.
#[inline(always)]
pub fn ledger_keylet<B: AsMut<[u8]> + ?Sized>(
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

/// Compute a Keylet from a low/high bound pair (as used by range-style
/// ledger entries).
#[inline(always)]
pub fn ledger_keylet_buf(low: &[u8], high: &[u8]) -> Result<Keylet> {
    Keylet::read_exact(|buf| ledger_keylet(buf, low, high))
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
        assert_eq!(ledger_last_hash_buf(), Err(HookError::NotImplemented));
        assert_eq!(ledger_nonce_buf(), Err(HookError::NotImplemented));
        assert_eq!(
            ledger_keylet_buf(&[0u8; 34], &[0u8; 34]),
            Err(HookError::NotImplemented)
        );
        let mut out = [0u8; 34];
        assert_eq!(ledger_last_hash(&mut out), Err(HookError::NotImplemented));
        assert_eq!(ledger_nonce(&mut out), Err(HookError::NotImplemented));
        assert_eq!(
            ledger_keylet(&mut out, &[0u8; 34], &[0u8; 34]),
            Err(HookError::NotImplemented)
        );
    }
}

/// Proves that `ledger_keylet_buf` rejects a host answer shorter than
/// [`crate::types::KEYLET_LEN`] as [`HookError::TooSmall`] instead of
/// returning a [`Keylet`] zero-padded past the bytes the host actually
/// reported.
#[cfg(all(test, feature = "testenv"))]
mod testenv_tests {
    #![allow(clippy::unwrap_used, clippy::panic)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    extern crate std;

    use std::rc::Rc;
    use std::vec::Vec;

    use super::*;
    use crate::error::HookError;
    use rshooks_core::backend::{HostBackend, install};

    /// Answers `ledger_keylet` with a byte string shorter than a full
    /// `Keylet`; `accept`/`rollback` are unused by this test and simply
    /// panic if ever reached.
    struct ShortLedgerKeyletBackend;

    impl HostBackend for ShortLedgerKeyletBackend {
        fn ledger_keylet(&self, _low: &[u8], _high: &[u8]) -> core::result::Result<Vec<u8>, i64> {
            Ok(std::vec![1, 2, 3])
        }

        fn accept(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("ShortLedgerKeyletBackend::accept unexpectedly called")
        }

        fn rollback(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("ShortLedgerKeyletBackend::rollback unexpectedly called")
        }
    }

    #[test]
    fn ledger_keylet_buf_short_host_answer_is_too_small() {
        let _guard = install(Rc::new(ShortLedgerKeyletBackend));
        assert_eq!(
            ledger_keylet_buf(&[0u8; 34], &[0u8; 34]),
            Err(HookError::TooSmall)
        );
    }
}
