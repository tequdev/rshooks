//! Persistent hook state: `state`, `state_set`, `state_foreign(_set)`.
//!
//! Two distinct integer conventions coexist in this module, deliberately:
//! - [`state_u64`] (and `state_foreign_u64`) use the host's "as-int64" mode
//!   (`write_ptr = 0, write_len = 0`) — the host packs the entry's raw bytes
//!   **big-endian** into the returned `i64` (see DESIGN.md §5.2). There is
//!   no write-side as-int64 mode ([`crate::error::HookError`]'s module doc
//!   notes `state_set` "carr[ies] no write buffer"), so [`state_update_u64`]
//!   writes back with `to_be_bytes` to round-trip through that same
//!   big-endian convention.
//! - [`state_u32`], [`state_i64`], [`state_xfl`], [`state_u64_le`] (and their
//!   `state_set_*`/`state_update_*` twins, where present) instead read/write
//!   a plain fixed-size buffer via [`state_exact`] and decode/encode it
//!   **little-endian** — the convention the `state-counter` example already
//!   uses by hand (`u64::from_le_bytes`/`to_le_bytes` around a plain
//!   `state`/`state_set` round-trip). These do not use the host's as-int64
//!   mode at all. [`state_u64_le`] is the unsigned 64-bit counterpart to
//!   [`state_u64`]'s as-int64 big-endian read — see its own doc comment for
//!   the side-by-side comparison, and DESIGN.md §5.6 ("Endianness
//!   conventions") for the full two-world rule this split reflects.
//!
//! Every helper above is a standalone function keyed by a raw `&[u8]` — for
//! a typed layer where the key itself is a compile-time-checked enum
//! variant and the value can be any [`crate::convert::ToBytes`]/
//! [`crate::convert::FromBytes`] type (including every `rshooks::types`
//! newtype), see [`mod@crate::state`]'s `state_get`/`state_set_loose`/
//! `state_update_loose` and the [`state_keys!`](crate::state_keys) macro,
//! built on top of this module's [`state`]/[`state_set`]/[`state_exact`].
//!
//! ## Why every `key`/`out`/`data` parameter here is generic, not `&[u8]`/`&mut [u8]`
//!
//! Every key- or buffer-shaped parameter in this module (`state`'s `key`
//! and `out`, `state_set`'s `key` and `data`, `state_foreign`'s `key` and
//! `out`, ...) is generic instead of a bare `&[u8]`/`&mut [u8]`
//! specifically so a [`crate::types::StateKey`] (or any other
//! `rshooks::types` newtype a hook chooses to key or read/write state
//! with) can be passed straight through as `&STATE_KEY`/`&mut raw`, with no
//! `.as_ref()`/`.as_mut()` at the call site. A bare `&[u8]`/`&mut [u8]`
//! parameter can't do this: `StateKey` only implements
//! [`core::ops::Deref`]/[`core::ops::DerefMut`] with `[u8; 32]` as its
//! target (not `[u8]` — see `crate::types`' module doc comment for why),
//! and Rust does not chain that one `Deref` hop with the further built-in
//! array-to-slice unsized coercion at a single call site, so `&STATE_KEY`
//! alone never reaches a `&[u8]` parameter. Bounding the parameter by
//! [`AsRef<[u8]>`](AsRef)/[`AsMut<[u8]>`](AsMut) instead sidesteps the
//! coercion question entirely: every `crate::types` newtype already
//! implements both directly, no coercion needed. This is zero-cost —
//! `#[inline(always)]` plus one generic parameter monomorphized per call
//! site compiles to the exact same code as the old concrete parameter did
//! (verified: `mise run build-examples`'s per-example wasm size and
//! worst-case instruction count are unchanged by this).
//!
//! `namespace`/`account` (`state_foreign`'s optional pair) are a special
//! case, covered by [`ForeignRef`] below rather than a plain `AsRef<[u8]>`/
//! `AsMut<[u8]>` bound — see that trait's doc comment for why.

use crate::convert::FixedRead;
use crate::error::{HookError, Result, res};
use crate::xfl::XFL;

/// Accepts either `None` (absent) or a bare reference to anything
/// implementing [`AsRef<[u8]>`] (present) as `state_foreign`'s
/// `namespace`/`account` arguments.
///
/// This is deliberately a different shape from `state`'s `key` parameter
/// (`&(impl AsRef<[u8]> + ?Sized)`): `namespace`/`account` are *optional*,
/// and a generic `Option<K: AsRef<[u8]>>` parameter cannot also accept a
/// bare `None` literal — with nothing else in the call to pin down `K`,
/// `None` is genuinely ambiguous and fails to compile
/// (`error[E0283]: type annotations needed`, verified directly against
/// rustc, not just reasoned about). `ForeignRef` sidesteps that by giving
/// the "present" and "absent" cases two different shapes instead of one
/// `Option<K>`: a bare `&value` (present, any `AsRef<[u8]>`) or `None`
/// (absent — resolvable because this trait has exactly one
/// `Option<...>`-shaped impl, so `None`'s type isn't ambiguous). The one
/// thing this does **not** support, unlike `state`'s `key` — is
/// `Some(&value)` for a `rshooks::types` newtype: `Option<&AccountId>`
/// doesn't match either impl below (also verified against rustc).
/// Supporting that too would mean adding a generic `Option<&'a T>` impl,
/// which reintroduces exactly the `None`-ambiguity problem this trait
/// exists to avoid. Pass the bare reference instead: `Some(&target)`
/// becomes `&target`, `None` stays `None`. (`Some(raw)` for an
/// already-`&[u8]` value, e.g. one produced by `.as_ref()`, still works —
/// it matches the `Option<&'a [u8]>` impl exactly.)
pub trait ForeignRef<'a> {
    /// `self`, resolved to `Some(bytes)` (present) or `None` (absent).
    fn foreign_ref(self) -> Option<&'a [u8]>;
}

