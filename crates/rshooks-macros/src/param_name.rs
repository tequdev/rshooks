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

use crate::err;
use crate::shape::{StructShape, parse_struct};
use proc_macro::TokenStream;

/// Entry point invoked by `#[proc_macro_derive(ParamName)]` in `lib.rs`.
pub fn derive(input: TokenStream) -> TokenStream {
    match parse_struct(input, "ParamName") {
        Ok(shape) => generate(&shape),
        Err(e) => e,
    }
}

/// Generates the `ToBytes` impl plus the 1–32-byte compile-time length
/// assert, for an already-validated [`StructShape`]. Deliberately does
/// *not* generate `FromBytes`/`FixedRead`/an inherent `LEN` const — see
/// this module's doc comment for why.
pub(crate) fn generate(shape: &StructShape) -> TokenStream {
    let name = &shape.name;

    let mut max_len_expr = String::from("0usize");
    for f in &shape.fields {
        max_len_expr.push_str(&format!(
            " + <{ty} as ::rshooks::convert::ToBytes>::MAX_LEN",
            ty = f.ty
        ));
    }

    let mut offset_consts = String::from("const __OFF_0: usize = 0usize;\n");
    for (i, f) in shape.fields.iter().enumerate() {
        offset_consts.push_str(&format!(
            "const __OFF_{next}: usize = __OFF_{i} + <{ty} as ::rshooks::convert::ToBytes>::MAX_LEN;\n",
            next = i.wrapping_add(1),
            i = i,
            ty = f.ty,
        ));
    }

    let mut write_body = String::new();
    for (i, f) in shape.fields.iter().enumerate() {
        write_body.push_str(&format!(
            "let _ = ::rshooks::convert::ToBytes::write(&self.{field}, &mut __dst[__OFF_{i}..__OFF_{next}]);\n",
            field = f.name,
            i = i,
            next = i.wrapping_add(1),
        ));
    }

    let src = format!(
        "
#[automatically_derived]
impl ::rshooks::convert::ToBytes for {name} {{
    const MAX_LEN: usize = {max_len_expr};

    #[inline(always)]
    #[allow(clippy::indexing_slicing)] // fixed, compile-time field offsets (see __OFF_* below); `__dst` was already proven to have exactly `MAX_LEN` bytes by the `get_mut(..MAX_LEN)` check\n\
    fn write(&self, buf: &mut [u8]) -> usize {{
        match buf.get_mut(..<Self as ::rshooks::convert::ToBytes>::MAX_LEN) {{
            ::core::option::Option::Some(__dst) => {{
                {offset_consts}
                {write_body}
                <Self as ::rshooks::convert::ToBytes>::MAX_LEN
            }}
            ::core::option::Option::None => 0,
        }}
    }}
}}

{length_assert}
",
        name = name,
        max_len_expr = max_len_expr,
        offset_consts = offset_consts,
        write_body = write_body,
        length_assert = param_name_length_assert(name),
    );
    let src = crate::krate::rewrite(src);

    match src.parse::<TokenStream>() {
        Ok(ts) => ts,
        Err(_) => err(
            shape.name_span,
            "rshooks-macros: internal ParamName codegen failed to parse",
        ),
    }
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
