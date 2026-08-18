//! Emitted-transaction (`etxn_*`) API: reserving emission slots, computing
//! fees/nonces, and emitting transactions.
//!
//! Burden and fee values are naturally unsigned magnitudes even though the
//! Hook API wire type is `i64` — fallible calls return them as `u64` (the
//! non-negative payload cast with `as`, safe because
//! [`crate::error::res`] already rejected negative values), while calls
//! that never return an error code (`etxn_generation`) are exposed as
//! plain values.

use crate::error::{Result, res};
use crate::types::{Hash, Nonce};

/// Burden of this hook's own emitted transactions so far.
#[inline(always)]
pub fn etxn_burden() -> Result<u64> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.etxn_burden()) {
        return res(v).map(|v| v as u64);
    }
    res(unsafe { rshooks_core::etxn_burden() }).map(|v| v as u64)
}

/// Writes the serialized `EmitDetails` object for the next transaction this
/// hook would emit into `out`, returning the number of bytes written.
///
/// The length is not protocol-fixed: it depends on whether this hook's wasm
/// module exports a `cbak` callback (xahaud's `HookAPI::etxn_details`,
/// `src/xrpld/app/hook/detail/HookAPI.cpp`, appends an extra `sfEmitCallback`
/// field when it does) — 116 bytes without a callback, 138 bytes with one.
/// Size `out` to [`crate::types::EMIT_DETAILS_MAX_LEN`] (the worst case) and
/// trust the returned length, not `out.len()`, as the field's true size.
#[inline(always)]
pub fn etxn_details<B: AsMut<[u8]> + ?Sized>(out: &mut B) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.etxn_details()) {
        return crate::testenv_bridge::write_bytes(out, r);
    }
    res(unsafe { rshooks_core::etxn_details(out.as_mut_ptr() as u32, out.len() as u32) })
        .map(|v| v as usize)
}

/// The base fee (in drops) required to emit `tx_blob`.
#[inline(always)]
pub fn etxn_fee_base(tx_blob: &[u8]) -> Result<u64> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.etxn_fee_base(tx_blob)) {
        return res(v).map(|v| v as u64);
    }
    res(unsafe { rshooks_core::etxn_fee_base(tx_blob.as_ptr() as u32, tx_blob.len() as u32) })
        .map(|v| v as u64)
}

/// Reserve `count` emission slots for this hook invocation. Must be called
/// before [`emit`].
#[inline(always)]
pub fn etxn_reserve(count: u32) -> Result<i64> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.etxn_reserve(count)) {
        return res(v);
    }
    res(unsafe { rshooks_core::etxn_reserve(count) })
}

/// The generation of transactions emitted by this hook so far.
#[inline(always)]
pub fn etxn_generation() -> u32 {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.etxn_generation()) {
        return v as u32;
    }
    unsafe { rshooks_core::etxn_generation() as u32 }
}

/// A fresh nonce for use in an emitted transaction, written into `out`.
/// Returns the number of bytes written. [`etxn_nonce_buf`] is the fixed-size
/// convenience twin.
#[inline(always)]
pub fn etxn_nonce<B: AsMut<[u8]> + ?Sized>(out: &mut B) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.etxn_nonce()) {
        return crate::testenv_bridge::write_array(out, r);
    }
    res(unsafe { rshooks_core::etxn_nonce(out.as_mut_ptr() as u32, out.len() as u32) })
        .map(|v| v as usize)
}

/// A fresh nonce for use in an emitted transaction.
#[inline(always)]
pub fn etxn_nonce_buf() -> Result<Nonce> {
    let mut buf = Nonce::default();
    let _ = etxn_nonce(buf.as_mut())?;
    Ok(buf)
}

/// Emit `tx_blob` as a new transaction, writing the emitted transaction's
/// hash into `out`. Requires a prior [`etxn_reserve`] call. Returns the
/// number of bytes written. [`emit_buf`] is the fixed-size convenience twin.
#[inline(always)]
pub fn emit<B: AsMut<[u8]> + ?Sized>(out: &mut B, tx_blob: &[u8]) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.emit(tx_blob)) {
        return crate::testenv_bridge::write_array(out, r);
    }
    res(unsafe {
        rshooks_core::emit(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            tx_blob.as_ptr() as u32,
            tx_blob.len() as u32,
        )
    })
    .map(|v| v as usize)
}

/// Emit `tx_blob` as a new transaction. Requires a prior [`etxn_reserve`]
/// call. Returns the emitted transaction's hash.
#[inline(always)]
pub fn emit_buf(tx_blob: &[u8]) -> Result<Hash> {
    let mut buf = Hash::default();
    let _ = emit(buf.as_mut(), tx_blob)?;
    Ok(buf)
}

/// Prepare a transaction template (`template`) into `out`, substituting
/// hook-computed fields. Returns the number of bytes written.
#[inline(always)]
pub fn prepare<B: AsMut<[u8]> + ?Sized>(out: &mut B, template: &[u8]) -> Result<usize> {
    let out = out.as_mut();
    res(unsafe {
        rshooks_core::prepare(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            template.as_ptr() as u32,
            template.len() as u32,
        )
    })
    .map(|v| v as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;
    use crate::types::EMIT_DETAILS_MAX_LEN;

    #[test]
    fn smoke_not_implemented_on_host() {
        assert_eq!(etxn_burden(), Err(HookError::NotImplemented));
        let mut ed_out = [0u8; EMIT_DETAILS_MAX_LEN];
        assert_eq!(etxn_details(&mut ed_out), Err(HookError::NotImplemented));
        assert_eq!(etxn_fee_base(&[0u8; 4]), Err(HookError::NotImplemented));
        assert_eq!(etxn_reserve(1), Err(HookError::NotImplemented));
        assert_eq!(etxn_generation(), rshooks_core::NOT_IMPLEMENTED as u32);
        assert_eq!(etxn_nonce_buf(), Err(HookError::NotImplemented));
        assert_eq!(emit_buf(&[0u8; 4]), Err(HookError::NotImplemented));
        let mut out = [0u8; 8];
        let mut nonce_out = [0u8; 32];
        assert_eq!(etxn_nonce(&mut nonce_out), Err(HookError::NotImplemented));
        assert_eq!(
            emit(&mut nonce_out, &[0u8; 4]),
            Err(HookError::NotImplemented)
        );
        assert_eq!(prepare(&mut out, &[0u8; 4]), Err(HookError::NotImplemented));
    }
}
