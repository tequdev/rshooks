//! Guard, control-flow, and trace macros.
//!
//! `guard!`/`guard_m!` match the C `GUARD`/`GUARDM` macros from `macro.h`
//! exactly, including the `+ 1` on `maxiter`:
//! `GUARD(maxiter)` in C is `_g((1ULL << 31U) + __LINE__, (maxiter) + 1)`.
//! The `unsafe` call to `_g` lives inside the macro expansion, so these are
//! usable from safe code without an `unsafe` block at the call site.
//!
//! `uninit_buf!` is deliberately NOT provided:
//! `MaybeUninit::uninit().assume_init()` for a byte array is UB for this use
//! case. Buffers are always `[0u8; N]`.

/// Guard a loop against exceeding `maxiter` iterations, per the Hook API's
/// static guard-check requirement (see DESIGN.md §2 C2). Matches the C
/// `GUARD` macro's guard-id formula exactly, including the `+ 1` on
/// `maxiter`.
///
/// # Examples
///
/// ```
/// use rshooks::guard;
///
/// let mut i = 0;
/// loop {
///     guard!(10);
///     if i >= 3 {
///         break;
///     }
///     i += 1;
/// }
/// assert_eq!(i, 3);
/// ```
#[macro_export]
macro_rules! guard {
    ($m:expr) => {
        unsafe { $crate::raw::_g((1u32 << 31) + line!(), ($m) + 1) }
    };
}

/// Like [`guard!`], but for multiple loops that share one source line (the
/// `$n` disambiguates them). Matches the C `GUARDM` macro's guard-id formula
/// exactly.
#[macro_export]
macro_rules! guard_m {
    ($m:expr, $n:expr) => {
        unsafe { $crate::raw::_g((1u32 << 31) + (line!() << 16) + ($n), ($m) + 1) }
    };
}

/// Defeats full loop unrolling for a small, fixed-trip-count loop whose body
/// contains further [`guard!`]-protected work, by making the loop's own
/// trip count opaque to the optimizer. Wrap the loop's induction variable at
/// its comparison, e.g. `while no_unroll(i) < N { .. }`.
///
/// # Why this exists
///
/// `guard!`'s worst-case-instruction-count model assumes each `guard!` call
/// site is a single point in the compiled module, entered up to `maxiter`
/// times across the whole hook execution — a nested loop's inner `guard!`
/// is meant to amortize its cost across every outer iteration that reaches
/// it (the outer loop's own `guard!` already bounds how many times that
/// can happen). This holds as long as the compiled module actually
/// contains one physical copy of the inner loop.
///
/// At `opt-level = 3`, LLVM routinely fully unrolls a small, provably-
/// bounded outer loop (2 or 3 iterations is a typical threshold) whenever
/// it judges duplicating the body worthwhile — completely independent of
/// whether that body is written inline or behind a function call (the
/// flatten pass inlines every defined non-entry function into
/// `hook()`/`cbak()` regardless, so unrolling and inlining compound rather
/// than substitute for each other — `docs/DESIGN.md` §6.2b). When the
/// outer loop wraps an inner `guard!`-protected loop, unrolling physically
/// duplicates that inner loop once per outer iteration: the checker (which
/// walks the compiled bytecode, not the source) then counts the inner
/// loop's full worst-case cost *once per duplicate* instead of once total
/// — silently multiplying, rather than amortizing, its contribution to the
/// worst-case instruction count. Measured on `examples/80_governance`'s
/// vote-garbage-collection loop (an outer 2-iteration table loop wrapping a
/// `guard!(66)`-bounded 32-topic scan): unrolling doubled that loop's
/// measured cost, worth roughly a third of the whole `govern` entry's
/// worst-case instruction count.
///
/// `no_unroll` breaks this by routing the loop's induction variable through
/// [`core::hint::black_box`] at its comparison: the optimizer can no longer
/// prove the trip count at compile time, so it keeps the loop as one real
/// `loop` construct instead of duplicating its body. This changes no
/// observable behavior (`black_box` is the identity function) — only which
/// optimizations the compiler is allowed to apply.
///
/// Only worth reaching for at a call site actually exhibiting this
/// pathology (an outer loop, itself small enough to be a plausible full-
/// unroll candidate, wrapping further `guard!`-protected work) — a plain
/// small loop with a cheap, unguarded body is usually *cheaper* fully
/// unrolled (straight-line code has no loop-control overhead and no
/// worst-case-padding waste), so applying this unconditionally to every
/// `guard!`-protected loop is not a good default.
///
/// # Failure mode
///
/// `black_box` is a best-effort optimization barrier, not a guaranteed
/// one — the compiler is not contractually required to honor it, only
/// documented (as of this writing) to do so in practice. If a future
/// toolchain version ever stopped treating it as opaque, this function
/// would still behave identically (it is the identity function either
/// way), but the unrolling this doc comment describes could silently
/// return, regressing the caller's worst-case instruction count with no
/// compile-time or runtime signal — nothing below the protocol's 65535
/// ceiling would catch a regression that stays under it. Guard against
/// this by measuring the worst case after any toolchain upgrade for a hook
/// that relies on `no_unroll`.
///
/// # Examples
///
/// ```
/// use rshooks::{guard, no_unroll};
///
/// let mut outer = 0u8;
/// while no_unroll(outer) < 2 {
///     guard!(2);
///     let this_outer = outer;
///     outer += 1;
///
///     let mut inner = 0u8;
///     while inner < 32 {
///         guard!(64);
///         inner += 1;
///         let _ = this_outer;
///     }
/// }
/// assert_eq!(outer, 2);
/// ```
#[inline(always)]
#[must_use] // `no_unroll(x);` as a bare statement is silently a no-op
pub fn no_unroll<T>(value: T) -> T {
    core::hint::black_box(value)
}

