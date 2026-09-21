//! Shared named-field-struct parsing, backing all four fixed-offset struct
//! derives: [`crate::hook_key`]'s `#[derive(HookKey)]`,
//! [`crate::hook_data`]'s `#[derive(HookData)]`,
//! [`crate::param_name`]'s `#[derive(ParamName)]`, and
//! [`crate::param_value`]'s `#[derive(ParamValue)]`.
//!
//! All four need to recognize exactly the same input shape — a plain,
//! non-generic, named-field struct, each field a bare `name: Type` pair —
//! and differ only in what they generate from it (see each module's own
//! doc comment). Factored out here so the shape-recognition logic has one
//! definition, not four copies that could drift apart.
//!
//! [`to_bytes_impl`]/[`from_bytes_impl`]/[`fixed_read_impl`] generate the
//! `ToBytes`/`FromBytes`/`FixedRead` impl blocks themselves — the same
//! fixed-offset shape every one of the four derives (plus
//! [`crate::hooks_struct`]'s `#[state_interface(..)]` value struct) needs,
//! parameterized over each caller's own field-write/-read bodies and length
//! expression.

use crate::err;
use proc_macro::{Delimiter, Span, TokenStream, TokenTree};
use std::iter::Peekable;

/// Sums every field's `<FieldType as ToBytes>::MAX_LEN`, as a source-text
/// expression — the shared shape behind `HookKey`/`HookData`/`ParamName`'s
/// `ToBytes::MAX_LEN` const and `ParamValue`'s inline total-length
/// expression (see each module's own doc comment for why `ParamValue`
/// cannot instead reference `<Self as ToBytes>::MAX_LEN`).
pub(crate) fn max_len_expr(fields: &[FieldShape]) -> String {
    let mut expr = String::from("0usize");
    for f in fields {
        expr.push_str(&format!(
            " + <{ty} as ::rshooks::convert::ToBytes>::MAX_LEN",
            ty = f.ty
        ));
    }
    expr
}

/// Emits the `const __OFF_N: usize = __OFF_{N-1} + <FieldTypeN as
/// ToBytes>::MAX_LEN;` chain every fixed-offset derive indexes its
/// `write`/`read` body against — see [`crate::hook_data`]'s "Codegen
/// strategy" doc section.
pub(crate) fn offset_consts(fields: &[FieldShape]) -> String {
    let mut consts = String::from("const __OFF_0: usize = 0usize;\n");
    for (i, f) in fields.iter().enumerate() {
        consts.push_str(&format!(
            "const __OFF_{next}: usize = __OFF_{i} + <{ty} as ::rshooks::convert::ToBytes>::MAX_LEN;\n",
            next = i.wrapping_add(1),
            i = i,
            ty = f.ty,
        ));
    }
    consts
}

/// Emits one `ToBytes::write` call per field, into `__dst[__OFF_i..__OFF_{i+1}]`
/// — shared by every derive that generates a `ToBytes` impl.
pub(crate) fn write_body(fields: &[FieldShape]) -> String {
    let mut body = String::new();
    for (i, f) in fields.iter().enumerate() {
        body.push_str(&format!(
            "let _ = ::rshooks::convert::ToBytes::write(&self.{field}, &mut __dst[__OFF_{i}..__OFF_{next}]);\n",
            field = f.name,
            i = i,
            next = i.wrapping_add(1),
        ));
    }
    body
}

/// Emits one `FromBytes::read` call per field, from `__src[__OFF_i..__OFF_{i+1}]`
/// — shared by every derive that generates a `FromBytes` impl.
pub(crate) fn read_body(fields: &[FieldShape]) -> String {
    let mut body = String::new();
    for (i, f) in fields.iter().enumerate() {
        body.push_str(&format!(
            "{field}: <{ty} as ::rshooks::convert::FromBytes>::read(&__src[__OFF_{i}..__OFF_{next}])?,\n",
            field = f.name,
            ty = f.ty,
            i = i,
            next = i.wrapping_add(1),
        ));
    }
    body
}

