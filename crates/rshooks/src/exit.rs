//! Typed entry-return values: [`Accept`]/[`Rollback`]/[`HookResult`], and
//! the sealed [`EntryReturn`] conversion the `#[hooks]` macro's generated
//! entry body calls into, plus [`EmitOutcome`] for a `#[cbak(<index>)]`
//! entry's optional callback-outcome argument.
//!
//! Every `#[hook]`/`#[cbak]` entry returns [`HookResult`] — `Ok(Accept)`
//! exits via [`crate::accept`], `Err(Rollback)` via [`crate::rollback`],
//! both through the same generated wrapper,
//! `::rshooks::exit::EntryReturn::finish(<call>)`. [`crate::accept`] and
//! [`crate::rollback`] (the `accept!`/`rollback!` macros) remain public and
//! usable *inside* a typed entry's body — both diverge (`-> !`), so they
//! coerce to `HookResult` at any point in the body, and they stay the
//! escape hatch for computed (non-`'static`) messages or WCE-critical raw
//! bodies.

use crate::api::control::{accept, rollback};

/// Declares one exit-value struct (`msg`/`code`, both `'static`/`Copy`) with
/// its `new`/`from_code`/`msg`/`code` constructors — [`Accept`] and
/// [`Rollback`] share this exact shape, differing only in rustdoc and which
/// host call ultimately consumes the pair.
macro_rules! exit_kind {
    (
        $(#[$doc:meta])*
        struct $name:ident {
            $(#[$new_doc:meta])*
            new;
            $(#[$from_code_doc:meta])*
            from_code;
            $(#[$msg_doc:meta])*
            msg;
            $(#[$code_doc:meta])*
            code;
        }
    ) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $name {
            msg: &'static [u8],
            code: i64,
        }

        impl $name {
            $(#[$new_doc])*
            #[inline(always)]
            #[must_use]
            pub const fn new(msg: &'static [u8], code: i64) -> Self {
                Self { msg, code }
            }

            $(#[$from_code_doc])*
            #[inline(always)]
            #[must_use]
            pub const fn from_code(code: i64) -> Self {
                Self { msg: b"", code }
            }

            $(#[$msg_doc])*
            #[inline(always)]
            #[must_use]
            pub const fn msg(&self) -> &'static [u8] {
                self.msg
            }

            $(#[$code_doc])*
            #[inline(always)]
            #[must_use]
            pub const fn code(&self) -> i64 {
                self.code
            }
        }
    };
}

exit_kind! {
    /// A successful exit: the message and code handed to the host `accept`
    /// call, returned from a typed entry as `Ok(Accept::new(..))`.
    ///
    /// Construct with [`Accept::new`] (an explicit message) or
    /// [`Accept::from_code`] (empty message, code only).
    struct Accept {
        /// A successful exit with an explicit message and code.
        new;
        /// A successful exit with an empty message and the given code.
        from_code;
        /// The message this exit hands to the host `accept` call.
        msg;
        /// The code this exit hands to the host `accept` call.
        code;
    }
}

exit_kind! {
    /// A failed exit: the message and code handed to the host `rollback` call,
    /// returned from a typed entry as `Err(Rollback::new(..))` or produced by
    /// `?` from a [`hook_errors!`] enum.
    ///
    /// Construct with [`Rollback::new`] (an explicit message) or
    /// [`Rollback::from_code`] (empty message, code only). `?` converts from
    /// every [`hook_errors!`] enum (empty message, unless the enum declares a
    /// `=> b"msg"` clause on the failing variant — see [`hook_errors!`]'s doc
    /// comment). A raw code is constructed with [`Rollback::from_code`] /
    /// [`Rollback::new`] / `Err(Rollback::from_code(code))` explicitly.
    ///
    /// **Deliberately no `From<HookError> for Rollback`.** A Hook API error
    /// code (`-1..=-45`, `-10024`) is not the hook's own return code; a
    /// `?`-propagated `HookError` → `Rollback` conversion would publish the
    /// host's code as the hook's verdict. Convert explicitly at the call site
    /// instead, discarding the `HookError` and keeping only "some call failed":
    ///
    /// ```rust,ignore
    /// let value = some_hook_api_call().map_err(|_| MyError::SomeCallFailed)?;
    /// ```
    ///
    /// — or fall back to [`crate::accept`]/[`crate::rollback`] directly when a
    /// computed (non-`'static`) message is needed.
    ///
    /// [`hook_errors!`]: crate::hook_errors
    struct Rollback {
        /// A failed exit with an explicit message and code.
        new;
        /// A failed exit with an empty message and the given code.
        from_code;
        /// The message this exit hands to the host `rollback` call.
        msg;
        /// The code this exit hands to the host `rollback` call.
        code;
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
/// is exactly one call site per entry, so the conversion cost is a single
/// 2-arm `match`, never duplicated per `?`.
///
/// **Sealed** — implemented for exactly [`HookResult`]. An entry returning
/// any other type (including `i64`) fails to compile with an ordinary
/// trait-bound diagnostic naming this trait (see
/// `tests/ui/fail/hooks_entry_return_not_entryreturn.rs`).
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

/// The `what` argument xahaud passes to a `cbak` export
/// (`Transactor::doHookCallback`, `src/xrpld/app/tx/detail/Transactor.cpp`:
/// `ctx_.tx.getTxnType() == ttEMIT_FAILURE ? 1 : 0`), decoded for a
/// `#[cbak(<index>)]` entry that declares one argument after `&self`.
///
/// [`EmitOutcome::EmitFailure`] means the emitted transaction expired
/// unapplied: the originating transaction the callback sees is the
/// `ttEMIT_FAILURE` pseudo-transaction (`TxQ.cpp`, "Emission failure,
/// adding cleanup pseudotxn") carrying `sfLedgerSequence`,
/// `sfTransactionHash` (the emitted transaction's hash) and the emitted
/// transaction's `sfEmitDetails`. Its own metadata reports `tesSUCCESS`,
/// so a callback must gate on this value before reading the applied
/// transaction's metadata (`meta_slot`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitOutcome {
    /// The emitted transaction was applied (`what == 0`); the otxn is the
    /// emitted transaction and `meta_slot` holds its metadata.
    Applied,
    /// The emitted transaction expired unapplied (`what != 0`); the otxn
    /// is the `ttEMIT_FAILURE` pseudo-transaction.
    EmitFailure,
}

impl From<u32> for EmitOutcome {
    #[inline(always)]
    fn from(what: u32) -> Self {
        if what == 0 {
            Self::Applied
        } else {
            Self::EmitFailure
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
}