/// Terminate hook execution successfully. `accept!()` sends no message and
/// error code `0`; `accept!(msg, code)` forwards both.
///
/// `code` may be a plain `i64` (including untyped integer literals like
/// `-1`) or any value whose type implements `Into<i64>` — e.g. an enum
/// defined with [`hook_errors!`](crate::hook_errors) — since it is passed
/// through `i64::from(code)` before reaching the host call.
#[macro_export]
macro_rules! accept {
    () => {
        $crate::api::control::accept(&[], 0)
    };
    ($msg:expr, $code:expr) => {
        $crate::api::control::accept($msg, i64::from($code))
    };
}

/// Terminate hook execution with a failure, rolling back state changes.
///
/// `code` accepts the same `i64`-literal-or-`Into<i64>` forms as
/// [`accept!`] — see its doc comment for details.
#[macro_export]
macro_rules! rollback {
    ($msg:expr, $code:expr) => {
        $crate::api::control::rollback($msg, i64::from($code))
    };
}

/// Emit a debug trace message. Compiles to nothing unless **rshooks's**
/// `trace` feature is enabled (traces cost bytes and execution time) —
/// enable it from a hook crate with
/// `rshooks = { ..., features = ["trace"] }`; no feature re-declaration
/// in the hook crate is needed.
#[macro_export]
macro_rules! trace {
    ($msg:expr) => {
        $crate::api::trace::__macro_support::trace_maybe($msg, &[], false)
    };
    ($msg:expr, $data:expr) => {
        $crate::api::trace::__macro_support::trace_maybe($msg, $data, false)
    };
}

/// Emit a debug trace message followed by an integer. Compiles to nothing
/// unless rshooks's `trace` feature is enabled (see [`trace!`]).
#[macro_export]
macro_rules! trace_num {
    ($msg:expr, $number:expr) => {
        $crate::api::trace::__macro_support::trace_num_maybe($msg, $number)
    };
}

/// Emit a debug trace message followed by an XFL value. Compiles to nothing
/// unless rshooks's `trace` feature is enabled (see [`trace!`]).
#[macro_export]
macro_rules! trace_float {
    ($msg:expr, $value:expr) => {
        $crate::api::trace::__macro_support::trace_float_maybe($msg, $value)
    };
}

/// Compile-time zero-padding helper backing [`pad!`](crate::pad).
///
/// Copies `src` into the start of a zeroed `[u8; N]`. The `pad!` macro wraps
/// every call in an inline `const` block, so the `assert!` and the indexing
/// below are compile-time checks — they can never become runtime panics.
#[doc(hidden)]
#[allow(clippy::indexing_slicing)] // in-bounds by the assert, const-evaluated only
pub const fn padded_bytes<const N: usize>(src: &[u8]) -> [u8; N] {
    assert!(
        src.len() <= N,
        "pad!: source is larger than the destination"
    );

    let mut output = [0u8; N];
    let mut i = 0;

    while i < src.len() {
        output[i] = src[i];
        i = i.wrapping_add(1);
    }

    output
}

