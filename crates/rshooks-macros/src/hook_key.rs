//! `#[derive(HookKey)]` — backs `rshooks::HookKey`.
//!
//! Turns a plain, fixed-size, named-field struct into a fixed-offset,
//! zero-cost `rshooks::convert::ToBytes` impl **plus** an explicit
//! `rshooks::state::StateKeyEncode` impl — for use as a **composite
//! hook-state key**. The generated `encode()` sends the struct's own real
//! encoded length (`<= rshooks::types::STATE_KEY_LEN`, 32 bytes), never
//! locally zero-padded up to 32: the Hook API host left-pads a shorter key
//! itself (see `rshooks::state`'s module doc comment, "Key length and
//! padding"). See `rshooks::HookKey`'s doc comment for the full user-facing
//! writeup, grammar, and worked/compile-fail examples — this module only
//! implements the codegen.
//!
//! # Why a separate derive, not `#[derive(HookData)]`?
//!
//! A hook-state key and a hook-state value share the same fixed-offset
//! struct shape but play different roles:
//!
//! - A state key is only ever **written** (handed to the host to locate a
//!   value) — never read back and decoded as itself. So `HookKey`
//!   generates only `ToBytes` (needed for [`crate::shape`]'s per-field
//!   codegen and nesting) plus `StateKeyEncode` — no `FromBytes`, no
//!   `FixedRead`, no inherent `LEN` const. `#[derive(HookData)]` (see
//!   [`crate::hook_data`]) generates the full read/write triple for state
//!   values, which genuinely are read back.
//! - A state key's real encoded length must fit within
//!   `rshooks::types::STATE_KEY_LEN` (32) bytes, with no lower bound to
//!   enforce (a `HookKey` struct always has at least one field by its own
//!   grammar, so its encoded length is never 0) — unlike a Hook API
//!   parameter name (1 to 32 bytes, see [`crate::param_name`]), which the
//!   Hook API itself rejects below 1 byte. `HookKey` bakes the upper bound
//!   in as a compile-time assert generated alongside the impls, so a
//!   struct that encodes to more than 32 bytes fails to compile at its own
//!   definition.
//!
//! # Why hand-rolled, not `syn`/`quote`; codegen strategy
//!
//! Identical rationale to [`crate::hook_data`] — struct-shape parsing is
//! shared via [`crate::shape`]; only the generated impl set differs.

use crate::shape::{
    StructShape, max_len_expr, offset_consts, parse_struct, to_bytes_impl, write_body,
};
use proc_macro::TokenStream;

/// Entry point invoked by `#[proc_macro_derive(HookKey)]` in `lib.rs`.
pub fn derive(input: TokenStream) -> TokenStream {
    match parse_struct(input, "HookKey") {
        Ok(shape) => generate(&shape),
        Err(e) => e,
    }
}

/// `HookKey`'s one addition to the shared `ToBytes` impl — the `extra`
/// text [`generate`] passes to [`crate::shape::to_bytes_impl`]. Identical
/// mechanism to [`crate::hook_data`]'s `WITH_BYTES` — see that constant's
/// doc comment for why only a concrete, non-generic impl can size the
/// scratch buffer to `Self::MAX_LEN`. Distinct from
/// [`StateKeyEncode::with_key_bytes`](::rshooks::state::StateKeyEncode::with_key_bytes)
/// below, which right-sizes the *key* buffer for `StateKeyEncode` callers;
/// this overrides `ToBytes::with_bytes` itself, for any caller that treats
/// the struct as an ordinary `ToBytes` value.
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

/// Generates the `ToBytes` impl plus an explicit `StateKeyEncode` impl
/// (with its 32-byte compile-time length assert), for an already-validated
/// [`StructShape`]. Does not generate `FromBytes`/`FixedRead`/an inherent
/// `LEN` const — see this module's doc comment for why.
pub(crate) fn generate(shape: &StructShape) -> TokenStream {
    let name = &shape.name;

    let max_len_expr = max_len_expr(&shape.fields);
    let offset_consts = offset_consts(&shape.fields);
    let write_body = write_body(&shape.fields);

    let src = format!(
        "{to_bytes}\n{state_key_encode}",
        to_bytes = to_bytes_impl(
            name,
            &max_len_expr,
            &format!("{offset_consts}\n{write_body}"),
            WITH_BYTES,
        ),
        state_key_encode = state_key_encode_impl(name),
    );
    crate::shape::finish(src, shape.name_span, "HookKey")
}

/// Generates the `StateKeyEncode` impl for `name` (which must already have
/// a `ToBytes` impl in scope): a compile-time assert that `<name as
/// ToBytes>::MAX_LEN` is `1..=STATE_KEY_LEN`, followed by `encode()`'s
/// body, writing `self` into a 32-byte scratch buffer and wrapping the
/// result in an [`EncodedStateKey`](::rshooks::state::EncodedStateKey) at
/// its real length (never locally zero-padded — see `rshooks::state`'s
/// module doc comment, "Key length and padding").
///
/// Also generates a `with_key_bytes` override (see
/// [`StateKeyEncode::with_key_bytes`](::rshooks::state::StateKeyEncode::with_key_bytes)'s
/// doc comment for why this exists): rather than routing through `encode`'s
/// always-32-byte `EncodedStateKey` scratch buffer, it writes `self` into a
/// buffer sized to exactly `<{name} as ToBytes>::MAX_LEN` bytes — a literal
/// this concrete, non-generic `impl` block already knows at its own
/// definition site (unlike a generic default trait method — see
/// `FixedRead::read_exact`'s doc comment for the identical restriction) —
/// and hands that right-sized slice straight to the caller's closure, with
/// no `EncodedStateKey` built at all. Carries its own copy of `encode`'s
/// compile-time length assert, since an override replaces the default body
/// (assert included).
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

    #[inline(always)]
    fn with_key_bytes<__R>(&self, f: impl ::core::ops::FnOnce(&[u8]) -> __R) -> __R {{
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
        let mut __raw = [0u8; <{name} as ::rshooks::convert::ToBytes>::MAX_LEN];
        let _ = ::rshooks::convert::ToBytes::write(self, &mut __raw);
        f(&__raw)
    }}
}}
",
        name = name,
    )
}
