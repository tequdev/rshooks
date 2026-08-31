//! Boundary conversion traits: [`ToBytes`]/[`FromBytes`].
//!
//! These traits fix how a small, fixed-size Rust value crosses into/out of a
//! protocol byte buffer (a hook state entry, a `state_keys!`-encoded key
//! payload, ...), using the same little-endian, fixed-layout convention
//! [`crate::api::state::state_u64`] documents for its underlying state
//! entries (as opposed to that function's own big-endian "as-int64" wire
//! encoding). This lets the typed storage layer (`crate::state`'s
//! `state_get`/`state_set_loose`/`state_update_loose`) encode/decode
//! arbitrary fixed-size types without repeating that logic per call site.
//!
//! # Implementor's contract
//!
//! Every impl — here, in `types.rs`, or in a hook crate — must stay
//! panic-free, loop-free, and heap-free (DESIGN.md §2 C2/C7):
//! `wasm32v1-none` hook binaries have no allocator, and an unguarded loop
//! fails the Hook API's guard checker.
//!
//! - Never index with `buf[i]` (`clippy::indexing_slicing` is denied
//!   crate-wide) — use `.get()`/`.get_mut()` over a compile-time-constant
//!   range, then `copy_from_slice`. A compile-time-constant range keeps the
//!   copy a handful of inlined loads/stores instead of lowering to a
//!   `memcpy`/`memcmp` call with a runtime length.
//! - [`ToBytes::MAX_LEN`] must equal the exact number of bytes a successful
//!   [`ToBytes::write`] produces.
//! - [`ToBytes::write`] must not panic if `buf` is shorter than `MAX_LEN`:
//!   write nothing and return `0` instead.
//!
//! See DESIGN.md §5.6 ("Endianness conventions") for the full two-world rule.

use crate::error::{HookError, Result};

/// Encode `Self` into the front of a caller-provided buffer.
///
/// Mirrors this crate's caller-buffer convention (`state`, `hook_account`,
/// ...): implementations never allocate and never panic. See the module
/// doc comment for the full contract.
pub trait ToBytes {
    /// The exact number of bytes a successful [`ToBytes::write`] produces.
    const MAX_LEN: usize;

    /// Write `self`'s encoding into `buf[..Self::MAX_LEN]`.
    ///
    /// Returns `Self::MAX_LEN` (the number of bytes written) on success, or
    /// `0` if `buf` is shorter than `Self::MAX_LEN` (nothing is written in
    /// that case — never a partial write).
    fn write(&self, buf: &mut [u8]) -> usize;
}

/// Decode `Self` from a byte buffer.
pub trait FromBytes: Sized {
    /// Decode `Self` from `buf`.
    ///
    /// # Errors
    ///
    /// Returns [`HookError::TooSmall`] if `buf` is shorter than the
    /// encoding this type expects.
    fn read(buf: &[u8]) -> Result<Self>;
}

impl ToBytes for u8 {
    const MAX_LEN: usize = 1;

    #[inline(always)]
    fn write(&self, buf: &mut [u8]) -> usize {
        match buf.get_mut(..1) {
            Some(dst) => {
                dst.copy_from_slice(&self.to_le_bytes());
                1
            }
            None => 0,
        }
    }
}

impl FromBytes for u8 {
    #[inline(always)]
    fn read(buf: &[u8]) -> Result<Self> {
        let src = buf.get(..1).ok_or(HookError::TooSmall)?;
        let mut arr = [0u8; 1];
        arr.copy_from_slice(src);
        Ok(u8::from_le_bytes(arr))
    }
}

impl ToBytes for u16 {
    const MAX_LEN: usize = 2;

    #[inline(always)]
    fn write(&self, buf: &mut [u8]) -> usize {
        match buf.get_mut(..2) {
            Some(dst) => {
                dst.copy_from_slice(&self.to_le_bytes());
                2
            }
            None => 0,
        }
    }
}

impl FromBytes for u16 {
    #[inline(always)]
    fn read(buf: &[u8]) -> Result<Self> {
        let src = buf.get(..2).ok_or(HookError::TooSmall)?;
        let mut arr = [0u8; 2];
        arr.copy_from_slice(src);
        Ok(u16::from_le_bytes(arr))
    }
}

impl ToBytes for u32 {
    const MAX_LEN: usize = 4;