impl<'a> ForeignRef<'a> for Option<&'a [u8]> {
    #[inline(always)]
    fn foreign_ref(self) -> Option<&'a [u8]> {
        self
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> ForeignRef<'a> for &'a T {
    #[inline(always)]
    fn foreign_ref(self) -> Option<&'a [u8]> {
        Some(self.as_ref())
    }
}

/// Read this hook's own state, decoded as an optional-defaulting `(ptr, len)`
/// pair — `None` becomes `(0, 0)`. Only ever used in the read direction,
/// never mixed with a write-direction call, so it does not blur the
/// pointer-direction discipline used elsewhere in this crate.
#[inline(always)]
fn opt_in(o: Option<&[u8]>) -> (u32, u32) {
    match o {
        Some(s) => (s.as_ptr() as u32, s.len() as u32),
        None => (0, 0),
    }
}

/// Read this hook's own state entry for `key` into `out`. Returns the number
/// of bytes written.
///
/// # Examples
///
/// ```
/// use rshooks::api::state::state;
/// use rshooks::error::HookError;
///
/// let mut out = [0u8; 32];
/// let key = [0u8; 32];
/// assert_eq!(state(&mut out, &key), Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn state<B: AsMut<[u8]> + ?Sized, K: AsRef<[u8]> + ?Sized>(
    out: &mut B,
    key: &K,
) -> Result<usize> {
    let out = out.as_mut();
    let key = key.as_ref();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.state(key)) {
        return crate::testenv_bridge::write_bytes(out, r);
    }
    res(unsafe {
        rshooks_core::state(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            key.as_ptr() as u32,
            key.len() as u32,
        )
    })
    .map(|v| v as usize)
}

/// Calls the host `state` function directly and returns its **undecoded**
/// `i64` result — no [`res`] applied, so no [`HookError`] is ever
/// constructed here. `#[inline(always)]` and `pub(crate)`: an internal
/// fast path for callers that need to compare the raw code against a
/// specific constant (e.g. [`rshooks_core::DOESNT_EXIST`]) *before* deciding
/// whether to decode at all — see DESIGN.md §5.1's "no specific-variant
/// decode inside rshooks" principle.
///
/// Deliberately **not** implemented by having [`state`] call this (or vice
/// versa): measured directly, routing [`state`] *through* a second,
/// separately-defined function (even with both `#[inline(always)]`)
/// changed `rshooks-build`'s unnest pass's output for an unrelated hook
/// (`examples/81_govern`, which never touches this function's raw-code
/// path) enough to push its nesting depth from 22 to 55 — the pass's
/// heuristics operate on the compiled wasm's actual block shape, not a
/// purely semantics-preserving view of the source. Duplicating this one
/// small function body instead keeps [`state`]'s own compiled output
/// provably unaffected.
#[inline(always)]
pub(crate) fn state_raw_code<B: AsMut<[u8]> + ?Sized, K: AsRef<[u8]> + ?Sized>(
    out: &mut B,
    key: &K,
) -> i64 {
    let out = out.as_mut();
    let key = key.as_ref();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.state(key)) {
        return crate::testenv_bridge::write_bytes_code(out, r);
    }
    unsafe {
        rshooks_core::state(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            key.as_ptr() as u32,
            key.len() as u32,
        )
    }
}

/// Read this hook's own state entry for `key` as a big-endian `u64`
/// ("as-int64" mode: the host is passed `write_ptr = 0, write_len = 0` and
/// returns the entry's bytes packed big-endian instead of a length).
///
/// Only valid for entries of at most 8 bytes whose top bit is clear —
/// anything else fails with [`crate::error::HookError::TooBig`] (xahaud's
/// `data_as_int64`, `applyHook.cpp`). Note the interpretation is
/// **big-endian**: an 8-byte little-endian counter read this way comes back
/// byte-swapped.
///
/// **Intended use**: this reads an entry whose bytes originated from Xahau
/// Binary itself — e.g. a value mirroring a protocol field like
/// `Tx.Sequence`, or interop with a C hook that wrote the entry with
/// explicit big-endian bytes to match protocol convention (see DESIGN.md
/// §5.6, "Endianness conventions", for the full two-world rule). It is
/// **not** the right function for reading a value this crate's own typed
/// storage layer wrote — `crate::state`'s `state_get`/`state_set_loose` and
/// every `rshooks::types`/`#[derive(HookData)]` value are little-endian
/// by convention, and reading one of those through this big-endian as-int64
/// path silently byte-swaps it (both calls succeed; only the returned value
/// is wrong). Reach for [`state_u64_le`] instead when the entry was written
/// by this crate's typed/LE layer.
#[inline(always)]
pub fn state_u64<K: AsRef<[u8]> + ?Sized>(key: &K) -> Result<u64> {
    let key = key.as_ref();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.state(key)) {
        return res(crate::testenv_bridge::as_int64_code(r)).map(|v| v as u64);
    }
    res(unsafe { rshooks_core::state(0, 0, key.as_ptr() as u32, key.len() as u32) })
        .map(|v| v as u64)
}

