//! Procedural macros used by [`rshooks`](https://docs.rs/rshooks).
//!
//! The public API is re-exported from `rshooks`: entry-point attributes,
//! fixed-layout derives, state and parameter declarations, identifier
//! splicing, and compile-time account-ID decoding. This host-side crate
//! permits bounded arithmetic used by its parsers and encoders.
#![allow(clippy::arithmetic_side_effects)]

mod base58;
mod metadata;
mod sha256;

use proc_macro::{Delimiter, Group, Ident, Literal, Punct, Spacing, Span, TokenStream, TokenTree};

mod decl_pair;
mod hook_data;
mod hook_key;
mod hooks_impl;
mod hooks_shared;
mod hooks_struct;
mod param_name;
mod param_value;
mod shape;
mod xfl_literal;

/// Turns a plain `fn name() -> i64 { .. }` into the Hook host's required
/// `hook` export.
///
/// Expands to the original function (unchanged) plus:
///
/// ```ignore
/// #[unsafe(no_mangle)]
/// pub extern "C" fn hook(_reserved: u32) -> i64 {
///     name()
/// }
/// ```
///
/// # Requirements
///
/// The annotated item must be a plain (`async`/`unsafe`/`const`/`extern`
/// modifiers not allowed, no generics, no `where` clause) `fn` that takes no
/// arguments and returns `i64`; `#[hook]` itself takes no arguments. Any
/// violation is reported as a `compile_error!` pointing at the offending
/// token, not a panic.
///
/// # Examples
///
/// ```
/// use rshooks_macros::hook;
///
/// #[hook]
/// fn my_hook() -> i64 {
///     0
/// }
/// ```
///
/// Hook authors do not depend on `rshooks-macros` directly in practice — see
/// `rshooks::hook`, which re-exports this and is what hook crates
/// actually import.
#[proc_macro_attribute]
pub fn hook(attr: TokenStream, item: TokenStream) -> TokenStream {
    entry_point("hook", attr, item)
}

/// Like [`macro@hook`], but exports `cbak` instead of `hook` — for the
/// optional callback a Hook module can export, invoked when a transaction it
/// previously emitted settles. See [`macro@hook`] for the exact requirements
/// and generated shape (identical, save for the export name).
#[proc_macro_attribute]
pub fn cbak(attr: TokenStream, item: TokenStream) -> TokenStream {
    entry_point("cbak", attr, item)
}

/// Declares a multi-hook chain — the v0.2 replacement for the top-level
/// [`macro@hook`]/[`macro@cbak`]/`metadata!`/`hook_state!`/`hook_parameter!`/
/// `otxn_parameter!` combination (which stay available for now; see
/// `docs/MULTI_HOOK_STRUCT_DESIGN.md` for the full rationale and the v0.2
/// implementation contract for the normative grammar).
///
/// Applied to a struct, it declares the chain's shared state/parameter
/// schema (see [`hooks_struct`]); applied to that struct's inherent `impl`
/// block, it declares the chain's hook/cbak entries (see [`hooks_impl`]).
/// Anything else is rejected.
///
/// Hook authors use the re-exported `rshooks::hooks` rather than depending
/// on this proc-macro crate directly.
#[proc_macro_attribute]
pub fn hooks(attr: TokenStream, item: TokenStream) -> TokenStream {
    match dispatch_hooks_target(&item) {
        HooksTarget::Struct => hooks_struct::expand(attr, item),
        HooksTarget::Impl => hooks_impl::expand(attr, item),
        HooksTarget::Other => err(
            Span::call_site(),
            "#[hooks] must be applied to a struct or an inherent impl",
        ),
    }
}

enum HooksTarget {
    Struct,
    Impl,
    Other,
}

/// Peeks far enough into `item`'s tokens (past leading attributes and an
/// optional `pub`/`pub(..)` visibility — visibility never precedes `impl`)
/// to tell whether [`hooks`] should dispatch to [`hooks_struct::expand`] or
/// [`hooks_impl::expand`]. Full shape validation happens inside whichever
/// module is dispatched to; this only needs to disambiguate the two.
fn dispatch_hooks_target(item: &TokenStream) -> HooksTarget {
    let mut iter = item.clone().into_iter().peekable();
    loop {
        match iter.peek() {
            Some(TokenTree::Punct(p)) if p.as_char() == '#' => {
                iter.next();
                iter.next();
            }
            _ => break,
        }
    }
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
    match iter.peek() {
        Some(TokenTree::Ident(id)) if id.to_string() == "struct" => HooksTarget::Struct,
        Some(TokenTree::Ident(id)) if id.to_string() == "impl" => HooksTarget::Impl,
        _ => HooksTarget::Other,
    }
}