    #[inline(always)]
    fn write(&self, buf: &mut [u8]) -> usize {
        match buf.get_mut(..4) {
            Some(dst) => {
                dst.copy_from_slice(&self.to_le_bytes());
                4
            }
            None => 0,
        }
    }
}

impl FromBytes for u32 {
    #[inline(always)]
    fn read(buf: &[u8]) -> Result<Self> {
        let src = buf.get(..4).ok_or(HookError::TooSmall)?;
        let mut arr = [0u8; 4];
        arr.copy_from_slice(src);
        Ok(u32::from_le_bytes(arr))
    }
}

impl ToBytes for u64 {
    const MAX_LEN: usize = 8;

    #[inline(always)]
    fn write(&self, buf: &mut [u8]) -> usize {
        match buf.get_mut(..8) {
            Some(dst) => {
                dst.copy_from_slice(&self.to_le_bytes());
                8
            }
            None => 0,
        }
    }
}

impl FromBytes for u64 {
    #[inline(always)]
    fn read(buf: &[u8]) -> Result<Self> {
        let src = buf.get(..8).ok_or(HookError::TooSmall)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(src);
        Ok(u64::from_le_bytes(arr))
    }
}

impl ToBytes for i64 {
    const MAX_LEN: usize = 8;

    #[inline(always)]
    fn write(&self, buf: &mut [u8]) -> usize {
        match buf.get_mut(..8) {
            Some(dst) => {
                dst.copy_from_slice(&self.to_le_bytes());
                8
            }
            None => 0,
        }
    }
}

impl FromBytes for i64 {
    #[inline(always)]
    fn read(buf: &[u8]) -> Result<Self> {
        let src = buf.get(..8).ok_or(HookError::TooSmall)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(src);
        Ok(i64::from_le_bytes(arr))
    }
}

impl ToBytes for crate::xfl::XFL {
    // An XFL is an opaque wrapper over a raw `i64` bit pattern (see
    // `xfl.rs`), so it shares `i64`'s width and little-endian convention.
    const MAX_LEN: usize = 8;

    #[inline(always)]
    fn write(&self, buf: &mut [u8]) -> usize {
        self.raw_bits().write(buf)
    }
}

impl FromBytes for crate::xfl::XFL {
    #[inline(always)]
    fn read(buf: &[u8]) -> Result<Self> {
        i64::read(buf).map(crate::xfl::XFL::from_raw_bits)
    }
}

/// Decodes the 8 bytes as the same little-endian raw `i64` bit pattern
/// [`ToBytes`]/[`FromBytes`] above use, with the same "exactly 8 bytes or
/// `TooSmall`" contract as `<[u8; 8]>::read_exact`.
impl FixedRead for crate::xfl::XFL {
    #[inline(always)]
    fn read_exact(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
        <[u8; 8]>::read_exact(read)
            .map(i64::from_le_bytes)
            .map(crate::xfl::XFL::from_raw_bits)
    }
}

impl<const N: usize> ToBytes for [u8; N] {
    const MAX_LEN: usize = N;

    #[inline(always)]
    fn write(&self, buf: &mut [u8]) -> usize {
        match buf.get_mut(..N) {
            Some(dst) => {
                dst.copy_from_slice(self);
                N
            }
            None => 0,
        }
    }
}

impl<const N: usize> FromBytes for [u8; N] {
    #[inline(always)]
    fn read(buf: &[u8]) -> Result<Self> {
        let src = buf.get(..N).ok_or(HookError::TooSmall)?;
        let mut out = [0u8; N];
        out.copy_from_slice(src);
        Ok(out)
    }
}

