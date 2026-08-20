//! Typed entry-return values: [`Accept`]/[`Rollback`]/[`HookResult`], and
//! the sealed [`EntryReturn`] conversion the `#[hooks]` macro's generated
//! entry body calls into.
//!
//! Every `#[hook]`/`#[cbak]` entry returns [`HookResult`] — `Ok(Accept)`
//! exits via [`crate::accept`], `Err(Rollback)` via [`crate::rollback`],
//! both through the same generated wrapper,
//! `::rshooks::exit::EntryReturn::finish(<call>)`. [`crate::accept`] and
//! [`crate::rollback`] (the `accept!`/`rollback!` macros) remain public and
//! usable *inside* a typed entry's body — both diverge (`-> !`), so they
//! coerce to `HookResult` at any point in the body, and they stay the
//! escape hatch for computed (non-`'static`) messages or WCE-critical raw
//! bodies. See `docs/DESIGN.md`'s "Typed entry return values" section and
//! `.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md` for the design rationale
//! and the probe numbers behind this shape (§1/§5/§7 D6).

use crate::api::control::{accept, rollback};

/// A successful exit: the message and code handed to the host `accept`
/// call, returned from a typed entry as `Ok(Accept::new(..))`.
///
/// Construct with [`Accept::new`] (an explicit message) or
/// [`Accept::from_code`] (empty message, code only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accept {
    msg: &'static [u8],
    code: i64,
}

impl Accept {
    /// A successful exit with an explicit message and code.
    #[inline(always)]
    #[must_use]
    pub const fn new(msg: &'static [u8], code: i64) -> Self {
        Self { msg, code }
    }

    /// A successful exit with an empty message and the given code.
    #[inline(always)]
    #[must_use]
    pub const fn from_code(code: i64) -> Self {
        Self { msg: b"", code }
    }

    /// The message this exit hands to the host `accept` call.
    #[inline(always)]
    #[must_use]
    pub const fn msg(&self) -> &'static [u8] {
        self.msg
    }

    /// The code this exit hands to the host `accept` call.
    #[inline(always)]
    #[must_use]
    pub const fn code(&self) -> i64 {
        self.code
    }
}

/// A failed exit: the message and code handed to the host `rollback` call,
/// returned from a typed entry as `Err(Rollback::new(..))` or produced by
/// `?` via one of the `From` impls below.
///
/// Construct with [`Rollback::new`] (an explicit message) or
/// [`Rollback::from_code`] (empty message, code only) — or convert into one with
/// `?`: `From<i64>` (empty message, raw code) and every [`hook_errors!`]
/// enum (empty message, unless the enum declares a `=> b"msg"` clause on the
/// failing variant — see [`hook_errors!`]'s doc comment).
///
/// **Deliberately no `From<HookError> for Rollback`.** [`HookError::code`]
/// is a 46-arm re-encode match, and measurement
/// (`.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md` §5, probe P5) showed a
/// `?`-propagated two-hop `HookError` → `Rollback` conversion costs 3.1x the
/// worst-case instruction count and +67% size versus a raw-code-check twin
/// — exactly the class of regression `docs/TODO.md` item 2 flagged as this
/// feature's biggest risk. Convert explicitly at the call site instead,
/// discarding the decoded `HookError` and keeping only "some call failed":
///
/// ```rust,ignore
/// let value = some_hook_api_call().map_err(|_| MyError::SomeCallFailed)?;
/// ```
///
/// — or fall back to [`crate::accept`]/[`crate::rollback`] directly when a
/// computed (non-`'static`) message is needed.
///
/// [`HookError::code`]: crate::error::HookError::code
/// [`hook_errors!`]: crate::hook_errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rollback {
    msg: &'static [u8],
    code: i64,
}

impl Rollback {
    /// A failed exit with an explicit message and code.
    #[inline(always)]
    #[must_use]
    pub const fn new(msg: &'static [u8], code: i64) -> Self {
        Self { msg, code }
    }

    /// A failed exit with an empty message and the given code.
    #[inline(always)]
    #[must_use]
    pub const fn from_code(code: i64) -> Self {
        Self { msg: b"", code }
    }

