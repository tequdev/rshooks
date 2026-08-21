//! User-defined Hook error enums that map to `i64` rollback/accept codes.
//!
//! [`hook_errors!`] expands a C-like enum declaration into a `#[repr(i64)]`
//! enum plus the small amount of boilerplate needed to use it as an exit
//! code: `impl From<Enum> for i64`, an inherent `code(self) -> i64` method,
//! and `impl From<Enum> for `[`Rollback`](crate::exit::Rollback) (so `?`
//! propagates a variant straight into a typed `#[hook]`/`#[cbak]` entry's
//! [`HookResult`](crate::exit::HookResult) — see [`crate::exit`]'s module
//! doc comment). Paired with [`exit_on_err!`] and the `i64::from`-based
//! `code`/`msg` argument of [`rollback!`](crate::rollback) /
//! [`accept!`](crate::accept), this gives a small `Result`-based idiom for
//! hook logic — write ordinary helper functions returning
//! `Result<T, YourErrorEnum>`, and convert to `rollback` only at the point a
//! hook actually needs to exit — without introducing a full error-type
//! hierarchy on top of [`crate::error::HookError`] (which remains the type
//! for *Hook API* failures; user-defined error enums are a separate,
//! unrelated concept that happens to also bottom out in an `i64`).

/// Define an enum of user error codes for use as `rollback!`/`accept!` exit
/// codes.
///
/// Each variant requires an explicit `i64`-valued discriminant — the
/// macro's grammar itself enforces this, there is no separate check — and
/// an optional trailing `=> b"message"` clause, e.g.
/// `BlockedAccount = 1 => b"firewall: blocked"`. The clause is per-variant
/// (some variants may carry one and others not, in the same enum) and feeds
/// only the `From<EnumName> for Rollback` impl below — every other output
/// (the enum itself, `code()`, `From<EnumName> for i64`) is unaffected by
/// whether any variant declares one. The macro expands to:
///
/// - a `#[repr(i64)]`, `Debug + Clone + Copy + PartialEq + Eq` enum with the
///   given variants and discriminants (doc comments on the enum and on each
///   variant are passed through verbatim, so `missing_docs` is satisfied
///   the same way it would be for a hand-written enum);
/// - `impl From<EnumName> for i64`;
/// - an inherent `fn code(self) -> i64` — the same conversion, as a method,
///   for call sites that prefer `err.code()` over `i64::from(err)`;
/// - `impl From<EnumName> for `[`rshooks::exit::Rollback`](crate::exit::Rollback)
///   — the code side is always `self as i64` (a free cast — the same
///   conversion `code()`/`From<EnumName> for i64` already use); the message
///   is the variant's `=> b"msg"` clause where declared, or an empty slice
///   otherwise. Generated unconditionally so `?` propagates any
///   `hook_errors!` enum into a [`HookResult`](crate::exit::HookResult).
///   When no variant declares a clause, this is [`Rollback::from_code`]
///   (no match); otherwise a match picks the slice.
///
/// [`Rollback::from_code`]: crate::exit::Rollback::from_code
///
/// # Examples
///
/// ```
/// use rshooks::exit::Rollback;
/// use rshooks::hook_errors;
///
/// hook_errors! {
///     /// Firewall error codes.
///     pub enum FirewallError {
///         /// The sender is on the blacklist.
///         BlockedAccount = 1,
///         /// A required Hook parameter was missing.
///         MissingParam = 2,
///     }
/// }
///
/// assert_eq!(FirewallError::BlockedAccount.code(), 1);
/// assert_eq!(i64::from(FirewallError::MissingParam), 2);
///
/// let r: Rollback = FirewallError::BlockedAccount.into();
/// assert_eq!(r, Rollback::from_code(1));
/// ```
///
/// Negative discriminants work the same way (the expression after `=` is
/// not restricted to positive integer literals):
///
/// ```
/// use rshooks::hook_errors;
///
/// hook_errors! {
///     enum Negative {
///         First = -1,
///         Second = -2,
///     }
/// }
///
/// assert_eq!(Negative::First.code(), -1);
/// assert_eq!(i64::from(Negative::Second), -2);
/// ```
///
/// A per-variant `=> b"msg"` clause feeds `From<EnumName> for Rollback` —
/// declared variants carry their message, undeclared ones fall back to an
/// empty message, in the same enum:
///
/// ```
/// use rshooks::exit::Rollback;
/// use rshooks::hook_errors;
///
/// hook_errors! {
///     pub enum DepositError {
///         BadAmount = 1 => b"deposit: bad amount",
///         StateSetFailed = 2,
///     }
/// }
///
/// let with_msg: Rollback = DepositError::BadAmount.into();
/// assert_eq!(with_msg, Rollback::new(b"deposit: bad amount", 1));
///
/// let without_msg: Rollback = DepositError::StateSetFailed.into();
/// assert_eq!(without_msg, Rollback::from_code(2));
/// ```
#[macro_export]
macro_rules! hook_errors {
    (@__msg) => {
        b""
    };
    (@__msg $msg:expr) => {
        $msg
    };
    // No `=> b"msg"` on any variant: `from_code` is a free cast (no match).
    // `#[inline(always)]`: this `From` is on every `?` of a `hook_errors!` enum.
    (@__from_rollback $name:ident; $($variant:ident =>);+) => {
        impl ::core::convert::From<$name> for $crate::exit::Rollback {
            #[inline(always)]
            fn from(value: $name) -> $crate::exit::Rollback {
                $crate::exit::Rollback::from_code(value as i64)
            }
        }
    };
    // At least one message clause: match to pick the slice.
    (@__from_rollback $name:ident; $($variant:ident => $($msg:expr)?);+) => {
        impl ::core::convert::From<$name> for $crate::exit::Rollback {
            #[inline(always)]
            fn from(value: $name) -> $crate::exit::Rollback {
                let msg: &'static [u8] = match value {
                    $(
                        $name::$variant => $crate::hook_errors!(@__msg $($msg)?),
                    )+
                };
                $crate::exit::Rollback::new(msg, value as i64)
            }
        }
    };
    (
        $(#[$enum_meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident = $code:expr $(=> $msg:expr)?
            ),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[repr(i64)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $vis enum $name {
            $(
                $(#[$variant_meta])*
                $variant = $code,
            )+
        }

        impl $name {
            /// The `i64` rollback/accept code for this variant.
            #[must_use]
            $vis fn code(self) -> i64 {
                self as i64
            }
        }

        impl ::core::convert::From<$name> for i64 {
            fn from(value: $name) -> i64 {
                value as i64
            }
        }

        $crate::hook_errors! { @__from_rollback $name; $($variant => $($msg)?);+ }
    };
}

/// Evaluate a `Result<T, E>` (typically returned by a plain helper
/// function), returning `T` on `Ok`, or rolling the hook back on `Err`.
///
/// `E` must implement `Into<i64>` — any enum defined with [`hook_errors!`]
/// does, via the `From<EnumName> for i64` impl it generates; a plain `i64`
/// error works too, since `i64: From<i64>`. Expands to a `match` that calls
/// [`rollback!`](crate::rollback) with `msg` and the error on the `Err` arm
/// (which — like `rollback!` itself — never returns on the real wasm host);
/// this is the same "convert at the boundary" pattern the crate already
/// uses for [`accept!`](crate::accept)/[`rollback!`](crate::rollback)'s own
/// `i64::from($code)` handling of `Into<i64>` codes.
///
/// # Examples
///
/// The `Ok` path just returns the wrapped value, so it is safe to run as an
/// ordinary doctest:
///
/// ```
/// use rshooks::{exit_on_err, hook_errors};
///
/// hook_errors! {
///     /// Firewall error codes.
///     pub enum FirewallError {
///         /// The sender is on the blacklist.
///         BlockedAccount = 1,
///     }
/// }
///
/// fn check(blocked: bool) -> Result<u32, FirewallError> {
///     if blocked {
///         Err(FirewallError::BlockedAccount)
///     } else {
///         Ok(42)
///     }
/// }
///
/// let value = exit_on_err!(b"firewall: blocked", check(false));
/// assert_eq!(value, 42);
/// ```
///
/// The `Err` path rolls back, which on a host build never returns (see
/// [`crate::api::control::rollback`]'s own doc comment for why) — so this
/// variant is `no_run`; on the real wasm host it unwinds hook execution
/// instead of looping:
///
/// ```no_run
/// use rshooks::{exit_on_err, hook_errors};
///
/// hook_errors! {
///     /// Firewall error codes.
///     pub enum FirewallError {
///         /// The sender is on the blacklist.
///         BlockedAccount = 1,
///     }
/// }
///
/// fn check(blocked: bool) -> Result<u32, FirewallError> {
///     if blocked {
///         Err(FirewallError::BlockedAccount)
///     } else {
///         Ok(42)
///     }
/// }
///
/// // Never returns: rolls back with b"firewall: blocked" and code 1.
/// let _value = exit_on_err!(b"firewall: blocked", check(true));
/// ```
#[macro_export]
macro_rules! exit_on_err {
    ($msg:expr, $result:expr) => {
        match $result {
            ::core::result::Result::Ok(value) => value,
            ::core::result::Result::Err(err) => $crate::rollback!($msg, err),
        }
    };
}

#[cfg(test)]
mod tests {
    use crate::exit::Rollback;

    hook_errors! {
        /// Test error enum exercising positive discriminants.
        pub enum SampleError {
            /// First variant.
            First = 1,
            /// Second variant.
            Second = 2,
        }
    }

    hook_errors! {
        /// Test error enum exercising negative discriminants.
        enum NegativeError {
            /// First variant.
            First = -1,
            /// Second variant.
            Second = -2,
        }
    }

    hook_errors! {
        /// Test error enum exercising a mix of clause and clause-less
        /// variants, for the `From<Enum> for Rollback` msg lookup.
        enum MixedMsgError {
            WithMsg = 1 => b"mixed: with message",
            WithoutMsg = 2,
        }
    }

    #[test]
    fn code_matches_discriminant() {
        assert_eq!(SampleError::First.code(), 1);
        assert_eq!(SampleError::Second.code(), 2);
    }

    #[test]
    fn into_i64_matches_code() {
        assert_eq!(i64::from(SampleError::First), SampleError::First.code());
        assert_eq!(i64::from(SampleError::Second), SampleError::Second.code());
    }

    #[test]
    fn negative_discriminants_round_trip() {
        assert_eq!(NegativeError::First.code(), -1);
        assert_eq!(i64::from(NegativeError::Second), -2);
    }

    #[test]
    fn derives_are_usable() {
        extern crate std;
        use std::format;

        // Debug + Clone + Copy + PartialEq + Eq, as documented.
        let a = SampleError::First;
        let b = a;
        assert_eq!(a, b);
        assert_eq!(format!("{a:?}"), "First");
    }

    #[test]
    fn exit_on_err_returns_ok_value() {
        let result: Result<u32, SampleError> = Ok(7);
        let value = exit_on_err!(b"unused", result);
        assert_eq!(value, 7);
    }

    #[test]
    fn clause_less_enum_still_converts_to_rollback_with_empty_msg() {
        // Backward compatibility (D2): an enum with no `=> b"msg"` clause
        // anywhere still gets `From<Enum> for Rollback`, msg always empty.
        let r: Rollback = SampleError::First.into();
        assert_eq!(r, Rollback::from_code(1));
        let r: Rollback = NegativeError::Second.into();
        assert_eq!(r, Rollback::from_code(-2));
    }

    #[test]
    fn msg_clause_feeds_rollback_message() {
        assert_eq!(MixedMsgError::WithMsg.code(), 1);
        let r: Rollback = MixedMsgError::WithMsg.into();
        assert_eq!(r, Rollback::new(b"mixed: with message", 1));
    }

    #[test]
    fn variant_without_msg_clause_falls_back_to_empty_in_a_mixed_enum() {
        let r: Rollback = MixedMsgError::WithoutMsg.into();
        assert_eq!(r, Rollback::from_code(2));
    }
}