/// A type whose value can be read in one shot as a fixed-size buffer from a
/// caller-buffer Hook API wrapper (`otxn_field`, `hook_param`, `slot`,
/// `state`) — the trait backing
/// [`crate::api::otxn::otxn_field_exact`]/[`crate::api::hook_ctx::hook_param_exact`]/
/// [`crate::api::slot::slot_exact`]/[`crate::api::state::state_exact`].
///
/// Implemented here for `[u8; N]` (any `N`), and — via the
/// `fixed_bytes_type!` macro — in `types.rs` for every `rshooks::types`
/// newtype. Each impl knows its own exact length, so
/// `otxn_field_exact::<AccountId>(sfAccount)` reads exactly `ACC_ID_LEN`
/// bytes without a caller-supplied `N`; inferring the return type from a
/// `let` binding's annotation (`let sender: AccountId =
/// otxn_field_exact(sfAccount)?;`) avoids a turbofish entirely — an
/// unannotated binding is a compile error, same as any unconstrained
/// generic return type.
///
/// # Why `read_exact` takes a closure, not a length
///
/// A single generic function can't allocate `[0u8; T::SOME_ASSOCIATED_LEN]`
/// — using an associated constant as an array length needs
/// `generic_const_exprs`, unstable on this toolchain. `read_exact` moves
/// that allocation into each concrete `impl`, where the length is a
/// literal; the caller-buffer wrapper (`otxn_field`, `state`, ...) is
/// passed in as a closure, so each `*_exact` function stays one generic
/// function per Hook API call rather than a hand-written non-generic
/// function per `T`.
pub trait FixedRead: Sized {
    /// Allocates this type's own fixed-size buffer, calls `read` with it,
    /// and returns `Self` if `read` reports writing exactly that many
    /// bytes.
    ///
    /// # Errors
    ///
    /// Propagates any error `read` returns. Returns
    /// [`HookError::TooSmall`] if `read` succeeds but reports writing a
    /// different number of bytes than this type's exact length (covers
    /// both "wrote fewer bytes" — the common case, e.g. no such field — and
    /// "wrote more," which `read`'s exactly-sized buffer argument should
    /// already prevent at the host level, but isn't assumed here).
    fn read_exact(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self>;
}

impl<const N: usize> FixedRead for [u8; N] {
    #[inline(always)]
    fn read_exact(read: impl FnOnce(&mut [u8]) -> Result<usize>) -> Result<Self> {
        let mut out = [0u8; N];
        let written = read(&mut out)?;
        if written == N {
            Ok(out)
        } else {
            Err(HookError::TooSmall)
        }
    }
}

/// Maximum length, in bytes, of a `hook_param`/`otxn_param` parameter
/// *name* — the Hook API's own bound on the `read_len` argument naming the
/// parameter (`hook_api.h`: `TOO_BIG` above 32 bytes, `TOO_SMALL` below 1).
///
/// A parameter name is **not** a fixed-32-byte, zero-padded key the way a
/// [`crate::types::StateKey`] is — it is matched at its own *natural*
/// length: `"MIN"` (3 bytes) and a zero-padded 32-byte version of the same
/// bytes name two different parameters, not one. [`crate::ParamName`]'s
/// encoding reflects that: a name's wire bytes are exactly its
/// [`ToBytes::MAX_LEN`], never padded — this constant is only the bound
/// (`1..=32`) the encoded length must satisfy.
pub const PARAM_NAME_MAX_LEN: usize = 32;

/// Pairs a Hook API parameter **name** type with the one **value** type
/// it's read as — this module's counterpart to
/// [`crate::state::TypedStateKey`]: implement it for a name type, then call
/// [`crate::api::hook_ctx::hook_param_typed`]/[`crate::api::otxn::otxn_param_typed`]
/// with a reference to a name value — the accessor resolves
/// [`Value`](Self::Value) from the name argument itself.
/// [`crate::api::hook_ctx::hook_param_exact`]/
/// [`crate::api::otxn::otxn_param_exact`] take a raw `&[u8]` name and the
/// value type `T` as independent arguments instead, so nothing stops
/// calling `otxn_param_exact::<WrongType>(b"INS")` for a name/type pairing
/// that was never intended; `TypedParamName` closes that gap.
///
/// Named `TypedParamName`, not `ParamName`, to avoid colliding with
/// [`crate::ParamName`] — the derive macro that gives a composite name
/// struct its [`ToBytes`] impl (this trait's supertrait) but does not
/// itself implement `TypedParamName`. See [`crate::ParamName`]'s and
/// [`crate::ParamValue`]'s doc comments for the two derives backing the
/// "name" (`Self`) and "value" (`Self::Value`) sides of this trait.
///
/// A Hook API parameter name is a genuine **variable-length key of up to
/// [`PARAM_NAME_MAX_LEN`] (32) bytes** — the same shape as a hook state
/// key — so `Self` may be any [`ToBytes`] type, not just a plain marker: a
/// whole composite [`crate::ParamName`]-derived struct works like a
/// composite state key, with the one difference that a parameter name is
/// encoded at its own **natural** length, never zero-padded.
///
/// # Cost
///
/// [`with_name_bytes`](Self::with_name_bytes)'s default body runs
/// `self.write(..)` into a [`PARAM_NAME_MAX_LEN`]-byte scratch buffer on
/// every call (there's no stable way to run a trait method at compile
/// time). A **plain byte-string** name has nothing to compute — its wire
/// encoding *is* its in-memory representation — so a hand-written
/// `TypedParamName` impl can override `with_name_bytes` to hand the
/// `'static` literal straight to the closure, using the same direct
/// host-call path as `hook_param_exact`/`otxn_param_exact`. A
/// **composite** name (more than one field, or an inner type that isn't
/// itself a plain byte string) can't skip encoding, but a hand-written
/// override still doesn't need the full 32-byte scratch buffer: in a
/// concrete, non-generic `impl`, `[0u8; Self::MAX_LEN]` is ordinary stable
/// Rust (unlike in the generic default below, which lacks
/// `generic_const_exprs` — the same restriction
/// [`FixedRead::read_exact`]'s doc comment describes), so the override can
/// allocate exactly [`Self::MAX_LEN`](ToBytes::MAX_LEN) bytes. See
/// `examples/12_typed-data`'s README for a measured example — a
/// right-sized buffer compiles to the same cost as the raw, un-abstracted
/// host call this typed layer replaces.
///
/// # Relationship to the hook-state typed layer
///
/// Mirrors [`crate::state::TypedStateKey`] (see its doc comment for the
/// full comparison): implement the pairing once, then call an accessor
/// that takes a reference to a name/key value and resolves the paired type
/// (`hook_param_typed`/`otxn_param_typed` here,
/// `state_get_typed`/`state_set_typed`/`state_update_typed` there) — no
/// turbofish, no chance of a mismatch. The one difference: a parameter is
/// read-only from the reading hook's own perspective, so there is no
/// `state_set_typed`/`state_update_typed` counterpart here.
pub trait TypedParamName: ToBytes {
    /// The one value type this name is paired with.
    type Value: FixedRead;

