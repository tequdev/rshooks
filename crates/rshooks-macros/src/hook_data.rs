//! `#[derive(HookData)]` — backs `rshooks::HookData`.
//!
//! Turns a plain, fixed-size, named-field struct into a fixed-offset,
//! zero-cost `rshooks::convert::ToBytes`/`FromBytes`/`FixedRead` triple,
//! for a **hook-state value** — read back and decoded by
//! `state_get_typed`/`state_get`, written by `state_set_typed`/`state_set_loose`.
//! See `rshooks::HookData`'s doc comment for the user-facing writeup,
//! grammar, and worked/compile-fail examples — this module only implements
//! the codegen.
//!
//! Three sibling derives cover the other roles a fixed-offset struct plays
//! in this crate:
//!
//! - [`crate::hook_key`]'s `#[derive(HookKey)]` — a hook-state **key**
//!   (write-only, plus a `StateKeyEncode` impl with a 32-byte bound checked
//!   at derive time).
//! - [`crate::param_name`]'s `#[derive(ParamName)]` — a composite Hook API
//!   parameter **name** (write-only, 1–32-byte bound checked at derive
//!   time).
//! - [`crate::param_value`]'s `#[derive(ParamValue)]` — a Hook API
//!   parameter **value** (read-only).
//!
//! Shape-recognition rationale: see the crate doc comment in `lib.rs`.
//! Shared with [`crate::hook_key`]/[`crate::param_name`]/
//! [`crate::param_value`] via [`crate::shape`].
//!
//! # Codegen strategy
//!
//! Each field's byte width is `<FieldType as ToBytes>::MAX_LEN`, an
//! associated-const expression this macro cannot resolve (it only sees a
//! field's type as syntax). So instead of literal numeric offsets, the
//! generated code emits a chain of `const __OFF_N: usize = __OFF_{N-1} +
//! <FieldTypeN as ToBytes>::MAX_LEN;` declarations — one per field
//! boundary — and every field read/write indexes `__dst[__OFF_i..__OFF_{i+1}]`
//! against those consts. All offsets are compile-time constants and every
//! per-field copy delegates to that field's own `ToBytes::write`/
//! `FromBytes::read`, matching the same unrolled fixed-offset shape used by
//! `rshooks::txn::codec` and `txn_template!`.
//!
//! # Why the generated code uses absolute `rshooks` paths
//!
//! This derive is re-exported as `rshooks::HookData`, so every crate that
//! can invoke it already depends on `rshooks`, without requiring the
//! invoking module to have `convert`/`error` in scope via `use`. The
//! generated code references `convert::{ToBytes, FromBytes, FixedRead}` and
//! `error::{HookError, Result}` through an absolute path built at
//! expansion time by [`crate::krate::rewrite`] — `::rshooks::` normally,
//! `crate::` when compiled as part of `rshooks` itself, or whatever name a
//! consumer's `Cargo.toml` gives the dependency (`hooks = { package =
//! "rshooks", .. }`).

use crate::shape::{
    StructShape, WITH_BYTES, fixed_read_impl, from_bytes_impl, max_len_expr, offset_consts,
    parse_struct, read_body, to_bytes_impl, write_body,
};
use proc_macro::TokenStream;

/// Entry point invoked by `#[proc_macro_derive(HookData)]` in `lib.rs`.
pub fn derive(input: TokenStream) -> TokenStream {
    match parse_struct(input, "HookData") {
        Ok(shape) => generate(&shape),
        Err(e) => e,
    }
}

/// Generates the `ToBytes`/`FromBytes`/`FixedRead` impls plus the inherent
/// `LEN` const, for an already-validated [`StructShape`].
pub(crate) fn generate(shape: &StructShape) -> TokenStream {
    let name = &shape.name;

    let max_len_expr = max_len_expr(&shape.fields);
    let offset_consts = offset_consts(&shape.fields);
    let write_body = write_body(&shape.fields);
    let read_body = read_body(&shape.fields);
    let len_expr = "<Self as ::rshooks::convert::ToBytes>::MAX_LEN";

    let src = format!(
        "{to_bytes}{from_bytes}{fixed_read}
impl {name} {{
    /// Total encoded length in bytes: every field's own
    /// [`ToBytes::MAX_LEN`](::rshooks::convert::ToBytes::MAX_LEN), summed
    /// in declaration order, no padding between fields.
    pub const LEN: usize = <Self as ::rshooks::convert::ToBytes>::MAX_LEN;
}}
",
        to_bytes = to_bytes_impl(
            name,
            &max_len_expr,
            &format!("{offset_consts}\n{write_body}"),
            WITH_BYTES,
        ),
        from_bytes = from_bytes_impl(name, len_expr, &offset_consts, &read_body),
        fixed_read = fixed_read_impl(name, len_expr),
    );
    crate::shape::finish(src, shape.name_span, "HookData")
}