/// As-int64-mode counterpart to [`state_raw_code`]: calls the host `state`
/// function with `write_ptr = 0, write_len = 0` and returns its
/// **undecoded** `i64` result. See [`state_raw_code`]'s doc comment for
/// why this exists (an internal fast path for raw-code comparisons before
/// ever constructing a [`HookError`]) and for why it deliberately
/// duplicates [`state_u64`]'s own call instead of [`state_u64`] routing
/// through it — [`state_u64`] itself is unaffected either way.
#[inline(always)]
pub(crate) fn state_u64_raw_code<K: AsRef<[u8]> + ?Sized>(key: &K) -> i64 {
    let key = key.as_ref();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.state(key)) {
        return crate::testenv_bridge::as_int64_code(r);
    }
    unsafe { rshooks_core::state(0, 0, key.as_ptr() as u32, key.len() as u32) }
}

/// Read this hook's own state entry for `key` as a plain little-endian
/// `u64`, via the ordinary (non-as-int64) buffer path — the little-endian
/// counterpart to [`state_u64`]'s big-endian as-int64 mode (see DESIGN.md
/// §5.6, "Endianness conventions").
///
/// | | [`state_u64`] | `state_u64_le` |
/// |---|---|---|
/// | Endianness | big-endian | little-endian |
/// | Host mechanism | as-int64 (`write_ptr=0, write_len=0`) | ordinary buffer read ([`state_exact`]) |
/// | Reads an entry written by | Xahau Binary / a BE-writing C hook | this crate's typed layer (`ToBytes`/`FromBytes`, `state_set_loose`/`state_set_typed`) or hand-written `to_le_bytes` |
/// | Size constraint | ≤8 bytes, top bit clear, else [`crate::error::HookError::TooBig`] | must be exactly 8 bytes, else [`crate::error::HookError::TooSmall`] |
///
/// # Examples
///
/// ```
/// use rshooks::api::state::state_u64_le;
/// use rshooks::error::HookError;
///
/// let key = [0u8; 32];
/// assert_eq!(state_u64_le(&key), Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn state_u64_le<K: AsRef<[u8]> + ?Sized>(key: &K) -> Result<u64> {
    state_exact::<[u8; 8]>(key.as_ref()).map(u64::from_le_bytes)
}

/// Read this hook's own state entry for `key`, requiring it to be exactly
/// `T`'s length — any [`crate::convert::FixedRead`] type, most commonly a
/// `rshooks::types` newtype or a raw `[u8; N]`.
///
/// An entry longer than that already fails as
/// [`HookError::TooSmall`](crate::error::HookError::TooSmall) from the
/// underlying host call (the buffer `T::read_exact` allocates has exactly
/// that capacity); an entry shorter is caught by `T::read_exact` itself
/// (the host call succeeds but writes fewer bytes) and mapped to the same
/// [`HookError::TooSmall`] variant, since both cases are "the entry does
/// not have the exact expected size". No loop, no panic.
///
/// `T` is inferred from context, not a turbofish — see
/// [`crate::api::otxn::otxn_field_exact`]'s doc comment for the full
/// story.
///
/// # Examples
///
/// ```
/// use rshooks::api::state::state_exact;
/// use rshooks::error::{HookError, Result};
///
/// let key = [0u8; 32];
/// let value: Result<[u8; 8]> = state_exact(&key);
/// assert_eq!(value, Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn state_exact<T: FixedRead>(key: &[u8]) -> Result<T> {
    T::read_exact(|buf| state(buf, key))
}

/// Read this hook's own state entry for `key` as a little-endian `u32` (via
/// [`state_exact`] — see the module doc comment for how this differs from
/// [`state_u64`]'s host-side as-int64 mode).
///
/// # Examples
///
/// ```
/// use rshooks::api::state::state_u32;
/// use rshooks::error::HookError;
///
/// let key = [0u8; 32];
/// assert_eq!(state_u32(&key), Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn state_u32<K: AsRef<[u8]> + ?Sized>(key: &K) -> Result<u32> {
    state_exact::<[u8; 4]>(key.as_ref()).map(u32::from_le_bytes)
}

/// Write this hook's own state entry for `key` as a little-endian `u32`.
/// Returns the number of bytes written.
#[inline(always)]
pub fn state_set_u32<K: AsRef<[u8]> + ?Sized>(value: u32, key: &K) -> Result<usize> {
    state_set(&value.to_le_bytes(), key)
}

/// Read this hook's own state entry for `key` as a little-endian `i64` (via
/// [`state_exact`]; see the module doc comment).
#[inline(always)]
pub fn state_i64<K: AsRef<[u8]> + ?Sized>(key: &K) -> Result<i64> {
    state_exact::<[u8; 8]>(key.as_ref()).map(i64::from_le_bytes)
}