    /// Encodes `self`'s parameter-name bytes and hands the result to `f`,
    /// returning whatever `f` returns.
    ///
    /// A closure rather than a returned `&[u8]` lets each implementation
    /// choose where the encoded bytes live (a `'static` literal, a
    /// right-sized stack buffer, ...) without the signature pinning one
    /// fixed buffer shape — see this trait's "Cost" section.
    ///
    /// The default runs `self.write(..)` into a [`PARAM_NAME_MAX_LEN`]-byte
    /// scratch buffer — the only option generic over an arbitrary
    /// [`ToBytes`] `Self`. A hand-written override is free to do better;
    /// see this trait's doc comment.
    ///
    /// A compile-time check (monomorphized per `Self`) rejects a `Self`
    /// whose [`ToBytes::MAX_LEN`] falls outside `1..=`[`PARAM_NAME_MAX_LEN`]
    /// — the Hook API's own bound on a parameter name's length. An
    /// override **replaces** that check along with the body, so it must
    /// carry its own copy to keep the same guarantee.
    #[inline(always)]
    fn with_name_bytes<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        const {
            assert!(
                <Self as ToBytes>::MAX_LEN >= 1,
                "rshooks: TypedParamName::MAX_LEN must be at least 1 byte \
                 (the Hook API's parameter-name lower bound)"
            );
            assert!(
                <Self as ToBytes>::MAX_LEN <= PARAM_NAME_MAX_LEN,
                "rshooks: TypedParamName::MAX_LEN exceeds the Hook API's \
                 32-byte parameter-name upper bound"
            );
        }
        let mut buf = [0u8; PARAM_NAME_MAX_LEN];
        let n = self.write(&mut buf);
        f(buf.get(..n).unwrap_or(&[]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u8_round_trips() {
        let mut buf = [0u8; 1];
        assert_eq!(0xABu8.write(&mut buf), 1);
        assert_eq!(buf, [0xAB]);
        assert_eq!(u8::read(&buf), Ok(0xABu8));
    }

    #[test]
    fn u8_write_into_empty_buffer_writes_nothing() {
        let mut buf: [u8; 0] = [];
        assert_eq!(0xABu8.write(&mut buf), 0);
    }

    #[test]
    fn u8_read_from_empty_buffer_fails() {
        assert_eq!(u8::read(&[]), Err(HookError::TooSmall));
    }

    #[test]
    fn u16_round_trips() {
        let mut buf = [0u8; 2];
        assert_eq!(0x0102u16.write(&mut buf), 2);
        assert_eq!(buf, 0x0102u16.to_le_bytes());
        assert_eq!(u16::read(&buf), Ok(0x0102u16));
    }

    #[test]
    fn u16_write_into_short_buffer_writes_nothing() {
        let mut buf = [0xFFu8; 1];
        assert_eq!(0x0102u16.write(&mut buf), 0);
        assert_eq!(buf, [0xFF]);
    }

    #[test]
    fn u16_read_from_short_buffer_fails() {
        assert_eq!(u16::read(&[0u8; 1]), Err(HookError::TooSmall));
    }

    #[test]
    fn u32_round_trips() {
        let mut buf = [0u8; 4];
        assert_eq!(42u32.write(&mut buf), 4);
        assert_eq!(buf, 42u32.to_le_bytes());
        assert_eq!(u32::read(&buf), Ok(42u32));
    }

    #[test]
    fn u32_write_into_short_buffer_writes_nothing() {
        let mut buf = [0xFFu8; 3];
        assert_eq!(42u32.write(&mut buf), 0);
        assert_eq!(buf, [0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn u32_read_from_short_buffer_fails() {
        assert_eq!(u32::read(&[0u8; 3]), Err(HookError::TooSmall));
    }

    #[test]
    fn u64_round_trips() {
        let mut buf = [0u8; 8];
        assert_eq!(0x0102_0304_0506_0708u64.write(&mut buf), 8);
        assert_eq!(u64::read(&buf), Ok(0x0102_0304_0506_0708u64));
    }

    #[test]
    fn i64_round_trips() {
        let mut buf = [0u8; 8];
        assert_eq!((-1i64).write(&mut buf), 8);
        assert_eq!(i64::read(&buf), Ok(-1i64));
    }

    #[test]
    fn fixed_array_round_trips() {
        let value = [1u8, 2, 3, 4, 5];
        let mut buf = [0u8; 5];
        assert_eq!(value.write(&mut buf), 5);
        assert_eq!(buf, value);
        assert_eq!(<[u8; 5]>::read(&buf), Ok(value));
    }

    #[test]
    fn xfl_round_trips_bit_pattern() {
        use crate::xfl::XFL;

        let value = XFL::from_raw_bits(0x1234_5678_9ABC_DEF0);
        let mut buf = [0u8; 8];
        assert_eq!(value.write(&mut buf), 8);
        assert_eq!(
            crate::xfl::XFL::read(&buf).map(XFL::raw_bits),
            Ok(0x1234_5678_9ABC_DEF0i64)
        );
    }

    #[test]
    fn fixed_read_array_succeeds_on_exact_write() {
        let result: Result<[u8; 4]> = <[u8; 4]>::read_exact(|buf| {
            buf.copy_from_slice(&[1, 2, 3, 4]);
            Ok(4)
        });
        assert_eq!(result, Ok([1, 2, 3, 4]));
    }

    #[test]
    fn fixed_read_array_rejects_short_write() {
        let result: Result<[u8; 4]> = <[u8; 4]>::read_exact(|_buf| Ok(3));
        assert_eq!(result, Err(HookError::TooSmall));
    }

    #[test]
    fn fixed_read_array_propagates_read_error() {
        let result: Result<[u8; 4]> = <[u8; 4]>::read_exact(|_buf| Err(HookError::InternalError));
        assert_eq!(result, Err(HookError::InternalError));
    }

    #[test]
    fn fixed_read_passes_a_buffer_of_exactly_n_bytes() {
        let _: Result<[u8; 7]> = <[u8; 7]>::read_exact(|buf| {
            assert_eq!(buf.len(), 7);
            Ok(buf.len())
        });
    }

    #[test]
    fn xfl_fixed_read_succeeds_on_exact_write() {
        use crate::xfl::XFL;
        let bits = 0x1234_5678_9ABC_DEF0i64;
        let result: Result<XFL> = XFL::read_exact(|buf| {
            buf.copy_from_slice(&bits.to_le_bytes());
            Ok(8)
        });
        assert_eq!(result.map(XFL::raw_bits), Ok(bits));
    }

    #[test]
    fn xfl_fixed_read_rejects_short_write() {
        use crate::xfl::XFL;
        let result: Result<XFL> = XFL::read_exact(|_buf| Ok(7));
        assert_eq!(result, Err(HookError::TooSmall));
    }

    #[test]
    fn xfl_fixed_read_propagates_read_error() {
        use crate::xfl::XFL;
        let result: Result<XFL> = XFL::read_exact(|_buf| Err(HookError::InternalError));
        assert_eq!(result, Err(HookError::InternalError));
    }
}
