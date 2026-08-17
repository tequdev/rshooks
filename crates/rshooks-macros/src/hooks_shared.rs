//! Small helpers shared by [`crate::hooks_struct`] and [`crate::hooks_impl`]
//! — the two modules implementing the `#[hooks]` struct/impl attribute
//! macro pair (see `docs/MULTI_HOOK_STRUCT_DESIGN.md` and the v0.2
//! implementation contract).
//!
//! Kept separate from [`crate::shape`] (which backs the *derive* macros'
//! named-field-struct grammar) because `#[hooks]` parses a materially
//! different surface: attribute *argument* lists (`key = value, flag, ..`)
//! and angle-bracket generics (`State<V>`), neither of which the derives
//! ever need to recognize.

use proc_macro::{Span, TokenStream, TokenTree};

use crate::err;

/// Whether `tt` is a bare `Punct` token spelled `ch`.
pub(crate) fn is_punct(tt: &TokenTree, ch: char) -> bool {
    matches!(tt, TokenTree::Punct(p) if p.as_char() == ch)
}

/// One parsed `key` or `key = value` entry from an inert attribute's
/// argument list (`#[state(key = expr)]`, `#[hook(0, on = all)]`, ..).
pub(crate) struct AttrEntry {
    pub(crate) key: String,
    pub(crate) key_span: Span,
    /// `None` for a bare flag (`required`); `Some` (possibly empty, which is
    /// itself rejected by the caller) for `key = <tokens..>`.
    pub(crate) value: Option<Vec<TokenTree>>,
}

/// Parses a flat, comma-separated `key = value` / bare-`key` list — the
/// shared shape behind every field attribute (`#[state(..)]`,
/// `#[hook_param(..)]`, `#[otxn_param(..)]`) and every entry attribute's
/// *named* arguments (`#[hook(0, name = "..", on = all)]`, the leading
/// positional index having already been consumed by the caller).
///
/// Does not interpret keys or values at all — duplicate detection, key
/// whitelisting and value-shape validation are the caller's job, since each
/// call site's rules differ.
pub(crate) fn parse_attr_entries(
    tokens: &[TokenTree],
    mac: &str,
) -> Result<Vec<AttrEntry>, TokenStream> {
    let mut entries = Vec::new();
    let mut i = 0usize;
    while i < tokens.len() {
        let key_id = match tokens.get(i) {
            Some(TokenTree::Ident(id)) => id.clone(),
            Some(other) => {
                return Err(err(
                    other.span(),
                    &format!("{mac}: expected an attribute key here"),
                ));
            }
            None => break,
        };
        i = i.wrapping_add(1);

        let value = if matches!(tokens.get(i), Some(tt) if is_punct(tt, '=')) {
            i = i.wrapping_add(1);
            let start = i;
            while i < tokens.len() && !matches!(tokens.get(i), Some(tt) if is_punct(tt, ',')) {
                i = i.wrapping_add(1);
            }
            if i == start {
                return Err(err(
                    key_id.span(),
                    &format!("{mac}: expected a value after `{key_id} =`"),
                ));
            }
            Some(tokens.get(start..i).unwrap_or_default().to_vec())
        } else {
            None
        };

        entries.push(AttrEntry {
            key: key_id.to_string(),
            key_span: key_id.span(),
            value,
        });

        match tokens.get(i) {
            Some(tt) if is_punct(tt, ',') => {
                i = i.wrapping_add(1);
            }
            Some(other) => {
                return Err(err(
                    other.span(),
                    &format!("{mac}: expected `,` between attribute entries"),
                ));
            }
            None => {}
        }
    }
    Ok(entries)
}

