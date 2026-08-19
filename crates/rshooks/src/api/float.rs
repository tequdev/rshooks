//! Buffer-shaped XFL functions: converting an [`XFL`] to/from its
//! serialized STO (Amount) representation, and reading an XFL out of a slot.
//!
//! The other 14 `float_*` Hook API functions are pure arithmetic on the
//! XFL bit representation and are wrapped privately inside `xfl.rs` as
//! `XFL` methods instead of being exposed here — see `xfl.rs`'s module doc
//! comment for the full accounting.

use crate::error::{HookError, Result, res};
use crate::types::{AccountId, CurrencyCode};
use crate::xfl::XFL;

/// Encode `amount` as a serialized Amount into `out`.
///
/// `currency`/`issuer` must be both `Some` (encode as an IOU amount) or both
/// `None` (encode as a native XRP/XAH amount) — the underlying protocol has
/// no meaning for one without the other, so the mixed case is rejected
/// locally as [`HookError::InvalidArgument`] without making a host call.
#[inline(always)]
pub fn float_sto<B: AsMut<[u8]> + ?Sized>(
    out: &mut B,
    currency: Option<&CurrencyCode>,
    issuer: Option<&AccountId>,
    amount: XFL,
    field_code: impl Into<u32>,
) -> Result<usize> {
    let field_code = field_code.into();
    let out = out.as_mut();
    let (cptr, clen, iptr, ilen) = match (currency, issuer) {
        (Some(c), Some(i)) => (
            c.as_ptr() as u32,
            c.len() as u32,
            i.as_ptr() as u32,
            i.len() as u32,
        ),
        (None, None) => (0u32, 0u32, 0u32, 0u32),
        _ => return Err(HookError::InvalidArgument),
    };
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| {
        b.float_sto(
            currency.map(AsRef::as_ref),
            issuer.map(AsRef::as_ref),
            amount.raw_bits(),
            field_code,
        )
    }) {
        return crate::testenv_bridge::write_bytes(out, r);
    }
    res(unsafe {
        rshooks_core::float_sto(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            cptr,
            clen,
            iptr,
            ilen,
            amount.raw_bits(),
            field_code,
        )
    })
    .map(|v| v as usize)
}

/// Decode a serialized Amount (`buf`) into an [`XFL`].
#[inline(always)]
pub fn float_sto_set(buf: &[u8]) -> Result<XFL> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.float_sto_set(buf)) {
        return res(v).map(XFL::from_raw_bits);
    }
    res(unsafe { rshooks_core::float_sto_set(buf.as_ptr() as u32, buf.len() as u32) })
        .map(XFL::from_raw_bits)
}

/// Read the amount held in `slot_no` as an [`XFL`].
#[inline(always)]
pub fn slot_float(slot_no: u32) -> Result<XFL> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.slot_float(slot_no)) {
        return res(v).map(XFL::from_raw_bits);
    }
    res(unsafe { rshooks_core::slot_float(slot_no) }).map(XFL::from_raw_bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;

    #[test]
    fn smoke_not_implemented_on_host() {
        let mut out = [0u8; 48];
        let one = XFL::one();
        assert_eq!(
            float_sto(&mut out, None, None, one, 0u32),
            Err(HookError::NotImplemented)
        );
        // `XFL` has no `PartialEq` by design, so `Result<XFL, _>` can't use
        // `assert_eq!`; `matches!` avoids needing `unwrap`/`expect`.
        assert!(matches!(
            float_sto_set(&out),
            Err(HookError::NotImplemented)
        ));
        assert!(matches!(slot_float(1), Err(HookError::NotImplemented)));
    }

    #[test]
    fn float_sto_rejects_mixed_options() {
        let mut out = [0u8; 48];
        let currency = CurrencyCode::default();
        let one = XFL::one();
        assert_eq!(
            float_sto(&mut out, Some(&currency), None, one, 0u32),
            Err(HookError::InvalidArgument)
        );
    }
}