/// Generates the `ToBytes` impl block (`MAX_LEN` const + `write()`) shared
/// by every fixed-offset codegen path: `HookKey`, `HookData`, `ParamName`,
/// and `#[state_interface(..)]`'s generated value struct (`ParamValue` is
/// the one exception — see its own doc comment for why it has no `ToBytes`
/// at all). `body` is `write()`'s statements before the final `MAX_LEN`
/// return (an [`offset_consts`]/[`write_body`] pair, or, for
/// `state_interface`, per-field writes at macro-computed literal offsets).
/// `extra` is spliced in as further associated items (`HookData`'s
/// `with_bytes` override) — pass `""` for none.
pub(crate) fn to_bytes_impl(name: &str, max_len_expr: &str, body: &str, extra: &str) -> String {
    format!(
        "
#[automatically_derived]
impl ::rshooks::convert::ToBytes for {name} {{
    const MAX_LEN: usize = {max_len_expr};

    #[inline(always)]
    #[allow(clippy::indexing_slicing)] // fixed, compile-time field offsets; `__dst` was already proven to have exactly `MAX_LEN` bytes by the `get_mut(..MAX_LEN)` check\n\
    fn write(&self, buf: &mut [u8]) -> usize {{
        match buf.get_mut(..<Self as ::rshooks::convert::ToBytes>::MAX_LEN) {{
            ::core::option::Option::Some(__dst) => {{
                {body}
                <Self as ::rshooks::convert::ToBytes>::MAX_LEN
            }}
            ::core::option::Option::None => 0,
        }}
    }}
    {extra}
}}
"
    )
}

/// Generates the `FromBytes` impl block (`read()`), shared the same way as
/// [`to_bytes_impl`]. `len_expr` is the buffer length to slice off —
/// `<Self as ::rshooks::convert::ToBytes>::MAX_LEN` when `Self` has a
/// `ToBytes` impl, or an inline sum (parenthesized) when it does not (see
/// [`crate::param_value`]'s doc comment). `body` is `read()`'s statements
/// before the final struct literal (an [`offset_consts`] chain, or empty);
/// `fields` are that struct literal's field initializers (a [`read_body`],
/// or `state_interface`'s own per-field reads). `extra` is spliced in as
/// further associated items ([`crate::hook_data`]'s `with_read_buf`
/// override) — pass `""` for none.
pub(crate) fn from_bytes_impl(
    name: &str,
    len_expr: &str,
    body: &str,
    fields: &str,
    extra: &str,
) -> String {
    format!(
        "
#[automatically_derived]
impl ::rshooks::convert::FromBytes for {name} {{
    #[inline(always)]
    #[allow(clippy::indexing_slicing)] // same fixed compile-time offsets as the `ToBytes::write` impl above\n\
    fn read(buf: &[u8]) -> ::rshooks::error::Result<Self> {{
        let __src = buf.get(..{len_expr})
            .ok_or(::rshooks::error::HookError::TooSmall)?;
        {body}
        ::core::result::Result::Ok(Self {{
            {fields}
        }})
    }}
    {extra}
}}
"
    )
}

/// Generates the `FixedRead` impl block (`read_exact()`), shared the same
/// way as [`to_bytes_impl`]/[`from_bytes_impl`]. `len_expr` is used both as
/// the scratch buffer's array length and as the read-length check — see
/// [`from_bytes_impl`] for what it is in each caller.
pub(crate) fn fixed_read_impl(name: &str, len_expr: &str) -> String {
    format!(
        "
#[automatically_derived]
impl ::rshooks::convert::FixedRead for {name} {{
    #[inline(always)]
    fn read_exact(
        read: impl FnOnce(&mut [u8]) -> ::rshooks::error::Result<usize>,
    ) -> ::rshooks::error::Result<Self> {{
        let mut __buf = [0u8; {len_expr}];
        let __written = read(&mut __buf)?;
        if __written == {len_expr} {{
            <Self as ::rshooks::convert::FromBytes>::read(&__buf)
        }} else {{
            ::core::result::Result::Err(::rshooks::error::HookError::TooSmall)
        }}
    }}
}}
"
    )
}