    /// The message this exit hands to the host `rollback` call.
    #[inline(always)]
    #[must_use]
    pub const fn msg(&self) -> &'static [u8] {
        self.msg
    }

    /// The code this exit hands to the host `rollback` call.
    #[inline(always)]
    #[must_use]
    pub const fn code(&self) -> i64 {
        self.code
    }
}

/// Converts a raw `i64` into a [`Rollback`] with an empty message — a code
/// the caller already holds (`Err(-5i64)?`, or a discriminant /
/// [`hook_errors!`] `code()`). Do not write
/// `some_hook_api_call().map_err(|e| e.code())?`: [`HookError::code`] is a
/// 46-arm re-encode, the 3.1× WCE path this crate forbids. Map the call
/// instead (`.map_err(|_| MyError::SomeCallFailed)?`) as documented on
/// [`Rollback`].
///
/// [`HookError::code`]: crate::error::HookError::code
/// [`hook_errors!`]: crate::hook_errors
impl ::core::convert::From<i64> for Rollback {
    #[inline(always)]
    fn from(code: i64) -> Self {
        Self { msg: b"", code }
    }
}

/// The `Ok`/`Err` pair a typed `#[hook]`/`#[cbak]` entry returns: `Ok(Accept)`
/// exits via the host `accept` call, `Err(Rollback)` via `rollback`. See the
/// module doc comment and [`EntryReturn`] for how the `#[hooks]` macro wires
/// this into the generated wasm export.
pub type HookResult = ::core::result::Result<Accept, Rollback>;

/// Sealing module for [`EntryReturn`] — see that trait's doc comment.
mod private {
    /// Implemented only for the one return shape [`super::EntryReturn`]
    /// accepts: [`super::HookResult`].
    pub trait Sealed {}
}

impl private::Sealed for HookResult {}

/// Converts a `#[hook]`/`#[cbak]` entry's return value into the terminal
/// `i64` the wasm export boundary needs.
///
/// The `#[hooks]` macro's generated entry body calls this unconditionally,
/// wrapping the entry's own call expression:
/// `::rshooks::exit::EntryReturn::finish(<Struct>::<fn>(&<Struct>))`. There
/// is exactly one call site per entry (design §1.3), so the conversion cost
/// is a single 2-arm `match`, never duplicated per `?`.
///
/// **Sealed** — implemented for exactly [`HookResult`]. An entry returning
/// any other type (including `i64`) fails to compile with an ordinary
/// trait-bound diagnostic naming this trait (see
/// `tests/ui/fail/hooks_entry_return_not_entryreturn.rs`, which also pins
/// the `-> i64` migration case).
///
/// `#[doc(hidden)]`: a hook author never names this trait directly — only
/// generated code calls it, at the fully qualified path
/// `::rshooks::exit::EntryReturn::finish`.
#[doc(hidden)]
pub trait EntryReturn: private::Sealed {
    /// Converts `self` into the `i64` the wasm export returns, by calling
    /// [`crate::api::control::accept`]/[`crate::api::control::rollback`],
    /// both of which diverge (`-> !`) on the real wasm host.
    fn finish(self) -> i64;
}

impl EntryReturn for HookResult {
    #[inline(always)]
    fn finish(self) -> i64 {
        match self {
            Ok(a) => accept(a.msg, a.code),
            Err(r) => rollback(r.msg, r.code),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_new_carries_msg_and_code() {
        let a = Accept::new(b"ok", 7);
        assert_eq!(a.msg(), b"ok");
        assert_eq!(a.code(), 7);
    }

    #[test]
    fn accept_from_code_has_empty_msg() {
        let a = Accept::from_code(3);
        assert_eq!(a.msg(), b"");
        assert_eq!(a.code(), 3);
    }

    #[test]
    fn rollback_new_carries_msg_and_code() {
        let r = Rollback::new(b"nope", -1);
        assert_eq!(r.msg(), b"nope");
        assert_eq!(r.code(), -1);
    }

    #[test]
    fn rollback_from_code_has_empty_msg() {
        let r = Rollback::from_code(-2);
        assert_eq!(r.msg(), b"");
        assert_eq!(r.code(), -2);
    }

    #[test]
    fn i64_into_rollback_has_empty_msg_and_raw_code() {
        let r: Rollback = 42i64.into();
        assert_eq!(r.msg(), b"");
        assert_eq!(r.code(), 42);
    }
}
