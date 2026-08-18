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

use proc_macro::{Spacing, Span, TokenStream, TokenTree};

use crate::err;

/// Whether `tt` is a bare `Punct` token spelled `ch`.
pub(crate) fn is_punct(tt: &TokenTree, ch: char) -> bool {
    matches!(tt, TokenTree::Punct(p) if p.as_char() == ch)
}

/// Whether `tt` is a `Punct` token spelled `ch` with [`Spacing::Joint`] —
/// i.e. immediately glued to the next token with no space, as in the `-`
/// of `->` or the first `>` of a `>>` run.
fn is_joint_punct(tt: Option<&TokenTree>, ch: char) -> bool {
    matches!(tt, Some(TokenTree::Punct(p)) if p.as_char() == ch && p.spacing() == Spacing::Joint)
}

/// Advances a `< .. >` angle-bracket nesting-depth tracker by the token(s)
/// at `tokens[i]`, for scanning a token run that may itself contain
/// generics (a `key = <expr>` value, a field's declared type, ...).
///
/// Angle brackets never arrive as a [`proc_macro::Group`] — they are plain
/// `Punct` tokens indistinguishable, at the token level, from the
/// less-than/greater-than comparison operators, a `->` return-type arrow,
/// or a `>>` shift-right operator. This helper resolves the two cases that
/// matter for the grammars this crate scans:
///
/// - A `->` arrow (`Punct('-', Joint)` immediately followed by
///   `Punct('>', _)`) is treated as one atomic non-bracket unit: neither
///   token touches `depth`, and the returned `consumed` is `2` so the
///   caller skips both at once. Without this, a value like
///   `default = |x: u32| -> u32 { x }` would misread the arrow's `>` as a
///   generic close.
/// - Every other `>` decrements `depth`, clamped at `0` (never negative).
///   A `>>` run's two `>` `Punct`s are, at the token level, indistinguishable
///   from two nested generic closes (`Vec<Vec<T>>`) — clamping means a
///   shift-right operator used at depth `0` (e.g. `default = 1u32 >> 2`)
///   leaves `depth` at `0` instead of going negative and desyncing every
///   comma check for the remainder of the scan, while a genuine `>>`
///   closing two open levels still decrements twice, back to `0`, exactly
///   as two separate `>` tokens would.
///
/// Returns `(consumed, new_depth)`.
pub(crate) fn step_angle_depth(tokens: &[TokenTree], i: usize, depth: i32) -> (usize, i32) {
    step_angle_depth_core(classify_angle_tok(tokens, i), depth)
}

/// One token's classification for [`step_angle_depth`]'s purposes,
/// independent of any live `proc_macro` type — kept separate purely so the
/// depth-stepping logic itself ([`step_angle_depth_core`]) can be unit
/// tested. `proc_macro::TokenTree`/`TokenStream::from_str`/`Span` all panic
/// outside an actual macro invocation (see `hooks_struct`'s
/// `ChainFieldJson` doc comment for the same convention followed
/// elsewhere in this crate).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AngleTok {
    /// `<`
    Lt,
    /// `>`
    Gt,
    /// A `-` `Punct` with [`Spacing::Joint`] immediately followed by a `>`
    /// token — the first half of a `->` arrow.
    ArrowStart,
    /// Anything else (including `,`, which [`step_angle_depth_core`] does
    /// not special-case — callers check `depth == 0` themselves before
    /// treating a comma as top-level).
    Other,
}

/// Classifies the token(s) at `tokens[i]` into an [`AngleTok`].
fn classify_angle_tok(tokens: &[TokenTree], i: usize) -> AngleTok {
    if is_joint_punct(tokens.get(i), '-')
        && matches!(tokens.get(i.wrapping_add(1)), Some(tt) if is_punct(tt, '>'))
    {
        return AngleTok::ArrowStart;
    }
    match tokens.get(i) {
        Some(tt) if is_punct(tt, '<') => AngleTok::Lt,
        Some(tt) if is_punct(tt, '>') => AngleTok::Gt,
        _ => AngleTok::Other,
    }
}

