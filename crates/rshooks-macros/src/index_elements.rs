//! Backs `rshooks::txn_template!`'s automatic positional numbering of
//! named-array elements — see [`expand`].

use proc_macro::{Group, Literal, Punct, Spacing, TokenStream, TokenTree};

use crate::hooks_shared::is_punct;

/// `input` is `[ <element tokens> ] <rest tokens>`: a bracketed,
/// comma-separated element list followed by arbitrary trailing tokens.
///
/// Splits the bracket group's contents on top-level commas (a comma
/// nested inside some element's own `{ .. }`/`( .. )`/`[ .. ]` body is
/// already hidden inside that element's own `Group` token, so a plain
/// top-level split is enough; a trailing comma yields no empty element
/// after it). Each element whose own first two tokens are not already
/// `<tt> :` (an explicit name) gets its zero-based position spliced in
/// front as `<N> :` — `optional sfX { .. }` starts `optional sfX ..`
/// (`optional` then an `Ident`, not a `Punct`), so it's numbered like any
/// other unnamed element; `name: sfX { .. }` and `name: optional sfX
/// { .. }` already start `name :`, so they keep their own name and don't
/// count as unnumbered.
///
/// Then walks `<rest tokens>` (recursing into every group, since the
/// numbered list is usually destined for a `fields = [ .. ]` several
/// groups deep) and splices the numbered, comma-terminated element list
/// in place of every `@ ELEMS` two-token marker found, verbatim — every
/// other token passes through unchanged.
pub fn expand(input: TokenStream) -> TokenStream {
    let mut iter = input.into_iter();
    let Some(TokenTree::Group(elements)) = iter.next() else {
        return crate::err(
            proc_macro::Span::call_site(),
            "__txn_template_index_elements!: expected a bracketed element list as the first token",
        );
    };
    let rest: TokenStream = iter.collect();

    let numbered = match number_elements(elements.stream()) {
        Ok(numbered) => numbered,
        Err(error) => return error,
    };
    splice_marker(rest, &numbered)
}

/// Splits `input` on top-level commas and prepends `<N>:` to every
/// element not already spelled `<tt> : ..`, per [`expand`]'s doc comment.
/// Reassembles the result as `elem0 , elem1 , .. ,` (every element,
/// including the last, followed by a comma — harmless alongside the
/// `$(, $($rest:tt)*)?` shape every consumer already expects).
fn number_elements(input: TokenStream) -> Result<TokenStream, TokenStream> {
    let tokens: Vec<TokenTree> = input.into_iter().collect();

    let mut elements: Vec<Vec<TokenTree>> = Vec::new();
    let mut current: Vec<TokenTree> = Vec::new();
    for tt in tokens {
        if is_punct(&tt, ',') {
            elements.push(std::mem::take(&mut current));
        } else {
            current.push(tt);
        }
    }
    if !current.is_empty() {
        elements.push(current);
    }

    let mut out = TokenStream::new();
    let mut index = 0usize;
    for element in elements {
        // A stray double comma (or an all-comma list) yields an empty
        // run between two separators -- not a real element, skip it
        // rather than numbering nothing.
        if element.is_empty() {
            continue;
        }
        let already_named = element.get(1).is_some_and(|tt| is_punct(tt, ':'));
        if already_named {
            // An explicit name must be spelled the way the generated
            // method names can carry it: an identifier or a plain index.
            let name_ok = match element.first() {
                Some(TokenTree::Ident(_)) => true,
                Some(TokenTree::Literal(lit)) => is_plain_index(&lit.to_string()),
                _ => false,
            };
            if !name_ok {
                let span = element
                    .first()
                    .map_or_else(proc_macro::Span::call_site, TokenTree::span);
                return Err(crate::err(
                    span,
                    "txn_template!: an array element name must be an identifier or a plain index (`0`, `1`, ..)",
                ));
            }
        } else {
            out.extend([
                TokenTree::Literal(Literal::usize_unsuffixed(index)),
                TokenTree::Punct(Punct::new(':', Spacing::Alone)),
            ]);
        }
        out.extend(element);
        out.extend([TokenTree::Punct(Punct::new(',', Spacing::Alone))]);
        index = index.wrapping_add(1);
    }
    Ok(out)
}

/// `true` for a literal spelled as bare ASCII digits (no sign, no suffix),
/// the only literal shape a generated method name can carry.
fn is_plain_index(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// Recursively scans `input` for every `@ ELEMS` two-token marker (an
/// `@` `Punct` immediately followed by an `Ident` spelled `ELEMS`,
/// anywhere, including inside a nested group) and replaces each with
/// `replacement`'s tokens spliced in directly (not wrapped in a group).
/// Every other token — including any group, rebuilt with its own
/// delimiter and span preserved — passes through unchanged.
fn splice_marker(input: TokenStream, replacement: &TokenStream) -> TokenStream {
    let tokens: Vec<TokenTree> = input.into_iter().collect();
    let mut out = TokenStream::new();
    let mut i = 0;
    while let Some(tt) = tokens.get(i) {
        let is_marker = is_punct(tt, '@')
            && matches!(tokens.get(i.wrapping_add(1)), Some(TokenTree::Ident(id)) if id.to_string() == "ELEMS");
        if is_marker {
            out.extend(replacement.clone());
            i = i.wrapping_add(2);
            continue;
        }
        match tt {
            TokenTree::Group(group) => {
                let mut rewritten = Group::new(
                    group.delimiter(),
                    splice_marker(group.stream(), replacement),
                );
                rewritten.set_span(group.span());
                out.extend([TokenTree::Group(rewritten)]);
            }
            other => out.extend([other.clone()]),
        }
        i = i.wrapping_add(1);
    }
    out
}
