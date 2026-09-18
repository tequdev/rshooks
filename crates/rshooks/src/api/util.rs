//! Address conversion, hashing, signature verification, and keylet
//! computation utilities.

use crate::error::{HookError, Result, res};
use crate::types::{AccountId, Hash, Keylet};

/// Convert an AccountID (`accid`) to its base58 r-address text form,
/// written into `out`. Variable-length text output, so this stays on the
/// caller-buffer `Result<usize>` convention rather than a fixed-size
/// convenience wrapper.
#[inline(always)]
pub fn util_raddr<B: AsMut<[u8]> + ?Sized>(out: &mut B, accid: &[u8]) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.util_raddr(accid)) {
        return crate::testenv_bridge::write_bytes(out, r);
    }
    res(unsafe {
        rshooks_core::util_raddr(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            accid.as_ptr() as u32,
            accid.len() as u32,
        )
    })
    .map(|v| v as usize)
}

/// Convert a base58 r-address (`r_address`) to its AccountID form, written
/// into `out`. Returns the number of bytes written. [`util_accid_buf`] is
/// the fixed-size convenience twin.
#[inline(always)]
pub fn util_accid<B: AsMut<[u8]> + ?Sized>(out: &mut B, r_address: &[u8]) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.util_accid(r_address)) {
        return crate::testenv_bridge::write_bytes(out, r);
    }
    res(unsafe {
        rshooks_core::util_accid(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            r_address.as_ptr() as u32,
            r_address.len() as u32,
        )
    })
    .map(|v| v as usize)
}

/// Convert a base58 r-address (`r_address`) to its AccountID form.
#[inline(always)]
pub fn util_accid_buf(r_address: &[u8]) -> Result<AccountId> {
    let mut buf = AccountId::default();
    let _ = util_accid(buf.as_mut(), r_address)?;
    Ok(buf)
}

/// Verify that `signature` over `data` was produced by the key `public_key`.
#[inline(always)]
pub fn util_verify(data: &[u8], signature: &[u8], public_key: &[u8]) -> Result<bool> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) =
        rshooks_core::backend::with_backend(|b| b.util_verify(data, signature, public_key))
    {
        return res(v).map(|v| v != 0);
    }
    res(unsafe {
        rshooks_core::util_verify(
            data.as_ptr() as u32,
            data.len() as u32,
            signature.as_ptr() as u32,
            signature.len() as u32,
            public_key.as_ptr() as u32,
            public_key.len() as u32,
        )
    })
    .map(|v| v != 0)
}

/// SHA-512-Half of `data`, written into `out`. Returns the number of bytes
/// written. [`util_sha512h_buf`] is the fixed-size convenience twin.
#[inline(always)]
pub fn util_sha512h<B: AsMut<[u8]> + ?Sized>(out: &mut B, data: &[u8]) -> Result<usize> {
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.util_sha512h(data)) {
        return crate::testenv_bridge::write_array(out, r);
    }
    res(unsafe {
        rshooks_core::util_sha512h(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            data.as_ptr() as u32,
            data.len() as u32,
        )
    })
    .map(|v| v as usize)
}

/// SHA-512-Half of `data`.
#[inline(always)]
pub fn util_sha512h_buf(data: &[u8]) -> Result<Hash> {
    let mut buf = Hash::default();
    let _ = util_sha512h(buf.as_mut(), data)?;
    Ok(buf)
}