/// The pure depth-stepping core of [`step_angle_depth`] — see that
/// function's doc comment for the full rationale. Operates on a
/// pre-classified [`AngleTok`] instead of a live `TokenTree` so it is
/// testable without a `proc_macro` context. Returns `(consumed, new_depth)`.
pub(crate) fn step_angle_depth_core(kind: AngleTok, depth: i32) -> (usize, i32) {
    match kind {
        AngleTok::ArrowStart => (2, depth),
        AngleTok::Lt => (1, depth.wrapping_add(1)),
        AngleTok::Gt => (1, depth.wrapping_sub(1).max(0)),
        AngleTok::Other => (1, depth),
    }
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
            let mut depth = 0i32;
            while i < tokens.len()
                && !(depth == 0 && matches!(tokens.get(i), Some(tt) if is_punct(tt, ',')))
            {
                let (consumed, new_depth) = step_angle_depth(tokens, i, depth);
                depth = new_depth;
                i = i.wrapping_add(consumed);
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
        tokens.get(i)?;
        let (consumed, new_depth) = step_angle_depth(tokens, i, depth);
        if consumed == 2 {
            // A `->` arrow (e.g. inside `Box<dyn Fn() -> T>`) — an atomic
            // non-bracket unit, never a generic close.
            i = i.wrapping_add(2);
            continue;
        }
        let closed = new_depth < depth && new_depth == 0;
        depth = new_depth;
        if closed {
            let inner = tokens.get(start.wrapping_add(1)..i)?.to_vec();
            return Some((inner, i.wrapping_add(1)));
        }
        i = i.wrapping_add(1);
    }
}

/// Parses a single string-literal token run — as captured by
/// [`AttrEntry::value`] — via `syn`. `tokens: None` means the key had no
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

