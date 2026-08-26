//! Boundary conversion traits: [`ToBytes`]/[`FromBytes`].
//!
//! These traits fix exactly how a small, fixed-size Rust value crosses the
//! boundary into/out of a protocol byte buffer (a hook state entry, a
//! `state_keys!`-encoded key payload, ...). They generalize the "little-
//! endian, fixed layout" convention this crate already documents for
//! [`crate::api::state::state_u64`]'s *underlying state entries* (as
//! opposed to that function's own big-endian "as-int64" wire encoding —
//! see its doc comment) so the typed storage layer (`crate::state`'s
//! `state_get`/`state_set_loose`/`state_update_loose`) can encode/decode
//! arbitrary fixed-size types without repeating that logic per call site.
//!
//! # Implementor's contract
//!
//! Every impl of [`ToBytes`]/[`FromBytes`] — the ones in this module, the
//! newtype impls in `types.rs`, and any a hook crate adds for its own
//! types — must stay panic-free, loop-free, and heap-free, the same
//! constraints as every other rshooks wrapper (DESIGN.md §2 C2/C7):
//! `wasm32v1-none` hook binaries have no allocator, and an unguarded loop
//! fails the Hook API's guard checker. Concretely:
//!
//! - Never index with `buf[i]` (this crate denies `clippy::indexing_slicing`
//!   crate-wide) — use `.get()`/`.get_mut()` over a range whose bounds are
//!   compile-time constants, then `copy_from_slice`. Keeping the range
//!   compile-time-constant (never a runtime-computed length) is what keeps
//!   the copy a handful of inlined loads/stores instead of a lowering to a
//!   `memcpy`/`memcmp` call with a runtime length — exactly the std idiom
//!   DESIGN.md warns produces unguardable loops in a Hook binary.
//! - [`ToBytes::MAX_LEN`] must be a compile-time constant equal to the
//!   exact number of bytes a successful [`ToBytes::write`] produces.
//! - [`ToBytes::write`] must not panic if `buf` is shorter than `MAX_LEN`:
//!   write nothing and return `0` instead (mirrors this crate's other
//!   caller-buffer wrappers, which rely on the host's own bounds checking
//!   rather than panicking locally).
//!
//! See DESIGN.md §5.6 ("Endianness conventions") for the full two-world
//! rule this module's little-endian convention is one half of.
//!
//! # Uninitialized read buffers
//!
//! [`FixedRead::read_exact`]'s buffer (and the sibling "read a prefix,
//! classify by length" call sites this same shape shows up at —
//! [`crate::api::otxn::OtxnFieldValue`]'s `Amount`/`Issue` impls,
//! [`crate::api::state`]'s `state_update_*` family) allocates
//! [`core::mem::MaybeUninit`], never `[0u8; N]`. The host call these buffers
//! are handed to always *completely overwrites* them on the only path their
//! contents are ever read (see [`uninit_slice_mut`]'s doc comment for the
//! exact contract), so zero-initializing first is dead work the Hook API's
//! guard checker still charges for — a zeroed `[u8; N]` buffer whose address
//! escapes into an `extern` call is a store LLVM cannot prove dead, because
//! it cannot see across the FFI boundary that nothing ever reads the zeros.

use crate::error::{HookError, Result};