/// Rewrites `src`'s hardcoded `::rshooks::` paths for the invoking crate
/// (see [`crate::krate::rewrite`]) and parses it into the derive's final
/// `TokenStream`, or a `compile_error!` at `name_span` naming `derive_name`
/// if the generated source itself fails to parse — the shared tail of
/// every fixed-offset derive's `generate` function.
pub(crate) fn finish(src: String, name_span: Span, derive_name: &str) -> TokenStream {
    let src = crate::krate::rewrite(src);
    match src.parse::<TokenStream>() {
        Ok(ts) => ts,
        Err(_) => err(
            name_span,
            &format!("rshooks-macros: internal {derive_name} codegen failed to parse"),
        ),
    }
}

/// One `name: Type` field, as captured from the input tokens.
pub struct FieldShape {
    /// The field's name, verbatim.
    pub name: String,
    /// The field's type, reconstructed as source text (see
    /// [`tokens_to_string`]) — never type-checked by this macro itself; a
    /// type that doesn't implement the required trait(s) surfaces as an
    /// ordinary rustc trait-bound error against the generated code.
    pub ty: String,
}

/// A named-field struct's shape, as captured from the input tokens.
pub struct StructShape {
    /// The struct's name, verbatim.
    pub name: String,
    /// Span of the struct's name, used to anchor struct-level errors (e.g.
    /// "must have at least one field").
    pub name_span: Span,
    /// Fields in declaration order — the order every generated offset (and,
    /// for `HookData`, the layout doc table and the struct literal in
    /// `FromBytes::read`) follows.
    pub fields: Vec<FieldShape>,
}

/// Advances past any leading `#[...]` attributes (including doc comments,
/// which the compiler already desugars to `#[doc = "..."]` by the time this
/// macro sees them). Input to a derive macro is always a syntactically
/// valid item (rustc parses it before invoking the derive), so this never
/// needs to report an error — an attribute here is always exactly `#`
/// followed by a bracketed group.
pub(crate) fn skip_attrs(iter: &mut Peekable<impl Iterator<Item = TokenTree>>) {
    loop {
        match iter.peek() {
            Some(TokenTree::Punct(p)) if p.as_char() == '#' => {
                iter.next();
                iter.next();
            }
            _ => break,
        }
    }
}

/// Advances past an optional leading `pub`/`pub(...)` visibility.
pub(crate) fn skip_vis(iter: &mut Peekable<impl Iterator<Item = TokenTree>>) {
    if let Some(TokenTree::Ident(id)) = iter.peek()
        && id.to_string() == "pub"
    {
        iter.next();
        if let Some(TokenTree::Group(g)) = iter.peek()
            && g.delimiter() == Delimiter::Parenthesis
        {
            iter.next();
        }
    }
}

