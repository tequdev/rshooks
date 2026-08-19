//! Native testenv host-buffer bridge.
//!
//! [`rshooks_core::backend::HostBackend`]'s byte-returning methods hand
//! back an owned `Result<Vec<u8>, i64>`; every wrapper function in
//! [`crate::api`] that reads bytes writes into a caller-supplied
//! `out: &mut [u8]` and returns the number of bytes written (or an error).
//! This module is the seam between the two conventions, honoring the host
//! buffer contract (`.claude/design/TESTENV_DESIGN.md` §5.2): a
//! destination shorter than the value returns
//! [`HookError::TooSmall`](crate::error::HookError::TooSmall) — it never
//! truncates. [`write_bytes_truncate`]/[`write_bytes_truncate_code`] are the
//! sole, deliberate exception, mirroring xahaud's own `hook_param`
//! asymmetry — see their doc comments.
//!
//! `pub(crate)`, gated identical to the interception blocks that call it
//! (`#[cfg(all(not(target_arch = "wasm32"), feature = "testenv"))]`, applied
//! at this module's declaration in `lib.rs`).

extern crate std;

use std::vec::Vec;

use crate::error::{HookError, Result as HookResult, res};
use crate::types::Keylet;

/// Alias for `HostBackend`'s own error-code convention (`Err` is the raw
/// negative `i64`, not yet decoded into a [`HookError`]) — distinct from
/// [`crate::error::Result`], whose `Err` is always the decoded type.
type BackendResult<T> = core::result::Result<T, i64>;

/// Copies `r`'s success value into `out`, honoring the host buffer
/// contract: `out` shorter than the value returns
/// [`HookError::TooSmall`], never a truncated copy. Success returns the
/// number of bytes copied (the value's own length). `r`'s error case is
/// decoded through [`res`], the same as every raw host call's return code.
#[inline(always)]
pub(crate) fn write_bytes(out: &mut [u8], r: BackendResult<Vec<u8>>) -> HookResult<usize> {
    match r {
        Ok(value) => match out.get_mut(..value.len()) {
            Some(dst) => {
                dst.copy_from_slice(&value);
                Ok(value.len())
            }
            None => Err(HookError::TooSmall),
        },
        Err(code) => res(code).map(|v| v as usize),
    }
}

/// Fixed-size counterpart to [`write_bytes`], for `HostBackend` methods
/// returning `Result<[u8; N], i64>` (`hook_account`, `hook_hash`,
/// `ledger_last_hash`, `ledger_nonce`, `etxn_nonce`, `emit`, ...). Same
/// buffer contract.
#[inline(always)]
pub(crate) fn write_array<const N: usize>(
    out: &mut [u8],
    r: BackendResult<[u8; N]>,
) -> HookResult<usize> {
    match r {
        Ok(value) => match out.get_mut(..N) {
            Some(dst) => {
                dst.copy_from_slice(&value);
                Ok(N)
            }
            None => Err(HookError::TooSmall),
        },
        Err(code) => res(code).map(|v| v as usize),
    }
}

/// Truncating counterpart to [`write_bytes`], for `hook_param` only: xahaud's
/// `hook_param` goes straight to `WRITE_WASM_MEMORY_AND_RETURN`, which writes
/// `min(src_len, dst_len)` bytes and reports that truncated length — unlike
/// `otxn_param` (and every other byte-returning call this bridge otherwise
/// handles), which checks the destination first and returns `TOO_SMALL`.
/// This asymmetry is xahaud's own behavior, not a design choice made here —
/// see `hook_param`'s interception block in `api/hook_ctx.rs`, the only
/// call site that should ever reach for this instead of [`write_bytes`].
/// Success returns the number of bytes actually copied (`min(out.len,
/// value.len)`), never the value's full length when it was truncated.
#[inline(always)]
pub(crate) fn write_bytes_truncate(out: &mut [u8], r: BackendResult<Vec<u8>>) -> HookResult<usize> {
    match r {
        Ok(value) => {
            let n = out.len().min(value.len());
            if let (Some(dst), Some(src)) = (out.get_mut(..n), value.get(..n)) {
                dst.copy_from_slice(src);
            }
            Ok(n)
        }
        Err(code) => res(code).map(|v| v as usize),
    }
}