/// Write this hook's own state entry for `key` as a little-endian `i64`.
/// Returns the number of bytes written.
#[inline(always)]
pub fn state_set_i64<K: AsRef<[u8]> + ?Sized>(value: i64, key: &K) -> Result<usize> {
    state_set(&value.to_le_bytes(), key)
}

/// Read this hook's own state entry for `key` as an [`XFL`], stored as its
/// raw bit pattern (`i64`) in little-endian bytes (via [`state_exact`]; see
/// the module doc comment).
#[inline(always)]
pub fn state_xfl<K: AsRef<[u8]> + ?Sized>(key: &K) -> Result<XFL> {
    state_exact::<[u8; 8]>(key.as_ref())
        .map(i64::from_le_bytes)
        .map(XFL::from_raw_bits)
}

/// Write this hook's own state entry for `key` as an [`XFL`]'s raw bit
/// pattern (`i64`), little-endian. Returns the number of bytes written.
#[inline(always)]
pub fn state_set_xfl<K: AsRef<[u8]> + ?Sized>(value: XFL, key: &K) -> Result<usize> {
    state_set(&value.raw_bits().to_le_bytes(), key)
}

/// Collapses a **raw, undecoded** host-call `i64` result (`code`) into
/// "value present" (`Ok(Some(v))`, via `decode`), "no entry yet"
/// (`Ok(None)`, from comparing `code` directly against
/// [`rshooks_core::DOESNT_EXIST`] — see [`state_raw_code`]'s doc comment for
/// why this must happen *before* any [`HookError`] is constructed), or "a
/// real error" (`Err(e)`, from `decode`'s own `res(code)` call on that rare
/// path — every caller here only ever propagates `e` onward via `?`,
/// never matching a further specific variant, so [`HookError::from`]'s
/// decode still optimizes away there too, per the same principle). Pure
/// function, no host call — kept separate from the `state_update_*`
/// functions so it has a standalone unit test.
#[inline(always)]
fn value_or_absent<T>(code: i64, decode: impl FnOnce(i64) -> Result<T>) -> Result<Option<T>> {
    if code == rshooks_core::DOESNT_EXIST {
        return Ok(None);
    }
    decode(code).map(Some)
}

/// Reads this hook's own state entry for `key` into a fixed `N`-byte
/// buffer via the raw host call, returning the **undecoded** `i64` result
/// alongside the buffer — the buffer-mode counterpart to
/// [`state_u64_raw_code`], backing [`state_update_u32`]/
/// [`state_update_i64`]/[`state_update_xfl`]. See [`state_raw_code`]'s doc
/// comment for why this exists; the exact-length check
/// [`state_exact`]/[`FixedRead::read_exact`] normally performs is each
/// caller's own responsibility here (via [`value_or_absent`]'s `decode`
/// closure), since this function only makes the raw code available before
/// any [`HookError`] is constructed.
///
/// Returns the buffer as [`core::mem::MaybeUninit`], not `[u8; N]` (see
/// `crate::convert`'s module doc comment): whether `state_raw_code` wrote
/// all `N` bytes is exactly the check each caller's `decode` closure
/// performs (`written == N`) before ever turning the buffer into a concrete
/// `[u8; N]` via `assume_init` — this function itself has no way to know
/// that check has passed, so it cannot honestly return an
/// already-initialized `[u8; N]` value.
#[inline(always)]
fn state_raw_code_buf<const N: usize, K: AsRef<[u8]> + ?Sized>(
    key: &K,
) -> (i64, core::mem::MaybeUninit<[u8; N]>) {
    let mut buf = core::mem::MaybeUninit::<[u8; N]>::uninit();
    // SAFETY: see `crate::convert::uninit_slice_mut`'s doc comment. The
    // returned `MaybeUninit` is only ever turned into a concrete `[u8; N]`
    // by a caller that has independently confirmed (via the raw `code`
    // returned alongside it) that `state_raw_code` wrote exactly `N` bytes
    // before calling `assume_init` — see each `state_update_*` call site.
    let code = state_raw_code(unsafe { crate::convert::uninit_slice_mut(&mut buf) }, key);
    (code, buf)
}

/// Read-modify-write this hook's own state entry for `key` as a `u64`
/// (host as-int64 big-endian convention, matching [`state_u64`]): `f` is
/// called with `None` if the key does not yet exist, or `Some` of the
/// current value otherwise; its return value is written back and also
/// returned as `Ok`. A real read error (anything but "doesn't exist")
/// short-circuits before `f` is called or anything is written.
#[inline(always)]
pub fn state_update_u64<K: AsRef<[u8]> + ?Sized>(
    key: &K,
    f: impl FnOnce(Option<u64>) -> u64,
) -> Result<u64> {
    let code = state_u64_raw_code(key);
    let current = value_or_absent(code, |c| res(c).map(|v| v as u64))?;
    let next = f(current);
    let _ = state_set(&next.to_be_bytes(), key)?;
    Ok(next)
}

