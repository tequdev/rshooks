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
    StructShape, fixed_read_impl, from_bytes_impl, max_len_expr, offset_consts, parse_struct,
    read_body, to_bytes_impl, write_body,
};
use proc_macro::TokenStream;

/// Entry point invoked by `#[proc_macro_derive(HookData)]` in `lib.rs`.
pub fn derive(input: TokenStream) -> TokenStream {
    match parse_struct(input, "HookData") {
        Ok(shape) => generate(&shape),
        Err(e) => e,
    }
}

/// `HookData`'s one addition to the shared `ToBytes` impl — the `extra`
/// text [`generate`] passes to [`crate::shape::to_bytes_impl`], including
/// its own rustdoc so the derived type's generated `with_bytes` keeps it.
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

/// `HookData`'s `FromBytes::with_read_buf` override — the read-side twin of
/// [`WITH_BYTES`], spliced into [`crate::shape::from_bytes_impl`]'s `extra`
/// slot. Right-sizes the read scratch to this struct's own
/// [`MAX_LEN`](::rshooks::convert::ToBytes::MAX_LEN) instead of
/// [`FromBytes::with_read_buf`](::rshooks::convert::FromBytes::with_read_buf)'s
/// generic-default scratch size — see that method's doc comment for why
/// only a concrete, non-generic impl (this one) can do so, and for what a
/// right-sized read buffer changes (an entry longer than `Self::MAX_LEN`
/// fails [`HookError::TooSmall`](::rshooks::error::HookError::TooSmall)
/// instead of decoding a leading prefix of it). Uses the same
/// `MaybeUninit`-scratch shape as `convert.rs`'s own primitive/`[u8; N]`
/// overrides, via the `#[doc(hidden)] pub`
/// `::rshooks::convert::Scratch`/`::rshooks::convert::uninit_slice_mut`
/// pair those overrides use directly — the same "hidden but public, for
/// generated code expanding in the invoking crate" convention as
/// `::rshooks::padded_bytes`.
const WITH_READ_BUF: &str = "
    #[inline(always)]
    fn with_read_buf<__R>(f: impl FnOnce(&mut [u8]) -> __R) -> __R {
        let mut __storage = ::core::mem::MaybeUninit::<
            ::rshooks::convert::Scratch<{ <Self as ::rshooks::convert::ToBytes>::MAX_LEN }>,
        >::uninit();
        // SAFETY: see `::rshooks::convert::FromBytes::with_read_buf`'s doc
        // comment; `f` is only ever a caller-buffer host-call funnel.
        let __buf = unsafe { ::rshooks::convert::uninit_slice_mut(&mut __storage) };
        f(__buf)
    }
";

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
        from_bytes = from_bytes_impl(name, len_expr, &offset_consts, &read_body, WITH_READ_BUF),
        fixed_read = fixed_read_impl(name, len_expr),
    );
    crate::shape::finish(src, shape.name_span, "HookData")
}