/// Raw-code counterpart to [`write_bytes_truncate`] — same truncating
/// contract, undecoded `i64` result. Backs `hook_param_raw_code`.
#[inline(always)]
pub(crate) fn write_bytes_truncate_code(out: &mut [u8], r: BackendResult<Vec<u8>>) -> i64 {
    match r {
        Ok(value) => {
            let n = out.len().min(value.len());
            if let (Some(dst), Some(src)) = (out.get_mut(..n), value.get(..n)) {
                dst.copy_from_slice(src);
            }
            n as i64
        }
        Err(code) => code,
    }
}

/// Raw-code counterpart to [`write_bytes`]: same buffer contract, but
/// returns the **undecoded** `i64` a raw host call would have returned (a
/// non-negative byte count on success, a negative error code on failure)
/// instead of a decoded [`Result`] — for call sites that inspect the raw
/// code before deciding whether to construct a [`HookError`]
/// (`state_raw_code`, `hook_param_raw_code`, `otxn_param_raw_code`, ...).
#[inline(always)]
pub(crate) fn write_bytes_code(out: &mut [u8], r: BackendResult<Vec<u8>>) -> i64 {
    match r {
        Ok(value) => match out.get_mut(..value.len()) {
            Some(dst) => {
                dst.copy_from_slice(&value);
                value.len() as i64
            }
            None => rshooks_core::TOO_SMALL,
        },
        Err(code) => code,
    }
}

/// A `state_foreign(_set)` target narrowed to the host's fixed widths:
/// `(namespace, account)`, each `None` meaning "this hook's own".
pub(crate) type ForeignTarget<'a> = (Option<&'a [u8; 32]>, Option<&'a [u8; 20]>);

/// Narrows loosely-typed foreign-target bytes to the host's fixed widths,
/// mirroring the host's own argument checks (`state_foreign`'s
/// `nread_len` must be 0 or 32, `aread_len` 0 or 20): a wrong-length
/// namespace is `INVALID_ARGUMENT`, a wrong-length account is
/// `INVALID_ACCOUNT`. Absent (`None`) means "this hook's own".
#[inline(always)]
pub(crate) fn foreign_target<'a>(
    ns: Option<&'a [u8]>,
    acc: Option<&'a [u8]>,
) -> core::result::Result<ForeignTarget<'a>, i64> {
    let ns = match ns {
        None => None,
        Some(bytes) => match <&[u8; 32]>::try_from(bytes) {
            Ok(a) => Some(a),
            Err(_) => return Err(rshooks_core::INVALID_ARGUMENT),
        },
    };
    let acc = match acc {
        None => None,
        Some(bytes) => match <&[u8; 20]>::try_from(bytes) {
            Ok(a) => Some(a),
            Err(_) => return Err(rshooks_core::INVALID_ACCOUNT),
        },
    };
    Ok((ns, acc))
}

/// Encodes a `HostBackend` byte-read result per the host's "as-int64"
/// convention (the value's bytes packed big-endian into a non-negative
/// `i64` — mirrors xahaud's `data_as_int64`): more than 8 bytes, or a
/// packed value whose top bit is set (unrepresentable as a non-negative
/// `i64`), is `TOO_BIG`. Backs `state_u64`/`state_u64_raw_code`/
/// `otxn_field_u64`/`state_foreign_u64`'s as-int64 read paths.
#[inline(always)]
pub(crate) fn as_int64_code(r: BackendResult<Vec<u8>>) -> i64 {
    match r {
        Ok(value) => {
            let mut buf = [0u8; 8];
            let Some(start) = 8usize.checked_sub(value.len()) else {
                return rshooks_core::TOO_BIG;
            };
            let Some(dst) = buf.get_mut(start..) else {
                return rshooks_core::TOO_BIG;
            };
            // `dst.len() == 8 - start == value.len()` by construction
            // (`start` was derived from `value.len()` above), so this is
            // always an exact-length copy.
            dst.copy_from_slice(&value);
            let packed = i64::from_be_bytes(buf);
            if packed < 0 {
                rshooks_core::TOO_BIG
            } else {
                packed
            }
        }
        Err(code) => code,
    }
}

/// Converts a `HostBackend::util_keylet` result (always exactly 34 bytes —
/// a keylet has no variable-length form) into the decoded [`Keylet`]
/// [`HookResult`] every `crate::api::keylet` typed helper returns. No
/// buffer contract to honor here (unlike [`write_array`]): the return type
/// itself, not a caller-supplied `out`, is the destination.
#[inline(always)]
pub(crate) fn keylet_result(r: BackendResult<[u8; 34]>) -> HookResult<Keylet> {
    match r {
        Ok(bytes) => Ok(Keylet::from(bytes)),
        Err(code) => Err(HookError::from(code)),
    }
}