/// Read-modify-write this hook's own state entry for `key` as a `u32`
/// (little-endian convention, matching [`state_u32`]). See
/// [`state_update_u64`] for the `Option`/error-propagation semantics.
#[inline(always)]
pub fn state_update_u32<K: AsRef<[u8]> + ?Sized>(
    key: &K,
    f: impl FnOnce(Option<u32>) -> u32,
) -> Result<u32> {
    let (code, buf) = state_raw_code_buf::<4, _>(key);
    let current = value_or_absent(code, |c| {
        let written = res(c)? as usize;
        if written == 4 {
            // SAFETY: `written == 4` (this buffer's own width) means
            // `state_raw_code` wrote every byte of `buf`.
            Ok(u32::from_le_bytes(unsafe { buf.assume_init() }))
        } else {
            Err(HookError::TooSmall)
        }
    })?;
    let next = f(current);
    let _ = state_set_u32(next, key)?;
    Ok(next)
}

/// Read-modify-write this hook's own state entry for `key` as an `i64`
/// (little-endian convention, matching [`state_i64`]). See
/// [`state_update_u64`] for the `Option`/error-propagation semantics.
#[inline(always)]
pub fn state_update_i64<K: AsRef<[u8]> + ?Sized>(
    key: &K,
    f: impl FnOnce(Option<i64>) -> i64,
) -> Result<i64> {
    let (code, buf) = state_raw_code_buf::<8, _>(key);
    let current = value_or_absent(code, |c| {
        let written = res(c)? as usize;
        if written == 8 {
            // SAFETY: `written == 8` (this buffer's own width) means
            // `state_raw_code` wrote every byte of `buf`.
            Ok(i64::from_le_bytes(unsafe { buf.assume_init() }))
        } else {
            Err(HookError::TooSmall)
        }
    })?;
    let next = f(current);
    let _ = state_set_i64(next, key)?;
    Ok(next)
}

/// Read-modify-write this hook's own state entry for `key` as an [`XFL`]
/// (little-endian raw-bits convention, matching [`state_xfl`]). See
/// [`state_update_u64`] for the `Option`/error-propagation semantics.
#[inline(always)]
pub fn state_update_xfl<K: AsRef<[u8]> + ?Sized>(
    key: &K,
    f: impl FnOnce(Option<XFL>) -> XFL,
) -> Result<XFL> {
    let (code, buf) = state_raw_code_buf::<8, _>(key);
    let current = value_or_absent(code, |c| {
        let written = res(c)? as usize;
        if written == 8 {
            // SAFETY: `written == 8` (this buffer's own width) means
            // `state_raw_code` wrote every byte of `buf`.
            Ok(XFL::from_raw_bits(i64::from_le_bytes(unsafe {
                buf.assume_init()
            })))
        } else {
            Err(HookError::TooSmall)
        }
    })?;
    let next = f(current);
    let _ = state_set_xfl(next, key)?;
    Ok(next)
}

/// Write this hook's own state entry for `key`. Returns the number of bytes
/// written.
#[inline(always)]
pub fn state_set<K: AsRef<[u8]> + ?Sized>(data: &[u8], key: &K) -> Result<usize> {
    let key = key.as_ref();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.state_set(key, data)) {
        let code = match r {
            Ok(c) | Err(c) => c,
        };
        return res(code).map(|v| v as usize);
    }
    res(unsafe {
        rshooks_core::state_set(
            data.as_ptr() as u32,
            data.len() as u32,
            key.as_ptr() as u32,
            key.len() as u32,
        )
    })
    .map(|v| v as usize)
}

/// Read a state entry belonging to another account/namespace. `namespace`
/// and `account` default to "this hook's own" (`None` → the documented
/// zero-length Hook API sentinel) when omitted. Returns the number of bytes
/// written to `out`.
#[inline(always)]
pub fn state_foreign<'ns, 'ac, B, K, N, A>(
    out: &mut B,
    key: &K,
    namespace: N,
    account: A,
) -> Result<usize>
where
    B: AsMut<[u8]> + ?Sized,
    K: AsRef<[u8]> + ?Sized,
    N: ForeignRef<'ns>,
    A: ForeignRef<'ac>,
{
    let out = out.as_mut();
    let key = key.as_ref();
    // `ForeignRef::foreign_ref` takes `self` by value, and the testenv
    // block below needs the same resolved bytes `opt_in` consumes next —
    // extracted once here (identical value, identical evaluation point) so
    // both can use it. See `crates/rshooks/testenv-call-sites.txt`'s notes
    // for why this extraction, not a restructure, is necessary here.
    let ns_bytes = namespace.foreign_ref();
    let acc_bytes = account.foreign_ref();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| {
        let (ns, acc) = crate::testenv_bridge::foreign_target(ns_bytes, acc_bytes)?;
        b.state_foreign(key, ns, acc)
    }) {
        return crate::testenv_bridge::write_bytes(out, r);
    }
    let (nptr, nlen) = opt_in(ns_bytes);
    let (aptr, alen) = opt_in(acc_bytes);
    res(unsafe {
        rshooks_core::state_foreign(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            key.as_ptr() as u32,
            key.len() as u32,
            nptr,
            nlen,
            aptr,
            alen,
        )
    })
    .map(|v| v as usize)
}