/// Parses a single byte-string-literal token run — as captured by
/// [`AttrEntry::value`] — via `syn::LitByteStr`, which accepts both the
/// plain `b"..."` form and the raw `br"..."`/`br#"..."#` forms uniformly
/// (unlike a manual `starts_with("b\"")` text check, which only recognizes
/// the plain form). Returns the original literal token, preserved verbatim
/// for codegen, alongside the *decoded* byte length (escapes resolved) for
/// the caller to validate.
pub(crate) fn parse_byte_string_value(
    tokens: Option<&[TokenTree]>,
    span: Span,
    mac: &str,
    field: &str,
) -> Result<(TokenTree, usize), TokenStream> {
    let (tt, lit) = match tokens {
        Some([tt @ TokenTree::Literal(lit)]) => (tt.clone(), lit.clone()),
        _ => {
            return Err(err(
                span,
                &format!("{mac}: `{field}` must be a single byte-string literal"),
            ));
        }
    };
    let lit_span = lit.span();
    let mut stream = TokenStream::new();
    stream.extend([TokenTree::Literal(lit)]);
    let decoded_len = syn::parse::<syn::LitByteStr>(stream)
        .map(|l| l.value().len())
        .map_err(|_| {
            err(
                lit_span,
                &format!(
                    "{mac}: `{field}` must be a byte-string literal, e.g. `b\"CFG\"` \
                     (raw byte-string literals like `br\"CFG\"` are also accepted)"
                ),
            )
        })?;
    Ok((tt, decoded_len))
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
    let mut i = 0usize;
    while i < tokens.len() {
        let (consumed, new_depth) = step_angle_depth(tokens, i, depth);
        if consumed == 2 {
            // A `->` arrow, consumed as one atomic non-bracket unit — push
            // both tokens through unsplit.
            if let Some(tt) = tokens.get(i) {
                current.push(tt.clone());
            }
            if let Some(tt) = tokens.get(i.wrapping_add(1)) {
                current.push(tt.clone());
            }
            depth = new_depth;
            i = i.wrapping_add(2);
            continue;
        }
        depth = new_depth;
        let tt = match tokens.get(i) {
            Some(tt) => tt,
            None => break,
        };
        if depth == 0 && is_punct(tt, ',') {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            i = i.wrapping_add(1);
            continue;
        }
        current.push(tt.clone());
        i = i.wrapping_add(1);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::indexing_slicing)] // tests are exempt, docs/DESIGN.md §8
    use super::*;

    // --- F4: `step_angle_depth_core` — generic-containing inputs ---
    //
    // `proc_macro::TokenTree` cannot be constructed outside a live macro
    // invocation (it panics — see `AngleTok`'s doc comment), so these tests
    // exercise the pure depth-stepping core directly, simulating the token
    // kind sequence a real `Pair<A, B>` / `State<Result<A, B>>` /
    // `|x| -> u32 { x }` input would produce. This is the logic both
    // `parse_attr_entries`'s value scan (hooks_shared.rs) and
    // `parse_named_fields`'s field-type scan (hooks_struct.rs) now share.

    /// Runs a full sequence of [`AngleTok`]s through [`step_angle_depth_core`]
    /// starting at depth `0`, returning the depth *after* each input token
    /// (an `ArrowStart` consumes 2 real tokens but only ever appears once in
    /// these fixtures, so per-`AngleTok`-element depth tracking is exact).
    fn run(kinds: &[AngleTok]) -> Vec<i32> {
        let mut depth = 0i32;
        let mut out = Vec::with_capacity(kinds.len());
        for &k in kinds {
            let (_, new_depth) = step_angle_depth_core(k, depth);
            depth = new_depth;
            out.push(depth);
        }
        out
    }

    #[test]
    fn generic_value_keeps_inner_comma_non_top_level() {
        // `default = Pair<A, B>` — the token run after `=` is:
        // Ident(Pair) `<` Ident(A) `,` Ident(B) `>`. A top-level-comma scan
        // must NOT stop at the inner `,` (index 3): depth is 1 there.
        use AngleTok::{Gt, Lt, Other};
        let depths = run(&[Other, Lt, Other, Other, Other, Gt]);
        assert_eq!(
            depths[3], 1,
            "comma nested inside `<..>` must not read as depth 0"
        );
        assert_eq!(
            *depths.last().expect("non-empty depths"),
            0,
            "depth returns to 0 after the close"
        );
    }

    #[test]
    fn nested_generic_field_type_closes_via_two_gt_tokens() {
        // `State<Result<A, B>>` — field-type scan sees `<` `Result` `<` `A`
        // `,` `B` `>` `>`. Two nested opens require two closes; the inner
        // comma (index 4) must stay non-top-level, and depth must reach 0
        // only after both `>` tokens.
        use AngleTok::{Gt, Lt, Other};
        let depths = run(&[Lt, Other, Lt, Other, Other, Other, Gt, Gt]);
        assert_eq!(depths[4], 2, "comma nested inside two open `<..>` levels");
        assert_eq!(depths[6], 1, "only one level closed after the first `>`");
        assert_eq!(depths[7], 0, "both levels closed after the second `>`");
    }

    #[test]
    fn arrow_start_consumes_two_tokens_without_touching_depth() {
        // `default = |x: u32| -> u32 { x }` — the `->` arrow's `-` and `>`
        // must never be read as a generic close.
        let (consumed, depth) = step_angle_depth_core(AngleTok::ArrowStart, 0);
        assert_eq!(consumed, 2);
        assert_eq!(depth, 0);

        // Same, mid-nesting (`Box<dyn Fn() -> T>`): the arrow must not
        // prematurely close the still-open `Box<..>` level.
        let (consumed, depth) = step_angle_depth_core(AngleTok::ArrowStart, 1);
        assert_eq!(consumed, 2);
        assert_eq!(depth, 1);
    }

    #[test]
    fn stray_shift_right_at_depth_zero_clamps_instead_of_going_negative() {
        // `default = 1u32 >> 2` at depth 0 — both `>` `Punct`s of the `>>`
        // run must leave depth at 0, not go negative (which would desync
        // every later top-level-comma check in the same scan).
        let (_, d1) = step_angle_depth_core(AngleTok::Gt, 0);
        assert_eq!(d1, 0);
        let (_, d2) = step_angle_depth_core(AngleTok::Gt, d1);
        assert_eq!(d2, 0);
    }

    #[test]
    fn genuine_double_close_still_returns_to_zero() {
        // `Vec<Vec<T>>` — two real opens, then a `>>` run closing both.
        use AngleTok::{Gt, Lt, Other};
        let depths = run(&[Lt, Other, Lt, Other, Gt, Gt]);
        assert_eq!(
            depths[3], 2,
            "still inside both open levels before any close"
        );
        assert_eq!(depths[4], 1);
        assert_eq!(depths[5], 0);
    }
}