/// Zero-pad a constant byte string to a fixed-size array, at compile time.
///
/// The array length is inferred from the surrounding context. The expansion
/// is an inline `const` block, which has two consequences:
///
/// - the argument must be a constant expression (e.g. a byte-string
///   literal), and a source longer than the destination is a **compile
///   error**, never a runtime panic;
/// - the padded array is baked into the binary at compile time — no copy or
///   zeroing loop exists at runtime, so no loop guard is needed for it.
///
/// This right-pads (`src` at the front, zero bytes at the end).
///
/// **Not needed for building a short hook-state key anymore**: a
/// `[u8; N]` (`1 <= N <= `[`crate::types::STATE_KEY_LEN`]) works directly
/// as a [`crate::state::StateKeyEncode`] key — e.g. `state_get::<u64>(b"counter")` —
/// sent to the host at its own real length; the host itself left-pads a
/// short key internally (see `rshooks::state`'s module doc comment, "Key
/// length and padding," and DESIGN.md §5.7). Reach for `pad!` when a
/// fixed-size buffer genuinely needs local right-padding for some other
/// reason — e.g. building a full, already-32-byte
/// [`crate::types::StateKey`]/[`crate::types::NameSpace`] constant on
/// purpose, or padding a byte string for a use unrelated to hook-state
/// keys. [`pad_left!`](crate::pad_left) is the mirror-image macro (left-pad
/// instead of right-pad) for the same kind of non-key use — see its own
/// doc comment.
///
/// # Examples
/// ```
/// use rshooks::pad;
/// use rshooks::types::StateKey;
///
/// let padded: [u8; 10] = pad!(b"hello");
/// assert_eq!(padded, [b'h', b'e', b'l', b'l', b'o', 0, 0, 0, 0, 0]);
///
/// // `StateKey` is a `#[repr(transparent)]` newtype over `[u8; 32]` (see
/// // `types.rs`), so `pad!` (which returns a plain `[u8; N]`) is wrapped
/// // explicitly via the newtype's public tuple field.
/// const KEY: StateKey = StateKey(pad!(b"counter"));
/// assert!(KEY.starts_with(b"counter"));
/// ```
///
/// A source longer than the destination fails to compile:
/// ```compile_fail
/// let too_small: [u8; 4] = rshooks::pad!(b"hello");
/// ```
#[macro_export]
macro_rules! pad {
    ($value:expr) => {
        const { $crate::padded_bytes($value) }
    };
}

/// Compile-time zero-padding helper backing [`pad_left!`](crate::pad_left).
///
/// Copies `src` into the *end* of a zeroed `[u8; N]` — the mirror image of
/// [`padded_bytes`], which copies `src` into the start. The `pad_left!`
/// macro wraps every call in an inline `const` block, so the `assert!` and
/// the indexing below are compile-time checks — they can never become
/// runtime panics.
#[doc(hidden)]
#[allow(clippy::indexing_slicing)] // in-bounds by the assert, const-evaluated only
pub const fn padded_bytes_left<const N: usize>(src: &[u8]) -> [u8; N] {
    assert!(
        src.len() <= N,
        "pad_left!: source is larger than the destination"
    );

    let mut output = [0u8; N];
    let offset = N.wrapping_sub(src.len());
    let mut i = 0;

    while i < src.len() {
        output[offset.wrapping_add(i)] = src[i];
        i = i.wrapping_add(1);
    }

    output
}

/// Zero-pad a constant byte string to a fixed-size array, at compile time —
/// on the **left** (zero bytes first, `src` at the end), the mirror image
/// of [`pad!`](crate::pad).
///
/// The host itself left-pads a state/param key shorter than the fixed key
/// width (32 bytes for hook state, 1–32 bytes for hook/otxn parameters) —
/// see DESIGN.md §5.6 ("Endianness conventions") for that host-side
/// behavior, and §5.7 ("Hook state key encoding") for why a Rust hook does
/// **not** need to reproduce it locally for an ordinary `StateKeyEncode`
/// key (a plain `[u8; N]`, `state_keys!` variant, or `#[derive(HookKey)]`
/// struct passed at its own real length already lands on the same slot the
/// host's left-pad produces — no `pad_left!` involved). Reach for
/// `pad_left!` instead when a hook genuinely needs the *already-padded* 32
/// bytes themselves as a value — e.g. reproducing, byte-for-byte, what the
/// host's left-pad of a given short key would look like, for a purpose
/// other than passing it to `state`/`state_set` (which never needs this:
/// pass the short key directly).
///
/// Same compile-time-only shape as [`pad!`](crate::pad): the array length
/// is inferred from context, the argument must be a constant expression, a
/// source longer than the destination is a compile error, and the padded
/// array is baked into the binary at compile time (no runtime copy/zeroing
/// loop, so no loop guard is needed for it).
///
/// # Examples
/// ```
/// use rshooks::pad_left;
/// use rshooks::types::StateKey;
///
/// let padded: [u8; 10] = pad_left!(b"hello");
/// assert_eq!(padded, [0, 0, 0, 0, 0, b'h', b'e', b'l', b'l', b'o']);
///
/// // The same short C-hook-style key, reproduced so a Rust hook lands on
/// // the exact same state slot.
/// const KEY: StateKey = StateKey(pad_left!(b"counter"));
/// assert!(KEY.ends_with(b"counter"));
/// ```
///
/// A source longer than the destination fails to compile:
/// ```compile_fail
/// let too_small: [u8; 4] = rshooks::pad_left!(b"hello");
/// ```
#[macro_export]
macro_rules! pad_left {
    ($value:expr) => {
        const { $crate::padded_bytes_left($value) }
    };
}