/// Embeds build-only Hook metadata in a dead WebAssembly export.
///
/// Hook authors use the re-exported [`rshooks::metadata!`] macro rather
/// than depending on this proc-macro crate directly. See that re-export for
/// the complete grammar and the JSON/cleaning contract.
#[proc_macro]
pub fn metadata(input: TokenStream) -> TokenStream {
    metadata::expand(input)
}

/// Derives `rshooks::convert::ToBytes` plus an explicit
/// `rshooks::state::StateKeyEncode` impl for a fixed-size, named-field
/// struct used as a **composite hook-state key** — see
/// `rshooks::HookKey`'s doc comment (the public-facing re-export hook
/// authors actually use) for the full writeup. Implemented in
/// [`hook_key`]; kept as a thin `#[proc_macro_derive]` entry point here,
/// mirroring [`hook`]/[`cbak`]'s split between the `#[proc_macro...]`
/// entry point and its implementation.
#[proc_macro_derive(HookKey)]
pub fn derive_hook_key(input: TokenStream) -> TokenStream {
    hook_key::derive(input)
}

/// Derives `rshooks::convert::ToBytes`/`FromBytes`/`FixedRead` for a
/// fixed-size, named-field struct used as a **hook-state value** — see
/// `rshooks::HookData`'s doc comment (the public-facing re-export hook
/// authors actually use) for the full writeup. Implemented in
/// [`hook_data`]; kept as a thin `#[proc_macro_derive]` entry point here,
/// mirroring [`hook`]/[`cbak`]'s split between the `#[proc_macro...]`
/// entry point and its implementation.
#[proc_macro_derive(HookData)]
pub fn derive_hook_data(input: TokenStream) -> TokenStream {
    hook_data::derive(input)
}

/// Derives `rshooks::convert::ToBytes` (only — no `FromBytes`/`FixedRead`)
/// for a fixed-size, named-field struct used as a **composite Hook API
/// parameter name** — see `rshooks::ParamName`'s doc comment (the
/// public-facing re-export hook authors actually use) for the full
/// writeup. Implemented in [`param_name`]; kept as a thin
/// `#[proc_macro_derive]` entry point here, mirroring [`macro@HookData`]'s
/// split between the `#[proc_macro...]` entry point and its implementation.
#[proc_macro_derive(ParamName)]
pub fn derive_param_name(input: TokenStream) -> TokenStream {
    param_name::derive(input)
}

/// Derives `rshooks::convert::FromBytes`/`FixedRead` (only — no
/// `ToBytes`) for a fixed-size, named-field struct used as a **Hook API
/// parameter value** — see `rshooks::ParamValue`'s doc comment (the
/// public-facing re-export hook authors actually use) for the full
/// writeup. Implemented in [`param_value`]; kept as a thin
/// `#[proc_macro_derive]` entry point here, mirroring [`macro@HookData`]'s
/// split between the `#[proc_macro...]` entry point and its implementation.
#[proc_macro_derive(ParamValue)]
pub fn derive_param_value(input: TokenStream) -> TokenStream {
    param_value::derive(input)
}

/// Declares a hook-state key/value pairing — see `rshooks::hook_state!`'s
/// doc comment (the public-facing re-export site) for the full grammar
/// staircase (Forms 1–4, the `existing` keyword form and the pairing form),
/// the entity every form declares and the accessors it carries, and worked
/// examples. Implemented in [`decl_pair`]; kept as a thin
/// `#[proc_macro]` entry point here, mirroring [`hook`]/[`cbak`]'s split
/// between the `#[proc_macro...]` entry point and its implementation.
#[proc_macro]
pub fn hook_state(input: TokenStream) -> TokenStream {
    decl_pair::expand(input, decl_pair::Role::State)
}

/// Declares a Hook API parameter name/value pairing, read via
/// `hook_param_typed` — see `rshooks::hook_parameter!`'s doc comment (the
/// public-facing re-export site) for the full grammar staircase and worked
/// examples. Implemented in [`decl_pair`]; kept as a thin `#[proc_macro]`
/// entry point here, mirroring [`hook`]/[`cbak`]'s split between the
/// `#[proc_macro...]` entry point and its implementation.
#[proc_macro]
pub fn hook_parameter(input: TokenStream) -> TokenStream {
    decl_pair::expand(input, decl_pair::Role::HookParam)
}

