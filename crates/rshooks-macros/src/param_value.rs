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

use crate::err;
use crate::shape::{StructShape, parse_struct};
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
    let mut total_len_expr = String::from("0usize");
    for f in &shape.fields {
        total_len_expr.push_str(&format!(
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

    let mut read_body = String::new();
    for (i, f) in shape.fields.iter().enumerate() {
        read_body.push_str(&format!(
            "{field}: <{ty} as ::rshooks::convert::FromBytes>::read(&__src[__OFF_{i}..__OFF_{next}])?,\n",
            field = f.name,
            ty = f.ty,
            i = i,
            next = i.wrapping_add(1),
        ));
    }

    let src = format!(
        "
#[automatically_derived]
impl ::rshooks::convert::FromBytes for {name} {{
    #[inline(always)]
    #[allow(clippy::indexing_slicing)] // fixed, compile-time field offsets (see __OFF_* below); `__src` was already proven to have exactly this many bytes by the `get(..)` check\n\
    fn read(buf: &[u8]) -> ::rshooks::error::Result<Self> {{
        let __src = buf.get(..({total_len_expr}))
            .ok_or(::rshooks::error::HookError::TooSmall)?;
        {offset_consts}
        ::core::result::Result::Ok(Self {{
            {read_body}
        }})
    }}
}}

#[automatically_derived]
impl ::rshooks::convert::FixedRead for {name} {{
    #[inline(always)]
    fn read_exact(
        read: impl FnOnce(&mut [u8]) -> ::rshooks::error::Result<usize>,
    ) -> ::rshooks::error::Result<Self> {{
        let mut __buf = [0u8; {total_len_expr}];
        let __written = read(&mut __buf)?;
        if __written == ({total_len_expr}) {{
            <Self as ::rshooks::convert::FromBytes>::read(&__buf)
        }} else {{
            ::core::result::Result::Err(::rshooks::error::HookError::TooSmall)
        }}
    }}
}}
",
        name = name,
        total_len_expr = total_len_expr,
        offset_consts = offset_consts,
        read_body = read_body,
    );

    match src.parse::<TokenStream>() {
        Ok(ts) => ts,
        Err(_) => err(
            shape.name_span,
            "rshooks-macros: internal ParamValue codegen failed to parse",
        ),
    }
}