/// Compute a Keylet of `keylet_type` from up to six `u32` components
/// (`a`..`f`), written into `out`. Returns the number of bytes written.
/// [`util_keylet_buf`] is the fixed-size convenience twin.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub fn util_keylet<B: AsMut<[u8]> + ?Sized>(
    out: &mut B,
    keylet_type: u32,
    a: u32,
    b: u32,
    c: u32,
    d: u32,
    e: u32,
    f: u32,
) -> Result<usize> {
    let out = out.as_mut();
    // Untyped escape hatch: by the time a caller reaches this function, any
    // pointer-shaped component has already been `.as_ptr() as u32`-cast away
    // by the caller (typed callers go through `crate::api::keylet`'s 26
    // helpers instead, which intercept with real slices first). This block
    // can therefore only forward `a..f` as opaque `KeyletArg::Value`s —
    // correct for a value-only keylet type, but unable to resolve real bytes
    // for a pointer-bearing type; calling this function directly for one
    // gets only e2e-fidelity under `testenv` until a typed helper exists.
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|backend| {
        backend.util_keylet(
            keylet_type,
            [
                rshooks_core::backend::KeyletArg::Value(a),
                rshooks_core::backend::KeyletArg::Value(b),
                rshooks_core::backend::KeyletArg::Value(c),
                rshooks_core::backend::KeyletArg::Value(d),
                rshooks_core::backend::KeyletArg::Value(e),
                rshooks_core::backend::KeyletArg::Value(f),
            ],
        )
    }) {
        return crate::testenv_bridge::write_array(out, r);
    }
    res(unsafe {
        rshooks_core::util_keylet(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            keylet_type,
            a,
            b,
            c,
            d,
            e,
            f,
        )
    })
    .map(|v| v as usize)
}

/// Compute a Keylet of `keylet_type` from up to six `u32` components
/// (`a`..`f`; unused components are `0` per the Hook API convention for the
/// given keylet type — see `rshooks_core::KEYLET_*`, or, for a precisely
/// typed per-`KEYLET_*`-type signature instead of six same-typed `u32`
/// slots, [`crate::api::keylet`]).
///
/// This function's own scratch buffer is uninitialized (`MaybeUninit`,
/// materialized only once the host reports writing all 34 bytes), never a
/// local zero-init — so no `wasm32v1-none` `memset`-lowering threshold
/// applies here at any `opt-level` (see `docs/DESIGN.md`'s §2 C6 for that
/// threshold in general, for hook-author-owned zero-init buffers it does
/// still apply to).
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub fn util_keylet_buf(
    keylet_type: u32,
    a: u32,
    b: u32,
    c: u32,
    d: u32,
    e: u32,
    f: u32,
) -> Result<Keylet> {
    let mut storage = core::mem::MaybeUninit::<[u8; crate::types::KEYLET_LEN]>::uninit();
    // SAFETY: only read via `assume_init` below, once `written == KEYLET_LEN`
    // proves the host wrote every byte.
    let buf = unsafe { crate::convert::uninit_slice_mut(&mut storage) };
    let written = util_keylet(buf, keylet_type, a, b, c, d, e, f)?;
    if written == crate::types::KEYLET_LEN {
        // SAFETY: `written == KEYLET_LEN` proves the host wrote every byte.
        Ok(Keylet(unsafe { storage.assume_init() }))
    } else {
        Err(HookError::TooSmall)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;

    #[test]
    fn smoke_not_implemented_on_host() {
        let mut out = [0u8; 64];
        assert_eq!(
            util_raddr(&mut out, &[0u8; 20]),
            Err(HookError::NotImplemented)
        );
        assert_eq!(util_accid_buf(b"raddress"), Err(HookError::NotImplemented));
        assert_eq!(
            util_verify(b"data", b"sig", b"pubkey"),
            Err(HookError::NotImplemented)
        );
        assert_eq!(util_sha512h_buf(b"data"), Err(HookError::NotImplemented));
        assert_eq!(
            util_keylet_buf(1, 0, 0, 0, 0, 0, 0),
            Err(HookError::NotImplemented)
        );
        let mut accid_out = [0u8; 20];
        assert_eq!(
            util_accid(&mut accid_out, b"raddress"),
            Err(HookError::NotImplemented)
        );
        let mut hash_out = [0u8; 32];
        assert_eq!(
            util_sha512h(&mut hash_out, b"data"),
            Err(HookError::NotImplemented)
        );
        let mut keylet_out = [0u8; 34];
        assert_eq!(
            util_keylet(&mut keylet_out, 1, 0, 0, 0, 0, 0, 0),
            Err(HookError::NotImplemented)
        );
    }
}
