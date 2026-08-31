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

use crate::err;
use proc_macro::{Delimiter, Spacing, Span, TokenStream, TokenTree};
use std::iter::Peekable;

/// One `name: Type` field, as captured from the input tokens.
///
/// `Clone` because [`crate::decl_pair`] generates the identical per-field
/// codegen twice per struct-shaped invocation — once for the declared
/// key/name type, once for the entity that mirrors its fields.
#[derive(Clone)]
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
fn skip_attrs(iter: &mut Peekable<impl Iterator<Item = TokenTree>>) {
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
fn skip_vis(iter: &mut Peekable<impl Iterator<Item = TokenTree>>) {
    if let Some(TokenTree::Ident(id)) = iter.peek() {
        if id.to_string() == "pub" {
            iter.next();
            if let Some(TokenTree::Group(g)) = iter.peek() {
                if g.delimiter() == Delimiter::Parenthesis {
                    iter.next();
                }
            }
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
        Some(other) => {
            return Err(err(
                other.span(),
                &format!("{derive_name}: expected a struct"),
            ));
        }
        None => {
            return Err(err(
                Span::call_site(),
                &format!("{derive_name}: expected a struct"),
            ));
        }
    }

    let name_id = match iter.next() {
        Some(TokenTree::Ident(id)) => id,
        Some(other) => {
            return Err(err(
                other.span(),
                &format!("{derive_name}: expected a struct name"),
            ));
        }
        None => {
            return Err(err(
                Span::call_site(),
                &format!("{derive_name}: expected a struct name"),
            ));
        }
    };
    let name_span = name_id.span();
    let name = name_id.to_string();

    if let Some(TokenTree::Punct(p)) = iter.peek() {
        if p.as_char() == '<' {
            return Err(err(
                p.span(),
                &format!("{derive_name} does not support generic structs"),
            ));
        }
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
        Some(other) => Err(err(
            other.span(),
            &format!("{derive_name}: expected a `{{ .. }}` field list"),
        )),
        None => Err(err(
            name_span,
            &format!("{derive_name}: expected a `{{ .. }}` field list"),
        )),
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
            Some(other) => {
                return Err(err(
                    other.span(),
                    &format!("{derive_name}: expected `:` after field name"),
                ));
            }
            None => {
                return Err(err(
                    field_span,
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

/// Reconstructs a type's source text from its captured tokens, preserving
/// each `Punct`'s [`Spacing`] so a multi-token compound like the `::` in
/// `crate::types::AccountId` round-trips as `::` (no space, required by
/// Rust's path grammar) rather than `: :` (a parse error in path
/// position). A space is inserted before every other token boundary — safe
/// since it can only separate tokens that were already distinct, never
/// glue two identifiers/literals into one.
pub fn tokens_to_string(tokens: &[TokenTree]) -> String {
    let mut out = String::new();
    let mut prev_joint = false;
    for tt in tokens {
        if !out.is_empty() && !prev_joint {
            out.push(' ');
        }
        out.push_str(&tt.to_string());
        prev_joint = matches!(tt, TokenTree::Punct(p) if p.spacing() == Spacing::Joint);
    }
    out
}