/// Forms a `&mut [u8]` view over `buf`'s storage without requiring it to be
/// initialized first — the primitive every host-call read buffer in this
/// crate uses instead of `[0u8; N]` zero-initialization (see the module doc
/// comment's "Uninitialized read buffers" section).
///
/// # Contract
///
/// The returned slice must only ever be read (directly, or indirectly via
/// [`MaybeUninit::assume_init`](core::mem::MaybeUninit::assume_init) on
/// `buf`) over the range a prior write into it actually covered. Every call
/// site in this crate satisfies that by handing the slice straight to a
/// Hook API host-call wrapper (`otxn_field`, `hook_param`, `state`, ...) as
/// its caller-buffer output parameter, then reading back only the prefix
/// (up to, and including exactly, the host's own reported "bytes written"
/// count) that call is documented to have written — never more. The host
/// call itself is `unsafe` `extern` FFI already fully trusted by every other
/// line of this crate (it is handed a raw pointer into guest linear memory
/// the host can read or write without restriction regardless of what this
/// function does), so trusting its reported byte count here adds no new
/// trust boundary.
///
/// # Safety
///
/// The caller must uphold the contract above. `u8` itself has no invalid
/// bit patterns and no padding, so forming the `&mut [u8]` here is sound
/// unconditionally — the requirement is entirely about what the caller does
/// with it afterward.
#[inline(always)]
pub(crate) unsafe fn uninit_slice_mut<const N: usize>(
    buf: &mut core::mem::MaybeUninit<[u8; N]>,
) -> &mut [u8] {
    // SAFETY: `buf` is `N` bytes of live, properly aligned storage (a
    // `MaybeUninit<[u8; N]>` has the same size and alignment as `[u8; N]`).
    // `u8` has no invalid bit patterns and no padding, so a `&mut [u8]` over
    // that storage is well-formed the instant it is created, whether or not
    // the storage has been written to yet — only reading through it before
    // it is written would be unsound, and this function's own safety
    // contract puts that burden on the caller.
    unsafe { core::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), N) }
}

/// Encode `Self` into the front of a caller-provided buffer.
///
/// Mirrors this crate's caller-buffer convention (`state`, `hook_account`,
/// ...): implementations never allocate and never panic. See the module
/// doc comment for the loop-free/panic-free contract every impl must
/// uphold.
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

