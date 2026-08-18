//! Raw debug-trace wrappers.
//!
//! These are unconditional wrappers over the Hook API's `trace*` functions.
//! The `trace!`/`trace_num!`/`trace_float!` macros in `macros.rs` compile to
//! nothing unless the `trace` feature is enabled — call these functions
//! directly if you want tracing regardless of feature flags.

use crate::error::{Result, res};
use crate::xfl::XFL;

/// Emit a trace message (`msg`), optionally followed by `data` (rendered as
/// hex when `as_hex` is set, otherwise as raw bytes).
#[inline(always)]
pub fn trace(msg: &[u8], data: &[u8], as_hex: bool) -> Result<i64> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.trace(msg, data, as_hex)) {
        return res(v);
    }
    res(unsafe {
        rshooks_core::trace(
            msg.as_ptr() as u32,
            msg.len() as u32,
            data.as_ptr() as u32,
            data.len() as u32,
            as_hex as u32,
        )
    })
}

/// Emit a trace message (`msg`) followed by an integer `number`.
#[inline(always)]
pub fn trace_num(msg: &[u8], number: i64) -> Result<i64> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.trace_num(msg, number)) {
        return res(v);
    }
    res(unsafe { rshooks_core::trace_num(msg.as_ptr() as u32, msg.len() as u32, number) })
}

/// Emit a trace message (`msg`) followed by an XFL `value`.
#[inline(always)]
pub fn trace_float(msg: &[u8], value: XFL) -> Result<i64> {
    res(unsafe {
        rshooks_core::trace_float(msg.as_ptr() as u32, msg.len() as u32, value.raw_bits())
    })
}

/// Internal support for the `trace!`/`trace_num!`/`trace_float!` macros.
///
/// The feature gate must be evaluated inside rshooks (a `#[cfg]` written
/// directly in a `macro_rules!` body would be evaluated against the *calling*
/// crate's features instead), so the macros expand to calls into these
/// always-present shims, which compile to no-ops unless rshooks's own
/// `trace` feature is enabled.
#[doc(hidden)]
pub mod __macro_support {
    /// Shim behind `trace!`. No-op without the `trace` feature.
    #[inline(always)]
    pub fn trace_maybe(msg: &[u8], data: &[u8], as_hex: bool) {
        #[cfg(feature = "trace")]
        let _ = super::trace(msg, data, as_hex);
        #[cfg(not(feature = "trace"))]
        let _ = (msg, data, as_hex);
    }

    /// Shim behind `trace_num!`. No-op without the `trace` feature.
    #[inline(always)]
    pub fn trace_num_maybe(msg: &[u8], number: i64) {
        #[cfg(feature = "trace")]
        let _ = super::trace_num(msg, number);
        #[cfg(not(feature = "trace"))]
        let _ = (msg, number);
    }

    /// Shim behind `trace_float!`. No-op without the `trace` feature.
    #[inline(always)]
    pub fn trace_float_maybe(msg: &[u8], value: crate::xfl::XFL) {
        #[cfg(feature = "trace")]
        let _ = super::trace_float(msg, value);
        #[cfg(not(feature = "trace"))]
        let _ = (msg, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;

    #[test]
    fn smoke_not_implemented_on_host() {
        assert_eq!(
            trace(b"msg", b"data", false),
            Err(HookError::NotImplemented)
        );
        assert_eq!(trace_num(b"msg", 42), Err(HookError::NotImplemented));
        assert_eq!(
            trace_float(b"msg", XFL::from_raw_bits(0)),
            Err(HookError::NotImplemented)
        );
    }
}
