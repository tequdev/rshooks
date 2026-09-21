//! `#[derive(ParamName)]` — backs `rshooks::ParamName`.
//!
//! Turns a plain, fixed-size, named-field struct into a fixed-offset,
//! zero-cost `rshooks::convert::ToBytes` impl for use as a **composite
//! Hook API parameter name** — satisfying
//! `rshooks::convert::TypedParamName`'s `Self: ToBytes` supertrait bound,
//! so a hand-written `TypedParamName` impl can pair it with a value type
//! directly. See `rshooks::ParamName`'s doc comment for the full
//! user-facing writeup, grammar, and worked/compile-fail examples — this
//! module only implements the codegen.
//!
//! # Why a separate derive, not `#[derive(HookData)]`?
//!
//! A hook-state key/value (`HookKey`/`HookData`) and a Hook API parameter
//! *name* share the same fixed-offset struct shape but are different
//! concepts:
//!
//! - A parameter name is only ever **written** — handed to
//!   `hook_param`/`otxn_param` to locate a value, never read back and
//!   decoded as itself. So `ParamName` generates only `ToBytes` — no
//!   `FromBytes`, no `FixedRead`, no inherent `LEN` const or layout doc
//!   table. Trying to read a `#[derive(ParamName)]` type back as a value
//!   fails to compile with an ordinary rustc trait-bound error naming the
//!   missing trait. The value-side counterpart is `#[derive(ParamValue)]`
//!   (see [`crate::param_value`]).
//! - A Hook API parameter name has its own length bound the Hook API
//!   itself enforces (`hook_api.h`: 1 to 32 bytes — see
//!   `rshooks::convert::PARAM_NAME_MAX_LEN`), distinct from a hook state
//!   key's fixed 32 bytes (see [`crate::hook_key`]) or a hook state
//!   value's lack of any size cap. `ParamName` bakes this in as a
//!   compile-time assert generated alongside the `ToBytes` impl, so a
//!   struct that encodes to 0 or to 33+ bytes fails to compile at its own
//!   definition.
//!
//! # Why hand-rolled, not `syn`/`quote`; codegen strategy
//!
//! Identical rationale to [`crate::hook_data`] — struct-shape parsing is
//! shared via [`crate::shape`]; only the `ToBytes`-only-plus-length-assert
//! generation differs.

use crate::shape::{
    StructShape, max_len_expr, offset_consts, parse_struct, to_bytes_impl, write_body,
};
use proc_macro::TokenStream;

/// Entry point invoked by `#[proc_macro_derive(ParamName)]` in `lib.rs`.
pub fn derive(input: TokenStream) -> TokenStream {
    match parse_struct(input, "ParamName") {
        Ok(shape) => generate(&shape),
        Err(e) => e,
    }
}

/// `ParamName`'s one addition to the shared `ToBytes` impl — the `extra`
/// text [`generate`] passes to [`crate::shape::to_bytes_impl`]. Identical
/// mechanism to [`crate::hook_data`]'s `WITH_BYTES` — see that constant's
/// doc comment for why only a concrete, non-generic impl can size the
/// scratch buffer to `Self::MAX_LEN`.
const WITH_BYTES: &str = "
    /// Encodes into a buffer sized to this struct's own
    /// [`MAX_LEN`](::rshooks::convert::ToBytes::MAX_LEN) rather than
    /// [`ToBytes::with_bytes`](::rshooks::convert::ToBytes::with_bytes)'s
    /// generic-default scratch size — see that method's doc comment for why
    /// only a concrete, non-generic impl (this one) can do so. `__buf` is
    /// exactly `MAX_LEN` bytes, so `write` always succeeds and fills all of
    /// it — the whole buffer is handed to `f` directly, with no slicing on
    /// `write`'s return value.
    #[inline(always)]
    fn with_bytes<__R>(&self, f: impl FnOnce(&[u8]) -> __R) -> __R {
        let mut __buf = [0u8; <Self as ::rshooks::convert::ToBytes>::MAX_LEN];
        let _ = <Self as ::rshooks::convert::ToBytes>::write(self, &mut __buf);
        f(&__buf)
    }
";

/// Generates the `ToBytes` impl plus the 1–32-byte compile-time length
/// assert, for an already-validated [`StructShape`]. Deliberately does
/// *not* generate `FromBytes`/`FixedRead`/an inherent `LEN` const — see
/// this module's doc comment for why.
pub(crate) fn generate(shape: &StructShape) -> TokenStream {
    let name = &shape.name;

    let max_len_expr = max_len_expr(&shape.fields);
    let offset_consts = offset_consts(&shape.fields);
    let write_body = write_body(&shape.fields);

    let src = format!(
        "{to_bytes}\n{length_assert}",
        to_bytes = to_bytes_impl(
            name,
            &max_len_expr,
            &format!("{offset_consts}\n{write_body}"),
            WITH_BYTES,
        ),
        length_assert = param_name_length_assert(name),
    );
    crate::shape::finish(src, shape.name_span, "ParamName")
}

/// Generates the compile-time assert that `<name as ToBytes>::MAX_LEN` is
/// `1..=PARAM_NAME_MAX_LEN` — the Hook API's own parameter-name length
/// bound (`name` must already have a `ToBytes` impl in scope).
///
/// Factored out so [`crate::hooks_struct`]'s `name_by` field codegen can
/// reuse the exact same logic instead of a second, potentially-drifting
/// copy.
pub(crate) fn param_name_length_assert(name: &str) -> String {
    format!(
        "
#[automatically_derived]
const _: () = {{
    assert!(
        <{name} as ::rshooks::convert::ToBytes>::MAX_LEN >= 1,
        \"rshooks-macros: a Hook API parameter name must encode to at least 1 byte (the Hook API's parameter-name lower bound)\"
    );
    assert!(
        <{name} as ::rshooks::convert::ToBytes>::MAX_LEN <= ::rshooks::convert::PARAM_NAME_MAX_LEN,
        \"rshooks-macros: a Hook API parameter name would exceed the Hook API's 32-byte parameter-name upper bound\"
    );
}};
",
        name = name,
    )
}