/// Parses the derive input into a [`StructShape`], or a `compile_error!`
/// `TokenStream` describing the first shape violation found. `derive_name`
/// (e.g. `"HookData"`, `"ParamName"`) names the calling derive in every
/// generated error message.
pub fn parse_struct(input: TokenStream, derive_name: &str) -> Result<StructShape, TokenStream> {
    let mut iter = input.into_iter().peekable();

    skip_attrs(&mut iter);
    skip_vis(&mut iter);

    match iter.next() {
        Some(TokenTree::Ident(id)) if id.to_string() == "struct" => {}
        Some(TokenTree::Ident(id)) if id.to_string() == "enum" => {
            return Err(err(
                id.span(),
                &format!("{derive_name} can only be derived for a struct, not an enum"),
            ));
        }
        Some(TokenTree::Ident(id)) if id.to_string() == "union" => {
            return Err(err(
                id.span(),
                &format!("{derive_name} can only be derived for a struct, not a union"),
            ));
        }
        other => {
            let span = other.map_or(Span::call_site(), |tt| tt.span());
            return Err(err(span, &format!("{derive_name}: expected a struct")));
        }
    }

    let name_id = match iter.next() {
        Some(TokenTree::Ident(id)) => id,
        other => {
            let span = other.map_or(Span::call_site(), |tt| tt.span());
            return Err(err(span, &format!("{derive_name}: expected a struct name")));
        }
    };
    let name_span = name_id.span();
    let name = name_id.to_string();

    if let Some(TokenTree::Punct(p)) = iter.peek()
        && p.as_char() == '<'
    {
        return Err(err(
            p.span(),
            &format!("{derive_name} does not support generic structs"),
        ));
    }

    match iter.next() {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
            let fields = parse_fields(g.stream(), derive_name)?;
            if fields.is_empty() {
                return Err(err(
                    name_span,
                    &format!("{derive_name}: struct must have at least one field"),
                ));
            }
            Ok(StructShape {
                name,
                name_span,
                fields,
            })
        }
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => Err(err(
            g.span(),
            &format!("{derive_name} does not support tuple structs — use named fields"),
        )),
        Some(TokenTree::Punct(p)) if p.as_char() == ';' => Err(err(
            p.span(),
            &format!("{derive_name} does not support unit structs — it needs at least one field"),
        )),
        other => {
            let span = other.map_or(name_span, |tt| tt.span());
            Err(err(
                span,
                &format!("{derive_name}: expected a `{{ .. }}` field list"),
            ))
        }
    }
}

/// Parses a brace group's inner stream into an ordered list of fields.
/// Every field must be `name: Type` (attributes and a leading `pub`/
/// `pub(...)` are skipped, not validated); `Type` is captured as raw tokens
/// up to the next top-level comma, so a bracketed array type (`[u8; 20]`)
/// or a multi-segment path (`crate::types::AccountId`) both work — nothing
/// deeper than one field-list nesting level is inspected.
pub(crate) fn parse_fields(
    stream: TokenStream,
    derive_name: &str,
) -> Result<Vec<FieldShape>, TokenStream> {
    let mut iter = stream.into_iter().peekable();
    let mut fields = Vec::new();

    while iter.peek().is_some() {
        skip_attrs(&mut iter);
        skip_vis(&mut iter);

        if iter.peek().is_none() {
            break;
        }

        let name_id = match iter.next() {
            Some(TokenTree::Ident(id)) => id,
            Some(other) => {
                return Err(err(
                    other.span(),
                    &format!("{derive_name}: expected a field name"),
                ));
            }
            None => break,
        };
        let field_span = name_id.span();
        let field_name = name_id.to_string();

        match iter.next() {
            Some(TokenTree::Punct(p)) if p.as_char() == ':' => {}
            other => {
                let span = other.map_or(field_span, |tt| tt.span());
                return Err(err(
                    span,
                    &format!("{derive_name}: expected `:` after field name"),
                ));
            }
        }

        let mut ty_tokens: Vec<TokenTree> = Vec::new();
        loop {
            match iter.peek() {
                Some(TokenTree::Punct(p)) if p.as_char() == ',' => {
                    iter.next();
                    break;
                }
                Some(_) => {
                    if let Some(tt) = iter.next() {
                        ty_tokens.push(tt);
                    }
                }
                None => break,
            }
        }
        if ty_tokens.is_empty() {
            return Err(err(
                field_span,
                &format!("{derive_name}: expected a field type"),
            ));
        }

        fields.push(FieldShape {
            name: field_name,
            ty: tokens_to_string(&ty_tokens),
        });
    }

    Ok(fields)
}

/// Reconstructs a type's source text from its captured tokens.
/// `TokenStream::to_string()` already respects each `Punct`'s spacing,
/// so a multi-token compound like the `::` in
/// `crate::types::AccountId` round-trips as `::` (no space, required by
/// Rust's path grammar) rather than `: :` (a parse error in path
/// position).
pub fn tokens_to_string(tokens: &[TokenTree]) -> String {
    tokens.iter().cloned().collect::<TokenStream>().to_string()
}
