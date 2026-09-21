//! Backs `rshooks::txn_template!`'s positional numbering of array
//! elements — see [`expand`].

use proc_macro::{Literal, Punct, Spacing, TokenStream, TokenTree};

use crate::hooks_shared::{is_punct, map_tokens};

/// `input` is `[ <element tokens> ] <rest tokens>`: a bracketed,
/// comma-separated element list followed by arbitrary trailing tokens.
///
/// Splits the bracket group's contents on top-level commas (a comma
/// nested inside some element's own `{ .. }`/`( .. )`/`[ .. ]` body is
/// already hidden inside that element's own `Group` token, so a plain
/// top-level split is enough; a trailing comma yields no empty element
/// after it) and prepends each element's zero-based position, `<N>:`, in
/// front of it. An element already spelled `<tt> : ..` is a compile
/// error — array elements are positional only, an explicit name is never
/// accepted.
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
/// element, per [`expand`]'s doc comment. Reassembles the result as
/// `0: elem0 , 1: elem1 , .. ,` (every element, including the last,
/// followed by a comma — harmless alongside the `$(, $($rest:tt)*)?`
/// shape every consumer already expects).
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
        if element.get(1).is_some_and(|tt| is_punct(tt, ':')) {
            // Checked non-empty above.
            #[allow(clippy::indexing_slicing)]
            let name = &element[0];
            return Err(crate::err(
                name.span(),
                &format!(
                    "txn_template!: array elements are positional and take no name -- remove `{name}:`"
                ),
            ));
        }
        out.extend([
            TokenTree::Literal(Literal::usize_unsuffixed(index)),
            TokenTree::Punct(Punct::new(':', Spacing::Alone)),
        ]);
        out.extend(element);
        out.extend([TokenTree::Punct(Punct::new(',', Spacing::Alone))]);
        index = index.wrapping_add(1);
    }
    Ok(out)
}

/// Recursively scans `input` for every `@ ELEMS` two-token marker (an
/// `@` `Punct` immediately followed by an `Ident` spelled `ELEMS`,
/// anywhere, including inside a nested group) and replaces each with
/// `replacement`'s tokens spliced in directly (not wrapped in a group).
/// Every other token — including any group, rebuilt with its own
/// delimiter and span preserved — passes through unchanged.
fn splice_marker(input: TokenStream, replacement: &TokenStream) -> TokenStream {
    map_tokens(input, &|toks| {
        let is_marker = matches!(toks.first(), Some(tt) if is_punct(tt, '@'))
            && matches!(toks.get(1), Some(TokenTree::Ident(id)) if id.to_string() == "ELEMS");
        is_marker.then(|| (replacement.clone(), 2))
    })
}