/// `state_foreign` counterpart to [`state_raw_code`]: calls the host
/// `state_foreign` function directly and returns its **undecoded** `i64`
/// result. See [`state_raw_code`]'s doc comment for why this exists (and
/// for why it deliberately duplicates [`state_foreign`]'s own call instead
/// of [`state_foreign`] routing through it) — [`state_foreign`] itself is
/// unaffected either way.
#[inline(always)]
pub(crate) fn state_foreign_raw_code<'ns, 'ac, B, K, N, A>(
    out: &mut B,
    key: &K,
    namespace: N,
    account: A,
) -> i64
where
    B: AsMut<[u8]> + ?Sized,
    K: AsRef<[u8]> + ?Sized,
    N: ForeignRef<'ns>,
    A: ForeignRef<'ac>,
{
    let out = out.as_mut();
    let key = key.as_ref();
    // See `state_foreign`'s matching comment: `ns_bytes`/`acc_bytes` are
    // needed by both the testenv block below and the existing `opt_in`
    // calls, so they're extracted once here.
    let ns_bytes = namespace.foreign_ref();
    let acc_bytes = account.foreign_ref();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| {
        let (ns, acc) = crate::testenv_bridge::foreign_target(ns_bytes, acc_bytes)?;
        b.state_foreign(key, ns, acc)
    }) {
        return crate::testenv_bridge::write_bytes_code(out, r);
    }
    let (nptr, nlen) = opt_in(ns_bytes);
    let (aptr, alen) = opt_in(acc_bytes);
    unsafe {
        rshooks_core::state_foreign(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            key.as_ptr() as u32,
            key.len() as u32,
            nptr,
            nlen,
            aptr,
            alen,
        )
    }
}

/// Read a foreign state entry as a big-endian `u64` ("as-int64" mode; see
/// [`state_u64`] for the size/top-bit rules and endianness caveat).
/// `namespace`/`account` follow [`state_foreign`]'s `Option` convention.
///
/// **Intended use**: like [`state_u64`], this is for an entry whose bytes
/// originated from Xahau Binary itself, not one written by this crate's own
/// little-endian typed layer (see DESIGN.md §5.6) — reach for
/// [`state_foreign_u64_le`] for the latter.
#[inline(always)]
pub fn state_foreign_u64<'ns, 'ac, K, N, A>(key: &K, namespace: N, account: A) -> Result<u64>
where
    K: AsRef<[u8]> + ?Sized,
    N: ForeignRef<'ns>,
    A: ForeignRef<'ac>,
{
    let key = key.as_ref();
    // See `state_foreign`'s matching comment.
    let ns_bytes = namespace.foreign_ref();
    let acc_bytes = account.foreign_ref();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| {
        let (ns, acc) = crate::testenv_bridge::foreign_target(ns_bytes, acc_bytes)?;
        b.state_foreign(key, ns, acc)
    }) {
        return res(crate::testenv_bridge::as_int64_code(r)).map(|v| v as u64);
    }
    let (nptr, nlen) = opt_in(ns_bytes);
    let (aptr, alen) = opt_in(acc_bytes);
    res(unsafe {
        rshooks_core::state_foreign(
            0,
            0,
            key.as_ptr() as u32,
            key.len() as u32,
            nptr,
            nlen,
            aptr,
            alen,
        )
    })
    .map(|v| v as u64)
}

/// Read a foreign state entry as a plain little-endian `u64`, via the
/// ordinary (non-as-int64) buffer path — the little-endian counterpart to
/// [`state_foreign_u64`]'s big-endian as-int64 mode (see [`state_u64_le`]
/// and DESIGN.md §5.6, "Endianness conventions", for the full comparison).
/// `namespace`/`account` follow [`state_foreign`]'s `Option` convention.
#[inline(always)]
pub fn state_foreign_u64_le<'ns, 'ac, K, N, A>(key: &K, namespace: N, account: A) -> Result<u64>
where
    K: AsRef<[u8]> + ?Sized,
    N: ForeignRef<'ns>,
    A: ForeignRef<'ac>,
{
    // Deliberately still `[0u8; 8]`, not `MaybeUninit`: unlike `state_u64_le`
    // (which goes through `state_exact`'s `written == N` check), this
    // function decodes `raw` on any `Ok` from `state_foreign` without first
    // confirming exactly 8 bytes were written — a foreign entry shorter
    // than 8 bytes reads back its zero-padded tail today rather than
    // failing as `TooSmall`. That's an existing behavior this optimization
    // pass leaves alone; switching to `MaybeUninit` here without also
    // adding the length check would turn "silently zero-padded" into a
    // genuine soundness hole (reading whatever uninitialized bytes happen
    // to follow `raw` on the stack).
    let mut raw = [0u8; 8];
    let _ = state_foreign(&mut raw, key, namespace, account)?;
    Ok(u64::from_le_bytes(raw))
}