/// Identical grammar to [`macro@hook_parameter`], targeting
/// `otxn_param_typed` (a parameter attached to the *originating
/// transaction*) instead of `hook_param_typed` — see
/// `rshooks::otxn_parameter!`'s doc comment for the full writeup.
#[proc_macro]
pub fn otxn_parameter(input: TokenStream) -> TokenStream {
    decl_pair::expand(input, decl_pair::Role::OtxnParam)
}

/// Decodes a classic XRPL/Xahau r-address (base58check string literal,
/// e.g. `account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh")`) into an
/// `::rshooks::types::AccountId` literal, entirely at compile time (host
/// side, inside this macro — see [`base58::decode`]/[`sha256::sha256`]).
///
/// The full usage docs — worked examples, known-address pairs, and the
/// `compile_fail` cases — live at `rshooks::account_id`'s doc comment
/// (this crate's re-export point), since that's what Hook authors actually
/// depend on and read docs for.
///
/// Expects exactly one token: a string literal. Anything else (missing,
/// extra tokens, a non-string literal) or a string that fails to decode is
/// reported as a `compile_error!` at the offending token, never a panic.
#[proc_macro]
pub fn account_id(input: TokenStream) -> TokenStream {
    let mut iter = input.into_iter();

    let literal = match iter.next() {
        Some(TokenTree::Literal(lit)) => lit,
        Some(other) => {
            return err(
                other.span(),
                "account_id! expects a single string literal, e.g. account_id!(\"r...\")",
            );
        }
        None => {
            return err(
                Span::call_site(),
                "account_id! expects a single string literal, e.g. account_id!(\"r...\")",
            );
        }
    };

    if let Some(extra) = iter.next() {
        return err(
            extra.span(),
            "account_id! expects a single string literal, e.g. account_id!(\"r...\") \
             (unexpected extra tokens)",
        );
    }

    let span = literal.span();
    let address = match unquote_str(&literal.to_string()) {
        Some(s) => s,
        None => {
            return err(
                span,
                "account_id! expects a single string literal, e.g. account_id!(\"r...\")",
            );
        }
    };

    let bytes = match base58::decode(&address) {
        Ok(bytes) => bytes,
        Err(e) => return err(span, &e.message()),
    };

    let src = format!(
        "::rshooks::types::AccountId([{}])",
        bytes
            .iter()
            .map(|b| format!("0x{b:02X}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    src.parse::<TokenStream>().unwrap_or_else(|_| {
        err(
            span,
            "rshooks-macros: internal account_id! expansion failed",
        )
    })
}

/// Encodes a numeric literal (optionally preceded by a bare `-`) into a
/// Xahau XFL raw bit pattern at compile time, entirely via pure
/// integer/string arithmetic (see [`xfl_literal::encode`] -- never `f64`,
/// since exactness is the whole point).
///
/// The full usage docs -- bit layout, worked examples, and the
/// `compile_fail` cases -- live at `rshooks::XFL`'s doc comment (this
/// crate's re-export point), since that's what Hook authors actually
/// depend on and read docs for.
///
/// Input grammar: an optional leading `-` token, then exactly one numeric
/// literal token (integer or decimal, with an optional exponent,
/// underscores allowed as digit separators). String/char/byte literals,
/// hex/octal/binary integers, a type suffix, more than 16 significant
/// decimal digits, or a magnitude outside XFL's representable range are all
/// reported as a `compile_error!` at the macro invocation, never a panic.
#[allow(non_snake_case)]
#[proc_macro]
pub fn XFL(input: TokenStream) -> TokenStream {
    xfl_literal::expand(input)
}

/// Strips a `proc_macro::Literal`'s `to_string()` output down to a plain
/// Rust `String`, if it is a plain (unprefixed, unsuffixed) string literal —
/// i.e. exactly `"..."`, escape sequences included verbatim as written
/// (r-addresses never contain characters needing escaping, so no unescaping
/// is attempted; a `"..."` string with backslashes would round-trip through
/// [`base58::decode`] as literal backslash characters, which simply fail to
/// decode as base58 like any other invalid character). Returns `None` for
/// anything else (raw strings, byte strings, non-string literals, ...).
fn unquote_str(text: &str) -> Option<String> {
    let inner = text.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner.to_string())
}

/// Shared implementation for [`hook`] and [`cbak`]: validates the annotated
/// item's shape, then appends a generated `extern "C"` export named
/// `export_name` that calls it.
fn entry_point(export_name: &str, attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return err(
            Span::call_site(),
            &format!("#[{export_name}] takes no arguments"),
        );
    }

    let tail = match extract_fn_name(item.clone(), export_name) {
        Ok(name) => match build_wrapper(export_name, &name) {
            Ok(wrapper) => wrapper,
            Err(e) => e,
        },
        Err(e) => e,
    };

    let mut out = item;
    out.extend(tail);
    out
}

