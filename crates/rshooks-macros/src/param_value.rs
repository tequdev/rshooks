//! `#[derive(ParamValue)]` — backs `rshooks::ParamValue`.
//!
//! Turns a plain, fixed-size, named-field struct into a fixed-offset,
//! zero-cost `rshooks::convert::FromBytes`/`FixedRead` pair — the
//! **read-back** half of a Hook API parameter, decoded by
//! `hook_param_typed`/`otxn_param_typed` once the parameter's *name* (a
//! separate concept — see [`crate::param_name`]) has located it. See
//! `rshooks::ParamValue`'s doc comment for the full user-facing writeup,
//! grammar, and worked/compile-fail examples — this module only implements
//! the codegen.
//!
//! # Why a separate derive, not `#[derive(HookData)]`?
//!
//! A Hook API parameter value and a hook-state value share the same
//! fixed-offset struct shape, but a parameter value is only ever **read**
//! by this hook — there is no "write my own `hook_param`" operation, so
//! nothing here ever needs `ToBytes` on the type as a whole. `ParamValue`
//! generates only `FromBytes`/`FixedRead` — no `ToBytes`, no inherent
//! `LEN` const. Every field must still itself implement `ToBytes` (each
//! field's own `MAX_LEN`/`write` is what the generated code sums/calls
//! internally), but `Self` is never required to. Trying to write a
//! `#[derive(ParamValue)]` type back out fails to compile with an ordinary
//! rustc trait-bound error naming the missing trait.
//!
//! # Why hand-rolled, not `syn`/`quote`; codegen strategy
//!
//! Identical rationale to [`crate::hook_data`] — struct-shape parsing is
//! shared via [`crate::shape`]. The per-field offset chain is the same
//! shape as `HookData`'s, except every offset expression here is computed
//! inline, from each field's own `<FieldType as ToBytes>::MAX_LEN`, rather
//! than through `<Self as ToBytes>::MAX_LEN` — since `Self` has no
//! `ToBytes` impl to reference.

use crate::shape::{
    StructShape, fixed_read_impl, from_bytes_impl, max_len_expr, offset_consts, parse_struct,
    read_body,
};
use proc_macro::TokenStream;

/// Entry point invoked by `#[proc_macro_derive(ParamValue)]` in `lib.rs`.
pub fn derive(input: TokenStream) -> TokenStream {
    match parse_struct(input, "ParamValue") {
        Ok(shape) => generate(&shape),
        Err(e) => e,
    }
}

/// Generates the `FromBytes`/`FixedRead` impls, for an already-validated
/// [`StructShape`]. Deliberately does *not* generate `ToBytes`/an inherent
/// `LEN` const — see this module's doc comment for why.
pub(crate) fn generate(shape: &StructShape) -> TokenStream {
    let name = &shape.name;

    // `Self` has no `ToBytes` impl here, so this sum over each field's own
    // `MAX_LEN` is inlined at every use site instead of referenced via
    // `<Self as ToBytes>::MAX_LEN`.
    let total_len_expr = format!("({})", max_len_expr(&shape.fields));
    let offset_consts = offset_consts(&shape.fields);
    let read_body = read_body(&shape.fields);

    let src = format!(
        "{from_bytes}{fixed_read}",
        from_bytes = from_bytes_impl(name, &total_len_expr, &offset_consts, &read_body),
        fixed_read = fixed_read_impl(name, &total_len_expr),
    );
    crate::shape::finish(src, shape.name_span, "ParamValue")
}