/// Write a state entry belonging to another namespace/account (a foreign
/// namespace on this hook's own account, or another account's namespace
/// depending on protocol rules). See [`state_foreign`] for the `Option`
/// convention. Returns the number of bytes written.
#[inline(always)]
pub fn state_foreign_set<'ns, 'ac, K, N, A>(
    data: &[u8],
    key: &K,
    namespace: N,
    account: A,
) -> Result<usize>
where
    K: AsRef<[u8]> + ?Sized,
    N: ForeignRef<'ns>,
    A: ForeignRef<'ac>,
{
    let key = key.as_ref();
    // See `state_foreign`'s matching comment.
    let ns_bytes = namespace.foreign_ref();
    let acc_bytes = account.foreign_ref();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| {
        let (ns, acc) = crate::testenv_bridge::foreign_target(ns_bytes, acc_bytes)?;
        b.state_foreign_set(key, data, ns, acc)
    }) {
        let code = match r {
            Ok(c) | Err(c) => c,
        };
        return res(code).map(|v| v as usize);
    }
    let (nptr, nlen) = opt_in(ns_bytes);
    let (aptr, alen) = opt_in(acc_bytes);
    res(unsafe {
        rshooks_core::state_foreign_set(
            data.as_ptr() as u32,
            data.len() as u32,
            key.as_ptr() as u32,
            key.len() as u32,
            nptr,
            nlen,
            aptr,
            alen,
        )
    })
    .map(|v| v as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;

    #[test]
    fn smoke_not_implemented_on_host() {
        let mut out = [0u8; 32];
        let key = [0u8; 32];
        assert_eq!(state(&mut out, &key), Err(HookError::NotImplemented));
        assert_eq!(state_u64(&key), Err(HookError::NotImplemented));
        assert_eq!(
            state_foreign_u64(&key, None, None),
            Err(HookError::NotImplemented)
        );
        assert_eq!(state_u64_le(&key), Err(HookError::NotImplemented));
        assert_eq!(
            state_foreign_u64_le(&key, None, None),
            Err(HookError::NotImplemented)
        );
        assert_eq!(state_set(&out, &key), Err(HookError::NotImplemented));
        assert_eq!(
            state_foreign(&mut out, &key, None, None),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_foreign_set(&out, &key, &key, &key),
            Err(HookError::NotImplemented)
        );
        assert_eq!(state_exact::<[u8; 8]>(&key), Err(HookError::NotImplemented));
        assert_eq!(state_u32(&key), Err(HookError::NotImplemented));
        assert_eq!(state_set_u32(1, &key), Err(HookError::NotImplemented));
        assert_eq!(state_i64(&key), Err(HookError::NotImplemented));
        assert_eq!(state_set_i64(1, &key), Err(HookError::NotImplemented));
        assert!(matches!(state_xfl(&key), Err(HookError::NotImplemented)));
        assert_eq!(
            state_set_xfl(XFL::one(), &key),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_update_u64(&key, |cur| cur.unwrap_or(0) + 1),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_update_u32(&key, |cur| cur.unwrap_or(0) + 1),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_update_i64(&key, |cur| cur.unwrap_or(0) + 1),
            Err(HookError::NotImplemented)
        );
        assert!(matches!(
            state_update_xfl(&key, |cur| cur.unwrap_or(XFL::one())),
            Err(HookError::NotImplemented)
        ));
    }

    #[test]
    fn state_accepts_newtype_key_and_out_buffer_by_reference() {
        // `&STATE_KEY`/`&mut buf` where both are `rshooks::types` newtypes
        // — no `.as_ref()`/`.as_mut()` needed — is the whole point of the
        // generic `K`/`B` bounds on `state`/`state_foreign` (see the module
        // doc comment). This test locks that call shape in.
        use crate::types::{AccountId, StateKey};

        let key = StateKey::default();
        let mut out = AccountId::default();
        assert_eq!(state(&mut out, &key), Err(HookError::NotImplemented));
        assert_eq!(
            state_foreign(&mut out, &key, None, None),
            Err(HookError::NotImplemented)
        );
    }

    #[test]
    fn state_foreign_accepts_bare_newtype_reference_for_namespace_and_account() {
        // The `ForeignRef` half of the same story: `&target`/`&namespace`
        // (bare, no `Some(..)`) for a `rshooks::types` newtype, `None` for
        // absent — see `ForeignRef`'s doc comment for why `Some(&target)`
        // itself isn't supported.
        use crate::types::{AccountId, NameSpace};

        let key = [0u8; 32];
        let mut out = [0u8; 32];
        let target = AccountId::default();
        let ns = NameSpace::default();
        assert_eq!(
            state_foreign(&mut out, &key, &ns, &target),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_foreign(&mut out, &key, None, &target),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            state_foreign_u64(&key, &ns, None),
            Err(HookError::NotImplemented)
        );
    }

    #[test]
    fn foreign_ref_resolves_present_and_absent() {
        // Pure-logic check on `ForeignRef` itself, no host call involved.
        let bytes = [1u8, 2, 3];
        let some: Option<&[u8]> = Some(&bytes);
        let none: Option<&[u8]> = None;
        assert_eq!(some.foreign_ref(), Some(bytes.as_slice()));
        assert_eq!(none.foreign_ref(), None);
        assert_eq!((&bytes).foreign_ref(), Some(bytes.as_slice()));
    }

    // `state_exact`'s length comparison lives in `T::read_exact` (the
    // `FixedRead` impl, one per concrete `T`), exercised standalone by
    // `convert.rs`'s `fixed_read_array_rejects_short_write`/
    // `fixed_read_succeeds_on_exact_write` (and `types.rs`'s newtype-
    // flavored equivalents). `state_exact` itself always goes through the
    // host stub on this target (returning `NotImplemented` before any
    // length check happens), so there is nothing left for a standalone,
    // non-host-call test to exercise here.

    #[test]
    fn value_or_absent_distinguishes_doesnt_exist_from_real_errors() {
        // `decode`'s own body always runs through `res` (never inspects a
        // specific `HookError` variant) — mirroring the real
        // `state_update_*` callers exactly.
        let decode = |code: i64| res(code).map(|v| v as u64);
        assert_eq!(value_or_absent(7, decode), Ok(Some(7u64)));
        assert_eq!(
            value_or_absent(rshooks_core::DOESNT_EXIST, decode),
            Ok(None)
        );
        assert_eq!(
            value_or_absent(rshooks_core::NOT_IMPLEMENTED, decode),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            value_or_absent(rshooks_core::TOO_BIG, decode),
            Err(HookError::TooBig)
        );
    }

    #[test]
    fn state_u64_le_agrees_with_the_typed_le_layer() {
        // No host call involved: proves `state_u64_le`'s `u64::from_le_bytes`
        // decodes the exact same bytes the typed `ToBytes`/`FromBytes` layer
        // (crate::convert, little-endian by convention — see DESIGN.md §5.6)
        // produces/expects for a `u64`, so the two are interchangeable on the
        // wire for a value this crate's own typed layer wrote.
        use crate::convert::{FromBytes, ToBytes};

        let value: u64 = 0x0102_0304_0506_0708;
        let mut buf = [0u8; 8];
        assert_eq!(value.write(&mut buf), 8);

        // `state_u64_le`'s decode step, applied directly to the typed
        // layer's own encoding.
        assert_eq!(u64::from_le_bytes(buf), value);
        // And the typed layer decodes its own encoding back too.
        assert_eq!(u64::read(&buf), Ok(value));
    }
}