/// Walks `item`'s tokens far enough to confirm it is a plain, no-argument,
/// `i64`-returning `fn`, and returns its name.
///
/// Deliberately tolerant of leading attributes (including doc comments) and
/// a leading `pub`/`pub(...)` visibility on the item, since those are
/// legal (if unusual) on a hook entry point and don't affect the generated
/// wrapper. Everything else about the shape is required exactly, since the
/// generated wrapper's `name()` call assumes it.
fn extract_fn_name(item: TokenStream, export_name: &str) -> Result<Ident, TokenStream> {
    let mut iter = item.into_iter().peekable();

    // Leading attributes: `#` followed by a `[...]` group, repeated.
    loop {
        match iter.peek() {
            Some(TokenTree::Punct(p)) if p.as_char() == '#' => {
                iter.next();
                match iter.next() {
                    Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket => {}
                    Some(other) => {
                        return Err(err(other.span(), "malformed attribute before `fn`"));
                    }
                    None => return Err(err(Span::call_site(), "malformed attribute before `fn`")),
                }
            }
            _ => break,
        }
    }

    // Optional visibility: `pub` or `pub(...)`.
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

    // `fn` keyword: no `async`/`unsafe`/`const`/`extern` modifiers allowed.
    match iter.next() {
        Some(TokenTree::Ident(id)) if id.to_string() == "fn" => {}
        Some(other) => {
            return Err(err(
                other.span(),
                &format!(
                    "#[{export_name}] can only be applied to a plain `fn` item \
                     (no `async`/`unsafe`/`const`/`extern` modifiers)"
                ),
            ));
        }
        None => return Err(err(Span::call_site(), "expected a function")),
    }

    // Function name.
    let name = match iter.next() {
        Some(TokenTree::Ident(id)) => id,
        Some(other) => return Err(err(other.span(), "expected a function name")),
        None => return Err(err(Span::call_site(), "expected a function name")),
    };

    // No generics.
    if let Some(TokenTree::Punct(p)) = iter.peek() {
        if p.as_char() == '<' {
            return Err(err(
                p.span(),
                &format!("#[{export_name}] does not support generic functions"),
            ));
        }
    }

    // Argument list: must be present and empty.
    match iter.next() {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
            if !g.stream().is_empty() {
                return Err(err(
                    g.span(),
                    &format!("#[{export_name}] functions must take no arguments"),
                ));
            }
        }
        Some(other) => return Err(err(other.span(), "expected `()` after the function name")),
        None => return Err(err(name.span(), "expected `()` after the function name")),
    }

    // Return type: exactly `-> i64`.
    match iter.next() {
        Some(TokenTree::Punct(p)) if p.as_char() == '-' => {}
        Some(other) => {
            return Err(err(
                other.span(),
                &format!("#[{export_name}] functions must return `i64` (expected `-> i64`)"),
            ));
        }
        None => return Err(err(name.span(), "expected `-> i64`")),
    }
    match iter.next() {
        Some(TokenTree::Punct(p)) if p.as_char() == '>' => {}
        Some(other) => return Err(err(other.span(), "expected `-> i64`")),
        None => return Err(err(name.span(), "expected `-> i64`")),
    }
    match iter.next() {
        Some(TokenTree::Ident(id)) if id.to_string() == "i64" => {}
        Some(other) => {
            return Err(err(
                other.span(),
                &format!("#[{export_name}] functions must return `i64`"),
            ));
        }
        None => return Err(err(name.span(), "expected `i64` return type")),
    }

    // Body: a single `{ .. }` group, nothing after it (no `where` clause).
    match iter.next() {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {}
        Some(other) => return Err(err(other.span(), "expected a function body")),
        None => return Err(err(name.span(), "expected a function body")),
    }
    if let Some(extra) = iter.next() {
        return Err(err(
            extra.span(),
            &format!(
                "#[{export_name}]: unexpected tokens after the function body \
                 (`where` clauses are not supported)"
            ),
        ));
    }

    Ok(name)
}

/// Builds the `extern "C"` export calling `target_name`, named
/// `export_name`.
fn build_wrapper(export_name: &str, target_name: &Ident) -> Result<TokenStream, TokenStream> {
    let src = format!(
        "#[unsafe(no_mangle)] pub extern \"C\" fn {export_name}(_reserved: u32) -> i64 {{ {target_name}() }}"
    );
    src.parse::<TokenStream>().map_err(|_| {
        err(
            Span::call_site(),
            "rshooks-macros: internal wrapper generation failed",
        )
    })
}