/// Reuses `<[u8; 8] as FixedRead>::read_exact`'s exact-length machinery,
/// then decodes the 8 bytes as the same little-endian raw `i64` bit
/// pattern [`ToBytes`]/[`FromBytes`] above already use — so an `XFL`
/// parameter (e.g. `examples/81_govern`'s `IRR`/`IRD`) reads exactly the
/// bytes a raw `hook_param_exact::<[u8; 8]>` + `i64::from_le_bytes` +
/// `XFL::from_raw_bits` call chain would, with the identical "must be
/// exactly 8 bytes or `TooSmall`" contract.
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
/// newtype. Each impl knows its *own* exact length, so
/// `otxn_field_exact::<AccountId>(sfAccount)` reads exactly `ACC_ID_LEN`
/// bytes without a caller-supplied `N`; with the return type inferred from
/// a `let` binding's type annotation instead of a turbofish
/// (`let sender: AccountId = otxn_field_exact(sfAccount)?;`), no turbofish
/// is needed at all. A binding with no inferable type (no annotation, no
/// other usage that pins the type down) is a compile error, same as any
/// other unconstrained generic return type — annotate the binding.
///
/// # Why `read_exact` takes a closure, not a length
///
/// A single generic function can't allocate
/// `MaybeUninit::<[u8; T::SOME_ASSOCIATED_LEN]>::uninit()` — using an
/// associated constant as an array length needs `generic_const_exprs`,
/// unstable on this crate's pinned stable toolchain. `read_exact` sidesteps
/// that by moving the actual `MaybeUninit<[u8; N]>`/newtype buffer
/// allocation into *this trait method's own implementation*, one
/// per concrete `Self`, where the length is a literal the `impl` block
/// already knows — never a generic parameter's associated constant. The
/// caller-buffer wrapper function itself (`otxn_field`, `state`, ...) is
/// passed in as a closure, so each `*_exact` function stays a single
/// generic function (one per Hook API call), not one monomorphized-away
/// non-generic function per concrete `T` written by hand.
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
        let mut out = core::mem::MaybeUninit::<[u8; N]>::uninit();
        // SAFETY: `uninit_slice_mut`'s contract requires the slice to be
        // read only over the range `read` actually wrote — `written == N`
        // below confirms `read` wrote every one of `out`'s `N` bytes before
        // `assume_init` reads any of them.
        let written = read(unsafe { uninit_slice_mut(&mut out) })?;
        if written == N {
            // SAFETY: `written == N` means `read` reported writing all `N`
            // bytes of `out`'s storage, so `out` is now fully initialized.
            Ok(unsafe { out.assume_init() })
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
/// [`crate::types::StateKey`] is (see [`crate::state::StateKeyEncode`]) —
/// it is matched at its own *natural* length: `"MIN"` (3 bytes) and a
/// hypothetical zero-padded 32-byte version of the same three bytes name
/// two different parameters, not one. [`crate::ParamName`]'s encoding
/// reflects that: a name's wire bytes are exactly its
/// [`ToBytes::MAX_LEN`], never padded up to this constant — this constant
/// is only an upper (and, at `1`, a lower) bound the encoded length must
/// satisfy.
pub const PARAM_NAME_MAX_LEN: usize = 32;

/// Pairs a Hook API parameter **name** type with the one **value** type
/// it's read as — this module's counterpart to
/// [`crate::state::TypedStateKey`], deliberately shaped the same way:
/// implement it for a name type, then call
/// [`crate::api::hook_ctx::hook_param_typed`]/[`crate::api::otxn::otxn_param_typed`]
/// with **a reference to a name value** — the accessor resolves
/// [`Value`](Self::Value) from the name argument itself, exactly like
/// [`crate::state::state_get_typed`] resolves `K::Value` from the key
/// argument. [`crate::api::hook_ctx::hook_param_exact`]/
/// [`crate::api::otxn::otxn_param_exact`] take a raw `&[u8]` name and the
/// value type `T` as two *independent* arguments instead — nothing stops
/// calling `otxn_param_exact::<WrongType>(b"INS")` for a name/type pairing
/// that was never intended, as long as `WrongType: FixedRead` (true of
/// nearly every fixed-size type this crate provides); `TypedParamName`
/// closes that gap the same way [`crate::state::TypedStateKey`] closes it
/// for state keys.
///
/// Named `TypedParamName`, not `ParamName` — a name that would collide
/// with [`crate::ParamName`], the *derive macro* that gives a composite
/// name struct its [`ToBytes`] impl (this trait's supertrait). Keeping the
/// two apart avoids the misleading suggestion that `#[derive(ParamName)]`
/// implements this trait itself — it doesn't; the derive only provides the
/// `ToBytes` encoding a hand-written `TypedParamName` impl then builds on. See
/// [`crate::ParamName`]'s and [`crate::ParamValue`]'s doc comments for the
/// two derives backing the "name" (`Self`) and "value" (`Self::Value`)
/// sides of this trait, respectively.
///
/// A Hook API parameter name is a genuine **variable-length key of up to
/// [`PARAM_NAME_MAX_LEN`] (32) bytes** — the same shape as a hook state key
/// (see [`crate::state::StateKeyEncode`]) — so `Self` may be any
/// [`ToBytes`] type, not just a plain marker: a whole composite
/// [`crate::ParamName`]-derived struct works exactly like a composite
/// state key, with the one difference that a parameter name is encoded at
/// its own **natural** length, never zero-padded to a fixed size.
///
/// # Zero-cost for the (overwhelmingly common) plain-byte-string case
///
/// Encoding an arbitrary `ToBytes` value into bytes is, in general, a real
/// (if small) runtime computation — [`with_name_bytes`](Self::with_name_bytes)'s
/// default body runs `self.write(..)` once per call (Rust has no stable
/// way to run a trait method at compile time yet). But a **plain
/// byte-string name** has nothing to compute: its wire encoding *is* its
/// in-memory representation. A hand-written `TypedParamName` impl for a
/// plain byte-string name overrides `with_name_bytes` to hand the literal
/// straight to the closure: no copy, no buffer, nothing to encode, skipping
/// the default body entirely.
/// Plain byte-string names use the same direct host-call path as
/// `hook_param_exact` and `otxn_param_exact`.
///
/// # Near-zero-cost for the composite (struct-shaped) case too
///
/// A **composite** name (a [`crate::ParamName`]-derived struct with more
/// than one field, or a newtype whose inner type isn't itself a plain byte
/// string) can't skip encoding entirely: something has to lay its fields
/// out into contiguous bytes. But it doesn't need [`PARAM_NAME_MAX_LEN`]
/// (32) bytes of stack scratch to do it, zero-initialized fresh on every
/// call, only to use the first handful — a hand-written `TypedParamName`
/// impl for a composite name can override `with_name_bytes` to allocate
/// exactly
/// [`Self::MAX_LEN`](ToBytes::MAX_LEN) bytes instead: a compile-time
/// literal at that impl's own (concrete, non-generic) definition site, so
/// `[0u8; Self::MAX_LEN]` is ordinary stable Rust there, even though the
/// *default* implementation below (generic over any `Self: ToBytes`, with
/// no way to use an associated const as an array length in generic code —
/// the same restriction [`FixedRead::read_exact`]'s doc comment describes)
/// cannot do the same and falls back to the full [`PARAM_NAME_MAX_LEN`]
/// scratch buffer. See `examples/12_typed-data`'s README for a measured
/// example — a right-sized buffer compiles to the same cost as the raw,
/// un-abstracted host call this typed layer replaces.
///
/// # Relationship to the hook-state typed layer
///
/// `TypedParamName` deliberately mirrors [`crate::state::TypedStateKey`]
/// (see its doc comment for the full comparison table): implement the
/// pairing once (a hand-written `TypedParamName` impl here, a hand-written
/// `TypedStateKey` impl there), then call an accessor that takes **a
/// reference to a name/key value** and resolves the paired type from it
/// (`hook_param_typed`/
/// `otxn_param_typed` here, `state_get_typed`/`state_set_typed`/`state_update_typed`
/// there) — no turbofish, no chance of a mismatch. The one difference: a
/// parameter is read-only from the reading hook's own perspective, so
/// there is no `hook_param`/`otxn_param` counterpart to `state_set_typed`/
/// `state_update_typed`.
pub trait TypedParamName: ToBytes {
    /// The one value type this name is paired with.
    type Value: FixedRead;

    /// Encodes `self`'s parameter-name bytes and hands the result to `f`,
    /// returning whatever `f` returns.
    ///
    /// A closure, not a returned `&[u8]`/an out-buffer parameter, so each
    /// implementation is free to choose *where* the encoded bytes live —
    /// a `'static` literal (the zero-copy plain-byte-string case), or a
    /// stack buffer sized exactly to `Self`'s own encoded length (the
    /// composite case) — without the trait method's signature pinning
    /// that choice to one fixed-size buffer shape. See this trait's
    /// "Zero-cost"/"Near-zero-cost" doc sections for both cases in detail.
    ///
    /// The default implementation runs `self.write(..)` into a
    /// [`PARAM_NAME_MAX_LEN`]-byte scratch buffer (the only option
    /// available to code that is generic over an arbitrary [`ToBytes`]
    /// `Self` — using [`Self::MAX_LEN`](ToBytes::MAX_LEN) as an array
    /// length isn't legal in a *generic* default trait method on stable
    /// Rust, only inside a concrete, non-generic `impl` — see
    /// [`FixedRead::read_exact`]'s doc comment for the identical
    /// restriction). A hand-written `TypedParamName` impl is free to
    /// override this default: a plain-byte-string name can hand `f` the
    /// `'static` literal directly (no buffer at all); a composite name can
    /// allocate a `[u8; Self::MAX_LEN]` buffer sized exactly to itself, in
    /// its own concrete `impl` — see this trait's doc comment.
    ///
    /// A compile-time check (monomorphized per `Self`) rejects a `Self`
    /// whose [`ToBytes::MAX_LEN`] falls outside `1..=`[`PARAM_NAME_MAX_LEN`]
    /// — the Hook API's own bound on a parameter name's length.
    ///
    /// An override **replaces** that check along with the body, so any
    /// override must carry its own copy of the same assertion if it wants
    /// the same guarantee — without it, a name encoding to 0 bytes
    /// (rejected by the host at runtime) or to a multi-kilobyte scratch
    /// buffer would compile with no complaint at all.
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