/// Proves the `testenv` interception blocks in this file actually reach an
/// installed [`rshooks_core::backend::HostBackend`] and honor the host
/// buffer contract (design §5.2: `TOO_SMALL` on an undersized destination,
/// never a truncated copy).
#[cfg(all(test, feature = "testenv"))]
mod testenv_tests {
    #![allow(clippy::unwrap_used, clippy::panic)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    extern crate std;

    use std::rc::Rc;
    use std::vec::Vec;

    use super::*;
    use crate::error::HookError;
    use rshooks_core::backend::{HostBackend, install};

    /// Answers every read with a fixed byte string; `accept`/`rollback`
    /// are unused by these tests and simply panic if ever reached.
    struct FixedBytesBackend(&'static [u8]);

    impl HostBackend for FixedBytesBackend {
        fn state(&self, _key: &[u8]) -> core::result::Result<Vec<u8>, i64> {
            Ok(self.0.to_vec())
        }

        fn state_foreign(
            &self,
            _key: &[u8],
            _ns: Option<&[u8; 32]>,
            _acc: Option<&[u8; 20]>,
        ) -> core::result::Result<Vec<u8>, i64> {
            Ok(self.0.to_vec())
        }

        fn accept(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("FixedBytesBackend::accept unexpectedly called")
        }

        fn rollback(&self, _msg: &[u8], _code: i64) -> ! {
            panic!("FixedBytesBackend::rollback unexpectedly called")
        }
    }

    #[test]
    fn state_reads_backend_bytes_through_the_public_wrapper() {
        let _guard = install(Rc::new(FixedBytesBackend(&[0xAA, 0xBB, 0xCC, 0xDD])));
        let mut out = [0u8; 8];
        let key = [0u8; 32];
        let n = state(&mut out, &key).unwrap();
        assert_eq!(n, 4);
        assert_eq!(&out[..4], &[0xAA, 0xBB, 0xCC, 0xDD]);
    }

    #[test]
    fn state_undersized_destination_yields_too_small_never_truncates() {
        let _guard = install(Rc::new(FixedBytesBackend(&[1, 2, 3, 4, 5, 6, 7, 8, 9])));
        let mut out = [0u8; 4]; // shorter than the backend's 9-byte value
        let key = [0u8; 32];
        assert_eq!(state(&mut out, &key), Err(HookError::TooSmall));
        // Never partially written on the `TooSmall` path.
        assert_eq!(out, [0u8; 4]);
    }

    #[test]
    fn state_u64_round_trips_through_the_backend_byte_path() {
        // 4 bytes, big-endian, matching the as-int64 convention `state_u64`
        // documents.
        let _guard = install(Rc::new(FixedBytesBackend(&[0x01, 0x02, 0x03, 0x04])));
        let key = [0u8; 32];
        assert_eq!(state_u64(&key), Ok(0x0102_0304));
    }

    #[test]
    fn state_foreign_rejects_wrong_length_targets_like_the_host() {
        // The host validates `nread_len` (0 or 32) and `aread_len` (0 or 20)
        // before touching state; the testenv path must mirror that instead
        // of silently treating a malformed target as "this hook's own".
        let _guard = install(Rc::new(FixedBytesBackend(&[0xEE])));
        let mut out = [0u8; 8];
        let key = [0u8; 32];
        let short_ns: &[u8] = &[0u8; 16];
        let short_acc: &[u8] = &[0u8; 4];
        assert_eq!(
            state_foreign(&mut out, &key, short_ns, None),
            Err(HookError::InvalidArgument)
        );
        assert_eq!(
            state_foreign(&mut out, &key, None, short_acc),
            Err(HookError::InvalidAccount)
        );
        // Well-formed lengths still reach the backend.
        let ns = [0u8; 32];
        let acc = [0u8; 20];
        assert_eq!(state_foreign(&mut out, &key, &ns, &acc), Ok(1));
    }
}
