//! `#[derive(HookKey)]` — backs `rshooks::HookKey`.
//!
//! Turns a plain, fixed-size, named-field struct into a fixed-offset,
//! zero-cost `rshooks::convert::ToBytes` impl **plus** an explicit
//! `rshooks::state::StateKeyEncode` impl — for use as a **composite
//! hook-state key**. The generated `encode()` sends the struct's own real
//! encoded length (`<= rshooks::types::STATE_KEY_LEN`, 32 bytes), never
//! locally zero-padded up to 32: the Hook API host left-pads a shorter key
//! itself (see `rshooks::state`'s module doc comment, "Key length and
//! padding"). See `rshooks::HookKey`'s doc comment (the public-facing
//! re-export site) for the full user-facing writeup, grammar, and
//! worked/compile-fail examples — this module only implements the codegen.
//!
//! # Why a separate derive, not `#[derive(HookData)]`?
//!
//! A hook-state key and a hook-state value share the same "fixed-offset
//! struct" shape, but play different roles, and `HookKey` is deliberately
//! narrower than `HookData`:
//!
//! - A state key is only ever **written** (handed to the host to locate a
//!   value) — never read back and decoded as itself. So `HookKey`
//!   generates only `rshooks::convert::ToBytes` (the encoding half,
//!   needed for [`crate::shape`]'s per-field codegen and for nesting) plus
//!   `StateKeyEncode` — no `FromBytes`, no `FixedRead`, no inherent `LEN`
//!   const. The value-side counterpart is `#[derive(HookData)]` (see
//!   [`crate::hook_data`]), which *does* generate the full read/write
//!   triple, since a state value genuinely is read back.
//! - A hook state key has its own length bound distinct from a Hook API
//!   *parameter* name's (1 to 32 bytes, see [`crate::param_name`]): a state
//!   key's real encoded length must fit within
//!   `rshooks::types::STATE_KEY_LEN` (32) bytes — there is no lower bound
//!   to enforce here (unlike a parameter name, which the Hook API itself
//!   rejects below 1 byte; a `HookKey` struct always has at least one field
//!   by its own grammar, so its encoded length is never 0). `HookKey` bakes
//!   the upper bound in as an unconditional compile-time assert generated
//!   alongside the impls — a `#[derive(HookKey)]` struct that encodes to
//!   more than 32 bytes fails to compile *at its own definition*, not only
//!   later at whatever call site first tries to use it as a key.
//!
//! See `rshooks::HookData`'s doc comment for the reciprocal note (use
//! `HookKey` for state keys, `HookData` for state values, `ParamName`/
//! `ParamValue` for Hook API parameter names/values).
//!
//! # Why hand-rolled, not `syn`/`quote`; codegen strategy
//!
//! Identical rationale to [`crate::hook_data`] (see that module's doc
//! comment) — struct-shape parsing is shared via [`crate::shape`]; only
//! the generated impl set differs.

use crate::err;
use crate::shape::{StructShape, parse_struct};
use proc_macro::TokenStream;

/// Entry point invoked by `#[proc_macro_derive(HookKey)]` in `lib.rs`.
pub fn derive(input: TokenStream) -> TokenStream {
    match parse_struct(input, "HookKey") {
        Ok(shape) => generate(&shape),
        Err(e) => e,
    }
}

/// Generates the `ToBytes` impl plus an explicit `StateKeyEncode` impl
/// (with its 32-byte compile-time length assert), for an already-validated
/// [`StructShape`]. Deliberately does *not* generate `FromBytes`/
/// `FixedRead`/an inherent `LEN` const — see this module's doc comment for
/// why.
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

{state_key_encode}
",
        name = name,
        max_len_expr = max_len_expr,
        offset_consts = offset_consts,
        write_body = write_body,
        state_key_encode = state_key_encode_impl(name),
    );

    match src.parse::<TokenStream>() {
        Ok(ts) => ts,
        Err(_) => err(
            shape.name_span,
            "rshooks-macros: internal HookKey codegen failed to parse",
        ),
    }
}

/// Generates the `StateKeyEncode` impl for `name` (which must already have
/// a `ToBytes` impl in scope): a compile-time (monomorphized) assert that
/// `<name as ToBytes>::MAX_LEN` is `1..=STATE_KEY_LEN` — the Hook API's own
/// key-length bound — followed by `encode()`'s body, writing `self` into a
/// 32-byte scratch buffer via its own `ToBytes::write` and wrapping the
/// result in an [`EncodedStateKey`](::rshooks::state::EncodedStateKey) at
/// its real length (never locally zero-padded — see `rshooks::state`'s
/// module doc comment, "Key length and padding").
///
/// Factored out of [`derive`] so the generated `StateKeyEncode` impl body
/// stays in one place.
pub(crate) fn state_key_encode_impl(name: &str) -> String {
    format!(
        "
#[automatically_derived]
impl ::rshooks::state::StateKeyEncode for {name} {{
    #[inline(always)]
    fn encode(&self) -> ::rshooks::state::EncodedStateKey {{
        const {{
            assert!(
                <{name} as ::rshooks::convert::ToBytes>::MAX_LEN >= 1,
                \"rshooks-macros: a hook-state key must encode to at least 1 byte (the Hook API's own key-length lower bound)\"
            );
            assert!(
                <{name} as ::rshooks::convert::ToBytes>::MAX_LEN <= ::rshooks::types::STATE_KEY_LEN,
                \"rshooks-macros: a hook-state key would need more than 32 bytes to encode (the state key space)\"
            );
        }}
        let mut __raw = [0u8; ::rshooks::types::STATE_KEY_LEN];
        let _ = ::rshooks::convert::ToBytes::write(self, &mut __raw);
        ::rshooks::state::EncodedStateKey::new(
            __raw,
            <{name} as ::rshooks::convert::ToBytes>::MAX_LEN,
        )
    }}
}}
",
        name = name,
    )
}