/// Builds a `compile_error!("msg");` item at `span`, so validation failures
/// surface as a normal, well-located compile error rather than a macro
/// panic. `pub(crate)` (not private) so [`hook_data`]'s parser can share it.
pub(crate) fn err(span: Span, msg: &str) -> TokenStream {
    let mut args = TokenStream::new();
    args.extend([TokenTree::Literal(Literal::string(msg))]);
    let group = Group::new(Delimiter::Parenthesis, args);

    let mut out = TokenStream::new();
    out.extend([
        TokenTree::Ident(Ident::new("compile_error", span)),
        TokenTree::Punct(Punct::new('!', Spacing::Alone)),
        TokenTree::Group(group),
        TokenTree::Punct(Punct::new(';', Spacing::Alone)),
    ]);
    out
}

/// Identifier-concatenation macro backing `rshooks::txn_template!`'s
/// `set_<field>` setter names on stable Rust (replaces the nightly
/// `${concat(set_, $field)}` metavariable expression).
///
/// Scans its input for bracket groups shaped like `[< tok tok .. >]` (first
/// inner token `<`, last inner token `>`, both plain `Punct`s) and replaces
/// each with a single new identifier formed by concatenating the string
/// form of every `Ident` token strictly between them. Recurses into every
/// other group unchanged, so this can wrap an arbitrarily large token
/// stream (an entire `impl` block, in `txn_template!`'s case) and only the
/// marked splice points are touched.
///
/// Only ever invoked internally, from `txn_template!`'s own expansion
/// (`$crate::__paste! { .. }`) — not part of the public API.
#[doc(hidden)]
#[proc_macro]
pub fn paste(input: TokenStream) -> TokenStream {
    rewrite_stream(input)
}

/// Applies [`rewrite_tree`] to every token in `input`.
fn rewrite_stream(input: TokenStream) -> TokenStream {
    input.into_iter().map(rewrite_tree).collect()
}

/// Rewrites a single token: a `[< .. >]` splice marker becomes the
/// concatenated identifier; any other group is recursed into (with its
/// delimiter and span preserved); anything else passes through unchanged.
fn rewrite_tree(tt: TokenTree) -> TokenTree {
    match tt {
        TokenTree::Group(group) => {
            if group.delimiter() == Delimiter::Bracket {
                if let Some(ident) = try_concat_marker(group.stream()) {
                    return TokenTree::Ident(ident);
                }
            }
            let mut rewritten = Group::new(group.delimiter(), rewrite_stream(group.stream()));
            rewritten.set_span(group.span());
            TokenTree::Group(rewritten)
        }
        other => other,
    }
}

/// If `stream` is shaped exactly like a `< ident ident .. >` splice marker
/// (at least one `Ident` strictly between a leading and trailing `Punct`
/// token spelled `<`/`>`), returns the concatenated identifier. Returns
/// `None` for anything else (including a marker whose interior contains a
/// non-`Ident` token) — such a group is left as ordinary bracketed tokens,
/// which is never valid `rshooks` usage but is not this macro's problem
/// to diagnose.
///
/// Concatenating only `Ident` tokens (never arbitrary token text) is what
/// guarantees the result is itself always a valid identifier: every `Ident`
/// token's text already satisfies Rust's identifier grammar, and
/// concatenating any number of valid identifiers end-to-end yields another
/// valid identifier (an identifier's continuation characters are a superset
/// of its allowed starting characters). So `Ident::new` below can never
/// panic on the text this function builds.
fn try_concat_marker(stream: TokenStream) -> Option<Ident> {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    if tokens.len() < 3 {
        return None;
    }
    let first = tokens.first()?;
    let last = tokens.last()?;
    if !is_punct(first, '<') || !is_punct(last, '>') {
        return None;
    }
    let middle = tokens.get(1..tokens.len().saturating_sub(1))?;

    let mut text = String::new();
    for t in middle {
        match t {
            TokenTree::Ident(id) => text.push_str(&id.to_string()),
            _ => return None,
        }
    }
    if text.is_empty() {
        return None;
    }
    Some(Ident::new(&text, Span::call_site()))
}

/// Whether `tt` is a bare `Punct` token spelled `ch`.
fn is_punct(tt: &TokenTree, ch: char) -> bool {
    matches!(tt, TokenTree::Punct(p) if p.as_char() == ch)
}