/// Converts a `snake_case` (or already-`UpperCamelCase`) identifier text
/// into `UpperCamelCase`, for building the field-marker type name
/// `__RshooksSpec<Struct><Field>` — see `docs/MULTI_HOOK_STRUCT_DESIGN.md`
/// §5.4 and contract §B1 item 1.
///
/// Splits on `_`, capitalizes the first ASCII letter of every non-empty
/// segment and leaves the rest as written (never lower-cases an
/// already-uppercase run), then concatenates with no separator.
pub(crate) fn to_upper_camel(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for segment in name.split('_') {
        let mut chars = segment.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// Converts an `UpperCamelCase` (or arbitrary) identifier text to
/// `snake_case`, for building a per-struct helper function name
/// (`__rshooks_assert_chain_impl_<snake struct name>`) that cannot collide
/// with another crate's — used only for readability, since the fixed-name
/// `#[no_mangle]` link marker (contract §B1 item 5) is what actually
/// enforces "one chain struct per crate".
pub(crate) fn to_snake_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len().wrapping_add(4));
    for (i, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Consumes a balanced `< .. >` generic-argument list starting at
/// `tokens[start]` (which must already be checked to be a `<` `Punct`).
///
/// Angle brackets never arrive as a [`proc_macro::Group`] — they are plain
/// `Punct` tokens — so nesting depth has to be tracked by hand. Returns the
/// inner tokens (exclusive of the outer `<`/`>`) and the index just past the
/// closing `>`, or `None` if the input ends before the brackets balance.
pub(crate) fn parse_balanced_angle(
    tokens: &[TokenTree],
    start: usize,
) -> Option<(Vec<TokenTree>, usize)> {
    debug_assert!(matches!(tokens.get(start), Some(tt) if is_punct(tt, '<')));
    let mut depth = 0i32;
    let mut i = start;
    loop {
        let tt = tokens.get(i)?;
        if is_punct(tt, '<') {
            depth = depth.wrapping_add(1);
        } else if is_punct(tt, '>') {
            depth = depth.wrapping_sub(1);
            if depth == 0 {
                let inner = tokens.get(start.wrapping_add(1)..i)?.to_vec();
                return Some((inner, i.wrapping_add(1)));
            }
        }
        i = i.wrapping_add(1);
    }
}

/// Parses a single string-literal token run — as captured by
/// [`AttrEntry::value`] — via `syn`, matching the existing `metadata!`
/// convention ([`crate::metadata`]). `tokens: None` means the key had no
/// `= value` at all (a bare flag where a value was required).
pub(crate) fn parse_string_value(
    tokens: Option<&[TokenTree]>,
    span: Span,
    mac: &str,
    field: &str,
) -> Result<String, TokenStream> {
    let literal = match tokens {
        Some([TokenTree::Literal(lit)]) => lit.clone(),
        _ => {
            return Err(err(
                span,
                &format!("{mac}: `{field}` must be a single string literal"),
            ));
        }
    };
    let lit_span = literal.span();
    let mut stream = TokenStream::new();
    stream.extend([TokenTree::Literal(literal)]);
    syn::parse::<syn::LitStr>(stream)
        .map(|lit| lit.value())
        .map_err(|_| {
            err(
                lit_span,
                &format!("{mac}: `{field}` must be a string literal"),
            )
        })
}

/// Splits a token slice on top-level commas — i.e. commas not nested inside
/// a [`proc_macro::Group`] or a manually-tracked `< .. >` pair. Used to
/// split a generic-argument list (`V,` / `V`) and, in `hooks_impl`, a
/// bracketed transaction-type list.
///
/// An entirely empty trailing segment (from a trailing comma) is dropped —
/// callers that care about a trailing comma's *presence* check that before
/// calling this; callers that only want the non-empty argument list (the
/// common case) get exactly that.
pub(crate) fn split_top_level_commas(tokens: &[TokenTree]) -> Vec<Vec<TokenTree>> {
    let mut out = Vec::new();
    let mut current: Vec<TokenTree> = Vec::new();
    let mut depth = 0i32;
    for tt in tokens {
        if is_punct(tt, '<') {
            depth = depth.wrapping_add(1);
        } else if is_punct(tt, '>') {
            depth = depth.wrapping_sub(1);
        }
        if depth == 0 && is_punct(tt, ',') {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(tt.clone());
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}
