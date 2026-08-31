//! `#[hooks]` on an inherent impl — the entry-declaration half of the
//! multi-hook attribute macro pair. See `docs/MULTI_HOOK_STRUCT_DESIGN.md`
//! §5.1/§5.2/§5.3/§5.7/§6 and the v0.2 implementation contract §B2 for the
//! normative grammar and generated-output shape; this module implements it.
//!
//! Parses inert `#[hook(<index>, ..)]`/`#[cbak(<index>)]` attributes off
//! associated functions, cross-validates the whole block (duplicate/missing
//! indices, hook/cbak pairing, entry-fn shape), then emits:
//!
//! - the original `impl` block, re-emitted with only those two attributes
//!   stripped — every other user token (fn bodies, signatures, doc
//!   comments, helper items) is re-emitted from the exact same
//!   [`proc_macro::TokenTree`]s the parser walked, never restringified;
//! - a `mod __rshooks_entries` carrying the `--cfg rshooks_entry="i"`
//!   selected/discovery export-wrapper pairs (contract §B2), the entries
//!   carrier (dead wasm export encoding index/metadata JSON), and the
//!   struct/impl handshake back-half.
//!
//! # Why hand-rolled, not `syn`/`quote`
//!
//! Same rationale as [`crate::hooks_struct`] and every other macro in this
//! crate (see the crate doc comment in `lib.rs`).

use std::collections::BTreeSet;

use proc_macro::{Delimiter, Group, Ident, Punct, Spacing, Span, TokenStream, TokenTree};

use crate::hooks_shared::{
    AttrEntry, is_punct, parse_attr_entries, parse_balanced_angle, parse_string_value,
    split_top_level_commas,
};
use crate::shape::tokens_to_string;
use crate::{err, sha256};

/// Wasm export-name prefix for the entries carrier — contract §B2.
const ENTRIES_EXPORT_PREFIX: &str = "__rshooks_hooks_v2_";
/// Fixed chain-length domain (contract §B2, §D — `vendor/xahaud/Enum.h`
/// `maxHookChainLength()`), used both for range-checking a declared index
/// and for the fixed 10-value `not(any(rshooks_entry = "0", ..))` cfg list
/// every discovery-build wrapper carries regardless of how many entries this
/// particular crate declares.
const MAX_INDEX: u8 = 9;

/// HOOKS_SELF_RECEIVER_DESIGN.md §6.2 diagnostic for every rejected
/// self-receiver shape other than `&mut self`/`&'a mut self` — `self`,
/// `mut self`, `self: T`, `&'a self`. Shared by entry functions (which
/// require exactly `&self`) and non-attributed helpers (§3.3), which accept
/// either no receiver or bare `&self`.
const SELF_RECEIVER_MSG: &str = "use `&self` — hook entrypoints receive the chain declaration \
                                  by shared reference (it is zero-sized)";
/// HOOKS_SELF_RECEIVER_DESIGN.md §6.2 diagnostic for `&mut self` /
/// `&'a mut self`. Shared by entry functions and non-attributed helpers.
const MUT_SELF_RECEIVER_MSG: &str = "chain handles are zero-sized and immutable; ledger state \
                                      is accessed through the handles, not by mutating the \
                                      struct — use `&self`";
/// Diagnostic for an entry function (`#[hook]`/`#[cbak]`) with no receiver
/// at all — entries require exactly `&self`.
const ENTRY_MISSING_SELF_MSG: &str = "hook entry functions take `&self` — the chain \
                                       declaration is passed by shared reference (it is \
                                       zero-sized)";
/// HOOKS_SELF_RECEIVER_DESIGN.md §3.3 diagnostic for `#[cfg]`/`#[cfg_attr]`
/// on a `self` receiver — the receiver's class is decided once at
/// macro-expansion time, before `cfg` resolves, so a conditional shape
/// would diverge from what actually compiles (same rationale as the
/// entry-level cfg ban in [`parse_impl_body`]).
const RECEIVER_CFG_MSG: &str =
    "#[hooks]: `#[cfg]`/`#[cfg_attr]` are not allowed on a `self` receiver";

/// Entry point for `#[hooks]` applied to an `impl` item, dispatched from
/// [`crate::hooks`].
pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return err(
            Span::call_site(),
            "#[hooks] on an impl block takes no arguments",
        );
    }

    let parsed = match parse_impl_item(item) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let body = match parse_impl_body(&parsed.items) {
        Ok(v) => v,
        Err(e) => return e,
    };

    if let Err(e) = validate_cross(&body.hooks, &body.cbaks) {
        return e;
    }

    generate(&parsed, body.tokens, body.hooks, body.cbaks)
}

// ---------------------------------------------------------------------
// impl header parsing
// ---------------------------------------------------------------------

struct ParsedImpl {
    leading_attrs: Vec<TokenTree>,
    impl_kw: TokenTree,
    struct_name: Ident,
    items: Vec<TokenTree>,
}

/// Parses `impl <BareIdent> { .. }` — non-generic, no trait, bare
/// (unqualified) self type — per contract §B2's accepted `impl` shape.
fn parse_impl_item(item: TokenStream) -> Result<ParsedImpl, TokenStream> {
    let tokens: Vec<TokenTree> = item.into_iter().collect();
    let mut i = 0usize;

    let mut leading_attrs = Vec::new();
    while let Some(tt) = tokens.get(i) {
        if !is_punct(tt, '#') {
            break;
        }
        leading_attrs.push(tt.clone());
        match tokens.get(i.wrapping_add(1)) {
            Some(g @ TokenTree::Group(group)) if group.delimiter() == Delimiter::Bracket => {
                leading_attrs.push(g.clone());
            }
            _ => return Err(err(Span::call_site(), "malformed attribute before `impl`")),
        }
        i = i.wrapping_add(2);
    }

    let impl_kw = match tokens.get(i) {
        Some(tt @ TokenTree::Ident(id)) if id.to_string() == "impl" => tt.clone(),
        Some(other) => {
            return Err(err(
                other.span(),
                "#[hooks]: expected an `impl` block here (this attribute form only \
                 applies to a chain's entry impl)",
            ));
        }
        None => return Err(err(Span::call_site(), "#[hooks]: expected an `impl` block")),
    };
    i = i.wrapping_add(1);

    if let Some(tt) = tokens.get(i) {
        if is_punct(tt, '<') {
            return Err(err(
                tt.span(),
                "#[hooks]: the chain entry impl cannot be generic",
            ));
        }
    }

    let struct_name = match tokens.get(i) {
        Some(TokenTree::Ident(id)) => id.clone(),
        Some(other) => {
            return Err(err(
                other.span(),
                "#[hooks]: expected a bare struct name after `impl` (no path \
                 qualification, no trait impl)",
            ));
        }
        None => {
            return Err(err(
                Span::call_site(),
                "#[hooks]: expected a struct name after `impl`",
            ));
        }
    };
    i = i.wrapping_add(1);

    match tokens.get(i) {
        Some(tt)
            if is_punct(tt, ':')
                && matches!(tokens.get(i.wrapping_add(1)), Some(t) if is_punct(t, ':')) =>
        {
            return Err(err(
                tt.span(),
                "#[hooks]: the impl's self type must be a bare struct name \
                 (no path qualification)",
            ));
        }
        Some(tt) if is_punct(tt, '<') => {
            return Err(err(
                tt.span(),
                "#[hooks]: the chain entry impl cannot be generic",
            ));
        }
        Some(TokenTree::Ident(id)) if id.to_string() == "for" => {
            return Err(err(
                id.span(),
                "#[hooks]: a trait impl is not accepted here — only a bare inherent \
                 `impl StructName { .. }`",
            ));
        }
        _ => {}
    }

    let body_group = match tokens.get(i) {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => g.clone(),
        Some(other) => {
            return Err(err(
                other.span(),
                "#[hooks]: expected `{ .. }` after the impl's self type",
            ));
        }
        None => {
            return Err(err(
                struct_name.span(),
                "#[hooks]: expected `{ .. }` after the impl's self type",
            ));
        }
    };
    if let Some(extra) = tokens.get(i.wrapping_add(1)) {
        return Err(err(
            extra.span(),
            "#[hooks]: unexpected tokens after the impl block",
        ));
    }

    Ok(ParsedImpl {
        leading_attrs,
        impl_kw,
        struct_name,
        items: body_group.stream().into_iter().collect(),
    })
}

// ---------------------------------------------------------------------
// impl body: item splitting, inert-attribute parsing, entry-shape checks
// ---------------------------------------------------------------------

struct HookEntry {
    index: u8,
    index_span: Span,
    fn_name: String,
    attr: HookAttrData,
    /// The entry fn's own `-> Ty` tokens, span-carrying and unexamined —
    /// see [`build_entry_return_assertion`], which is the only consumer.
    return_tokens: Vec<TokenTree>,
    /// Fallback span for an implicit `-> ()` (`return_tokens` empty): the
    /// fn's own name, so the diagnostic still names the function.
    return_span: Span,
    /// Extra `ident: Type` arguments after `&self` — declared signature
    /// parameters (`docs/PARAM_SIGNATURE_DESIGN.md` §1), in declaration
    /// (= wire index) order. Empty for an ordinary no-arg entry.
    sig_args: Vec<SigArg>,
}

struct CbakEntry {
    index: u8,
    index_span: Span,
    fn_name: String,
    /// See [`HookEntry::return_tokens`].
    return_tokens: Vec<TokenTree>,
    /// See [`HookEntry::return_span`].
    return_span: Span,
}

struct HookAttrData {
    name: Option<(String, Span)>,
    on: OnSpec,
    can_emit: Option<Vec<(String, Span)>>,
    description: Option<String>,
}

enum OnSpec {
    Omitted,
    All,
    List(Vec<(String, Span)>),
    Directional(Vec<(String, Span)>, Vec<(String, Span)>),
}

/// Result of [`parse_impl_body`]: the re-emitted body tokens (attrs
/// stripped) plus the declared `#[hook]`/`#[cbak]` entries.
struct ParsedBody {
    tokens: Vec<TokenTree>,
    hooks: Vec<HookEntry>,
    cbaks: Vec<CbakEntry>,
}

// ---------------------------------------------------------------------
// Signature parameters — `docs/PARAM_SIGNATURE_DESIGN.md` §1: extra
// `ident: Type` arguments on a `#[hook(..)]` entry fn declare Hook
// Parameter Signature Interface parameters.
// ---------------------------------------------------------------------

/// The diagnostic listing every supported signature-parameter type token —
/// shared by [`classify_sig_type`]'s only call site so the list can't drift
/// out of sync with the match arms below it.
const SIG_TYPE_MSG: &str = "#[hooks]: unsupported signature parameter type — supported types are \
                             u8, u16, u32, u64, [u8; 16], [u8; 32], Hash, AccountId, [u8; 20], \
                             CurrencyCode, AmountBytes, IssueBytes, Blob<N> (write the type as a \
                             bare name — a path-qualified or aliased spelling is not resolved)";

/// The interface draft's display-name diagnostic — shared by every
/// rejection [`is_valid_sig_arg_name`] backs.
const SIG_NAME_MSG: &str = "#[hooks]: a signature parameter name must be 1..=16 ASCII bytes \
                             matching [A-Za-z][A-Za-z0-9]* (the Hook Parameter Signature \
                             Interface's display-name rule) — rename the argument";

/// Diagnostic for an extra argument on a `#[hook(..)]` fn when the
/// `unstable-param-sig-interface` feature is off — the interface's whole
/// surface (this module's [`parse_sig_args`] and downstream codegen) stays
/// compiled but unreachable in that configuration, so an extra argument is
/// rejected here instead. A `#[cbak(..)]` extra argument never reaches this
/// diagnostic: the callback-specific rejection above it is unconditional.
#[cfg(not(feature = "unstable-param-sig-interface"))]
const SIG_FEATURE_GATE_MSG: &str = "#[hooks]: extra arguments after `&self` declare Hook \
                                     Parameter Signature Interface parameters (draft spec) — \
                                     enable rshooks's `unstable-param-sig-interface` feature to \
                                     use them (only #[hook(..)] entry fns may declare them; the \
                                     interface may change while the spec is a draft)";

/// One parsed `ident: Type` signature-parameter argument (`docs/PARAM_SIGNATURE_DESIGN.md`
/// §1) — a `#[hook(..)]` entry fn's extra argument after `&self`. Wire
/// index is this entry's position in [`HookEntry::sig_args`], not stored
/// here.
struct SigArg {
    /// The argument's identifier, already validated against the
    /// interface's display-name charset/length rule — usable verbatim as
    /// both the declared name's bytes and the generated local's suffix.
    name: String,
    /// The argument's type, reconstructed as source text (see
    /// [`tokens_to_string`]) — spliced verbatim into the generated
    /// prologue so an aliased type is decoded, and const-asserted, exactly
    /// as written (never re-derived from `type_byte`).
    type_text: String,
    /// The `STI_*` type byte [`classify_sig_type`] derived from the type
    /// token's own shape — see `docs/PARAM_SIGNATURE_DESIGN.md` §1's type
    /// table.
    type_byte: u8,
}

/// Whether `name` matches the interface draft's display-name charset:
/// `[A-Za-z][A-Za-z0-9]*`, 1..=16 bytes — the macro-time twin of
/// [`crate::sig`]'s (rshooks crate) `is_valid_name` const fn, checked here
/// so a bad name is a diagnostic at the argument's own span rather than a
/// `const`-eval failure blamed on the generated prologue.
fn is_valid_sig_arg_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 16 {
        return false;
    }
    bytes.iter().enumerate().all(|(i, &b)| {
        if i == 0 {
            b.is_ascii_alphabetic()
        } else {
            b.is_ascii_alphanumeric()
        }
    })
}

/// Classifies a signature-parameter argument's type tokens into its
/// `STI_*` type byte (`docs/PARAM_SIGNATURE_DESIGN.md` §1's table),
/// matching on the token *shape* (a whitespace-normalized textual
/// comparison, plus a structural check for `Blob<N>` since angle brackets
/// never arrive as a single token — see [`crate::hooks_shared::AngleTok`]'s
/// doc comment). Does not resolve type aliases: an aliased type is caught
/// later by the monomorphized `const` assert the caller emits alongside
/// this byte (`docs/PARAM_SIGNATURE_DESIGN.md` §1).
fn classify_sig_type(tokens: &[TokenTree]) -> Option<u8> {
    if let [TokenTree::Ident(id), TokenTree::Punct(lt), rest @ ..] = tokens {
        if id.to_string() == "Blob" && lt.as_char() == '<' && rest.len() >= 2 {
            if let Some(TokenTree::Punct(gt)) = rest.last() {
                if gt.as_char() == '>' {
                    return Some(0x07); // STI_VL
                }
            }
        }
    }
    classify_sig_type_text(&tokens_to_string(tokens).replace(' ', ""))
}

/// The non-`Blob<N>` half of [`classify_sig_type`]'s table, over an already
/// whitespace-stripped flattened type string — `TokenTree`-free purely for
/// unit-testability, following this crate's standard "kind tag" boundary
/// pattern (see [`qualifier_token_kind`]'s doc comment).
fn classify_sig_type_text(flat: &str) -> Option<u8> {
    match flat {
        "u8" => Some(0x10),               // STI_UINT8
        "u16" => Some(0x01),              // STI_UINT16
        "u32" => Some(0x02),              // STI_UINT32
        "u64" => Some(0x03),              // STI_UINT64
        "[u8;16]" => Some(0x04),          // STI_UINT128
        "[u8;32]" | "Hash" => Some(0x05), // STI_UINT256
        "AmountBytes" => Some(0x06),      // STI_AMOUNT
        "AccountId" => Some(0x08),        // STI_ACCOUNT
        "[u8;20]" => Some(0x11),          // STI_UINT160
        "IssueBytes" => Some(0x18),       // STI_ISSUE
        "CurrencyCode" => Some(0x1A),     // STI_CURRENCY
        _ => None,
    }
}

/// Parses a `#[hook(..)]` entry fn's extra arguments (everything
/// [`scan_fn_item`] captured in [`ScannedFn::extra_args`] past the `&self`
/// receiver) into signature parameters, in declaration order. `entry_span`
/// anchors diagnostics that have no more specific token to point at (an
/// empty argument list past a trailing comma never reaches here — the
/// caller only calls this when `extra_args` is non-empty).
///
/// At most 16 arguments are accepted (`0x00..=0x0F` — the interface's
/// index range); a 17th is a macro-time error at its own first token's
/// span.
fn parse_sig_args(extra_args: &[TokenTree], entry_span: Span) -> Result<Vec<SigArg>, TokenStream> {
    // `extra_args` starts with the comma separating `&self` from the first
    // real argument (see `scan_fn_item`'s doc comment) unless it is the
    // single-trailing-comma case, which is already cleared to empty by the
    // caller before this function is ever invoked.
    let rest = match extra_args.first() {
        Some(tt) if is_punct(tt, ',') => extra_args.get(1..).unwrap_or_default(),
        _ => extra_args,
    };
    let groups = split_top_level_commas(rest);

    if let Some(seventeenth) = groups.get(16) {
        let span = seventeenth.first().map_or(entry_span, TokenTree::span);
        return Err(err(
            span,
            "#[hooks]: a hook entry function may declare at most 16 signature parameters \
             (index 0x00..=0x0F)",
        ));
    }

    let mut out = Vec::with_capacity(groups.len());
    for group in &groups {
        let (name_id, after_name) = match group.split_first() {
            Some((TokenTree::Ident(id), rest)) => (id.clone(), rest),
            Some((other, _)) => {
                return Err(err(
                    other.span(),
                    "#[hooks]: expected a signature parameter here, e.g. `count: u16`",
                ));
            }
            None => return Err(err(entry_span, "#[hooks]: expected a signature parameter")),
        };
        let after_colon = match after_name.split_first() {
            Some((tt, after)) if is_punct(tt, ':') => after,
            _ => {
                return Err(err(
                    name_id.span(),
                    "#[hooks]: expected `: Type` after the signature parameter's name",
                ));
            }
        };
        if after_colon.is_empty() {
            return Err(err(name_id.span(), "#[hooks]: expected a type after `:`"));
        }

        let name = name_id.to_string();
        if !is_valid_sig_arg_name(&name) {
            return Err(err(name_id.span(), SIG_NAME_MSG));
        }

        let Some(type_byte) = classify_sig_type(after_colon) else {
            let span = after_colon.first().map_or(name_id.span(), TokenTree::span);
            return Err(err(span, SIG_TYPE_MSG));
        };

        out.push(SigArg {
            name,
            type_text: tokens_to_string(after_colon),
            type_byte,
        });
    }
    Ok(out)
}

/// Builds one declared `HookParameterName`'s wire bytes, as uppercase hex —
/// the carrier's `name_hex` field (`docs/PARAM_SIGNATURE_DESIGN.md` §4).
/// Mirrors [`crate::sig::sig_param_name`] (the rshooks crate's runtime/const
/// builder) exactly, over plain bytes instead of a `const`-generic array.
fn sig_param_name_hex(index: u8, type_byte: u8, name: &str) -> String {
    let mut bytes = vec![0x5Fu8, 0x50, 0x53, 0x00, index, type_byte, name.len() as u8];
    bytes.extend_from_slice(name.as_bytes());
    hex_upper(&bytes)
}

/// Walks the impl body's flat token list item by item, stripping consumed
/// `#[hook(..)]`/`#[cbak(..)]` attributes and validating each entry
/// function's shape, while re-emitting everything else (including every
/// other associated item) verbatim.
fn parse_impl_body(tokens: &[TokenTree]) -> Result<ParsedBody, TokenStream> {
    let mut i = 0usize;
    let mut output: Vec<TokenTree> = Vec::new();
    let mut hooks: Vec<HookEntry> = Vec::new();
    let mut cbaks: Vec<CbakEntry> = Vec::new();

    while i < tokens.len() {
        let mut kept_attrs: Vec<TokenTree> = Vec::new();
        let mut hook_attr: Option<(HookAttrData, u8, Span)> = None;
        let mut cbak_attr: Option<(u8, Span)> = None;
        let mut cfg_span: Option<Span> = None;

        while let Some(tt) = tokens.get(i) {
            if !is_punct(tt, '#') {
                break;
            }
            let hash = tt.clone();
            let group = match tokens.get(i.wrapping_add(1)) {
                Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket => g.clone(),
                _ => {
                    return Err(err(
                        hash.span(),
                        "#[hooks]: malformed attribute inside a #[hooks] impl",
                    ));
                }
            };
            let inner: Vec<TokenTree> = group.stream().into_iter().collect();
            match inner.first() {
                Some(TokenTree::Ident(id)) if id.to_string() == "hook" => {
                    if hook_attr.is_some() {
                        return Err(err(
                            id.span(),
                            "#[hooks]: duplicate `#[hook(..)]` on the same item",
                        ));
                    }
                    let args = inert_attr_args(&inner, id, "hook")?;
                    let data = parse_hook_attr(&args, id)?;
                    hook_attr = Some(data);
                }
                Some(TokenTree::Ident(id)) if id.to_string() == "cbak" => {
                    if cbak_attr.is_some() {
                        return Err(err(
                            id.span(),
                            "#[hooks]: duplicate `#[cbak(..)]` on the same item",
                        ));
                    }
                    let args = inert_attr_args(&inner, id, "cbak")?;
                    cbak_attr = Some(parse_cbak_attr(&args, id)?);
                }
                Some(TokenTree::Ident(id))
                    if matches!(id.to_string().as_str(), "cfg" | "cfg_attr") =>
                {
                    cfg_span = Some(id.span());
                    kept_attrs.push(hash);
                    kept_attrs.push(TokenTree::Group(group));
                }
                _ => {
                    kept_attrs.push(hash);
                    kept_attrs.push(TokenTree::Group(group));
                }
            }
            i = i.wrapping_add(2);
        }

        if i >= tokens.len() {
            break;
        }

        let mut vis: Vec<TokenTree> = Vec::new();
        if let Some(tt @ TokenTree::Ident(id)) = tokens.get(i) {
            if id.to_string() == "pub" {
                vis.push(tt.clone());
                i = i.wrapping_add(1);
                if let Some(g @ TokenTree::Group(group)) = tokens.get(i) {
                    if group.delimiter() == Delimiter::Parenthesis {
                        vis.push(g.clone());
                        i = i.wrapping_add(1);
                    }
                }
            }
        }

        let is_entry = hook_attr.is_some() || cbak_attr.is_some();
        if is_entry && cfg_span.is_some() {
            return Err(err(
                cfg_span.unwrap_or_else(Span::call_site),
                "#[hooks]: `#[cfg]`/`#[cfg_attr]` are not allowed on a hook entry function",
            ));
        }
        if hook_attr.is_some() && cbak_attr.is_some() {
            return Err(err(
                Span::call_site(),
                "#[hooks]: a function cannot be both `#[hook]` and `#[cbak]`",
            ));
        }

        if let Some(q) = scan_fn_qualifiers(tokens, i) {
            if is_entry && q.has_qualifiers {
                let bad_span = tokens.get(i).map_or_else(Span::call_site, TokenTree::span);
                return Err(err(
                    bad_span,
                    "#[hooks]: a hook entry function (#[hook]/#[cbak]) must be a plain `fn` \
                     — remove the `const`/`async`/`unsafe`/`extern \"...\"` qualifier",
                ));
            }
            let qualifier_tokens = tokens.get(i..q.fn_index).unwrap_or_default().to_vec();
            let scanned = scan_fn_item(tokens, q.fn_index)?;
            i = scanned.next;

            if is_entry {
                match scanned.receiver {
                    Some((ReceiverClass::SharedSelf, _)) => {}
                    None => {
                        return Err(err(scanned.args_group.span(), ENTRY_MISSING_SELF_MSG));
                    }
                    Some((ReceiverClass::MutSelf, span)) => {
                        return Err(err(span, MUT_SELF_RECEIVER_MSG));
                    }
                    Some((ReceiverClass::Rejected, span)) => {
                        return Err(err(span, SELF_RECEIVER_MSG));
                    }
                }
                if scanned.has_generics {
                    return Err(err(
                        scanned.name.span(),
                        "#[hooks]: an entry function cannot be generic",
                    ));
                }
                // Extra arguments after `&self` declare signature
                // parameters (`docs/PARAM_SIGNATURE_DESIGN.md` §1), but
                // only on `#[hook(..)]`: a `#[cbak(..)]`'s originating
                // transaction is the emitted transaction, not the
                // invocation, so the interface doesn't apply there.
                if !scanned.extra_args.is_empty() && cbak_attr.is_some() {
                    let bad_span = scanned
                        .extra_args
                        .first()
                        .map_or_else(|| scanned.args_group.span(), TokenTree::span);
                    return Err(err(
                        bad_span,
                        "#[hooks]: #[cbak] entry functions must take no arguments other than \
                         `&self` — a callback's originating transaction is the emitted \
                         transaction, not the invocation, so the signature parameter interface \
                         does not apply",
                    ));
                }
                #[cfg(not(feature = "unstable-param-sig-interface"))]
                if !scanned.extra_args.is_empty() {
                    let bad_span = scanned
                        .extra_args
                        .first()
                        .map_or_else(|| scanned.args_group.span(), TokenTree::span);
                    return Err(err(bad_span, SIG_FEATURE_GATE_MSG));
                }
                // The return type itself is deliberately unchecked here: an
                // entry must return a type implementing
                // `::rshooks::exit::EntryReturn` (sealed to `HookResult` —
                // `.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md` §1.3/§3/§7).
                // The generated body wraps the call in
                // `EntryReturn::finish(..)` (`render_entry_body_and_wrappers`),
                // so a non-implementing type fails to compile there with an
                // ordinary trait-bound diagnostic — no separate check needed.
            } else {
                // Non-attributed helper (§3.3): `&self` passes through
                // untouched, `&mut self`/other self shapes are rejected with
                // the same diagnostics as entries.
                match scanned.receiver {
                    None | Some((ReceiverClass::SharedSelf, _)) => {}
                    Some((ReceiverClass::MutSelf, span)) => {
                        return Err(err(span, MUT_SELF_RECEIVER_MSG));
                    }
                    Some((ReceiverClass::Rejected, span)) => {
                        return Err(err(span, SELF_RECEIVER_MSG));
                    }
                }
            }

            let fn_name = scanned.name.to_string();
            if let Some((data, index, index_span)) = hook_attr {
                let sig_args = if scanned.extra_args.is_empty() {
                    Vec::new()
                } else {
                    parse_sig_args(&scanned.extra_args, scanned.args_group.span())?
                };
                hooks.push(HookEntry {
                    index,
                    index_span,
                    fn_name,
                    attr: data,
                    return_tokens: scanned.return_tokens.clone(),
                    return_span: scanned.return_span,
                    sig_args,
                });
            } else if let Some((index, index_span)) = cbak_attr {
                cbaks.push(CbakEntry {
                    index,
                    index_span,
                    fn_name,
                    return_tokens: scanned.return_tokens.clone(),
                    return_span: scanned.return_span,
                });
            }

            output.extend(kept_attrs);
            output.extend(vis);
            output.extend(qualifier_tokens);
            output.extend(scanned.tokens);
        } else {
            match tokens.get(i) {
                Some(TokenTree::Ident(id)) if id.to_string() == "const" => {
                    if is_entry {
                        return Err(err(
                            id.span(),
                            "#[hooks]: `#[hook]`/`#[cbak]` can only be applied to an \
                             associated function",
                        ));
                    }
                    let (const_tokens, next) = scan_const_item(tokens, i)?;
                    i = next;
                    output.extend(kept_attrs);
                    output.extend(vis);
                    output.extend(const_tokens);
                }
                Some(TokenTree::Ident(id)) if id.to_string() == "type" => {
                    return Err(err(
                        id.span(),
                        "#[hooks]: associated types are not allowed inside a #[hooks] impl",
                    ));
                }
                Some(other) => {
                    return Err(err(
                        other.span(),
                        "#[hooks]: expected an associated function or constant",
                    ));
                }
                None => {
                    return Err(err(
                        Span::call_site(),
                        "#[hooks]: expected an item after this attribute",
                    ));
                }
            }
        }
    }

    Ok(ParsedBody {
        tokens: output,
        hooks,
        cbaks,
    })
}

/// Extracts a `#[hook(..)]`/`#[cbak(..)]` attribute's parenthesized
/// argument tokens — `inner` is the whole bracket-group content
/// (`hook ( .. )`, i.e. `[Ident("hook"), Group(Paren, ..)]`); this returns
/// the `Group`'s own inner stream, not the `Group` token itself.
fn inert_attr_args(
    inner: &[TokenTree],
    attr_ident: &Ident,
    mac_name: &str,
) -> Result<Vec<TokenTree>, TokenStream> {
    match inner.get(1) {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
            Ok(g.stream().into_iter().collect())
        }
        Some(other) => Err(err(
            other.span(),
            &format!(
                "#[{mac_name}]: expected parenthesized arguments, e.g. `#[{mac_name}(0, ..)]`"
            ),
        )),
        None => Err(err(
            attr_ident.span(),
            &format!(
                "#[{mac_name}]: expected parenthesized arguments, e.g. `#[{mac_name}(0, ..)]`"
            ),
        )),
    }
}

/// The result of [`scan_fn_qualifiers`]: whether an item starting at some
/// token index is a (possibly qualified) function item.
struct FnQualifiers {
    /// The absolute token index of the `fn` keyword itself.
    fn_index: usize,
    /// Whether any qualifier (`const`/`async`/`unsafe`/`extern "..."`)
    /// preceded `fn`.
    has_qualifiers: bool,
}

/// Classifies the token kind relevant to [`classify_fn_qualifiers`] — the
/// four qualifier keywords, `fn` itself, a literal (the optional `extern`
/// ABI string), or anything else. `TokenTree`-typed on purpose (unlike
/// [`classify_fn_qualifiers`]) — this is the only place span/`Ident`
/// inspection happens for the qualifier scan.
fn qualifier_token_kind(tt: &TokenTree) -> &'static str {
    match tt {
        TokenTree::Ident(id) => match id.to_string().as_str() {
            "const" => "const",
            "async" => "async",
            "unsafe" => "unsafe",
            "extern" => "extern",
            "fn" => "fn",
            _ => "<other>",
        },
        TokenTree::Literal(_) => "<lit>",
        _ => "<other>",
    }
}

/// Looks ahead from `tokens[start]` for a (possibly empty) run of the
/// function-qualifier keywords Rust allows before `fn` — `const`/`async`
/// (mutually exclusive), then `unsafe`, then `extern "abi"` — followed by
/// `fn` itself. Returns `None` when `tokens[start]` does not begin a
/// (possibly qualified) function item at all (e.g. a plain associated
/// `const NAME: Ty = ..;` item, whose leading `const` is not followed by
/// `fn`).
///
/// Delegates the actual decision to [`classify_fn_qualifiers`], which is
/// `TokenTree`-free purely for unit-testability: every `proc_macro` type,
/// `Span` included, panics outside a live macro invocation, so the tested
/// core works over plain `&str` "kind" tags instead.
fn scan_fn_qualifiers(tokens: &[TokenTree], start: usize) -> Option<FnQualifiers> {
    // At most 5 tokens are ever consulted: `const`/`async`, `unsafe`,
    // `extern`, its optional ABI literal, `fn`.
    let kinds: Vec<&'static str> = (0..5)
        .map_while(|off| {
            tokens
                .get(start.wrapping_add(off))
                .map(qualifier_token_kind)
        })
        .collect();
    let (offset, has_qualifiers) = classify_fn_qualifiers(&kinds)?;
    Some(FnQualifiers {
        fn_index: start.wrapping_add(offset),
        has_qualifiers,
    })
}

/// Pure decision logic behind [`scan_fn_qualifiers`]: given the leading
/// token "kinds" of an item (as produced by [`qualifier_token_kind`]),
/// decides whether it is a (possibly qualified) function item and, if so,
/// the offset of `fn` within `kinds` plus whether any qualifier preceded
/// it. Returns `None` when no `fn` terminates the qualifier run (e.g.
/// `["const", "NAME"]` — a plain associated const, not a const fn).
fn classify_fn_qualifiers(kinds: &[&str]) -> Option<(usize, bool)> {
    let mut i = 0usize;
    let mut has_qualifiers = false;
    if matches!(kinds.get(i), Some(&"const") | Some(&"async")) {
        i = i.wrapping_add(1);
        has_qualifiers = true;
    }
    if matches!(kinds.get(i), Some(&"unsafe")) {
        i = i.wrapping_add(1);
        has_qualifiers = true;
    }
    if matches!(kinds.get(i), Some(&"extern")) {
        i = i.wrapping_add(1);
        has_qualifiers = true;
        if matches!(kinds.get(i), Some(&"<lit>")) {
            i = i.wrapping_add(1);
        }
    }
    match kinds.get(i) {
        Some(&"fn") => Some((i, has_qualifiers)),
        _ => None,
    }
}

/// A scanned associated-function item: enough shape information for the
/// entry-fn checks (§5.7), plus the exact original tokens (`fn` through the
/// closing `}` of the body) for verbatim re-emission.
struct ScannedFn {
    tokens: Vec<TokenTree>,
    name: Ident,
    has_generics: bool,
    /// The leading receiver, if any: its class plus the `self` token's span.
    /// `None` means no receiver at all — rejected for an entry function
    /// (which requires exactly `&self`), legal for an ordinary helper's
    /// first parameter.
    receiver: Option<(ReceiverClass, Span)>,
    /// Argument-list tokens following the receiver, minus a single optional
    /// trailing top-level `,` — used by the entry-fn shape check to reject
    /// any parameter beyond the mandatory `&self`. Unused for helpers,
    /// which may take any further parameters.
    extra_args: Vec<TokenTree>,
    args_group: Group,
    /// The `-> Ty` return type's own tokens, empty for an implicit `-> ()`.
    /// Carried alongside `tokens` (where the same tokens are re-emitted
    /// verbatim) so a diagnostic can be built from them directly, without
    /// the span loss a `format!` + reparse round-trip would cause — see
    /// [`build_entry_return_assertion`].
    return_tokens: Vec<TokenTree>,
    /// Fallback span for an implicit `-> ()` (`return_tokens` empty): the
    /// fn's own name.
    return_span: Span,
    next: usize,
}

/// Scans one `fn` item starting at `tokens[start]` (already confirmed to be
/// the `fn` keyword), consuming through its closing body brace.
fn scan_fn_item(tokens: &[TokenTree], start: usize) -> Result<ScannedFn, TokenStream> {
    let mut i = start;
    let mut collected: Vec<TokenTree> = Vec::new();

    let Some(fn_kw) = tokens.get(i) else {
        return Err(err(Span::call_site(), "#[hooks]: expected `fn`"));
    };
    collected.push(fn_kw.clone());
    i = i.wrapping_add(1);

    let name = match tokens.get(i) {
        Some(TokenTree::Ident(id)) => id.clone(),
        Some(other) => return Err(err(other.span(), "#[hooks]: expected a function name")),
        None => return Err(err(Span::call_site(), "#[hooks]: expected a function name")),
    };
    collected.push(TokenTree::Ident(name.clone()));
    i = i.wrapping_add(1);

    let mut has_generics = false;
    if matches!(tokens.get(i), Some(tt) if is_punct(tt, '<')) {
        has_generics = true;
        let Some((_, after)) = parse_balanced_angle(tokens, i) else {
            return Err(err(
                name.span(),
                "#[hooks]: unbalanced `<..>` in function generics",
            ));
        };
        collected.extend_from_slice(tokens.get(i..after).unwrap_or_default());
        i = after;
    }

    let args_group = match tokens.get(i) {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => g.clone(),
        Some(other) => {
            return Err(err(
                other.span(),
                "#[hooks]: expected `(...)` after the function name",
            ));
        }
        None => {
            return Err(err(
                name.span(),
                "#[hooks]: expected `(...)` after the function name",
            ));
        }
    };
    collected.push(TokenTree::Group(args_group.clone()));
    i = i.wrapping_add(1);

    let args_tokens: Vec<TokenTree> = args_group.stream().into_iter().collect();
    let detected_receiver = detect_receiver(&args_tokens)?;
    let receiver = detected_receiver.map(|(class, span, _)| (class, span));
    let receiver_consumed = detected_receiver.map_or(0, |(_, _, consumed)| consumed);
    let mut extra_args: Vec<TokenTree> = args_tokens
        .get(receiver_consumed..)
        .unwrap_or_default()
        .to_vec();
    if let [tt] = extra_args.as_slice() {
        if is_punct(tt, ',') {
            extra_args.clear();
        }
    }

    // Return-type tokens are re-emitted verbatim but otherwise unexamined:
    // only a type implementing `::rshooks::exit::EntryReturn` (sealed to
    // `HookResult`) is accepted, enforced by the generated body's own trait
    // bound. They're captured (`return_tokens`/`return_span` below) so
    // `build_entry_return_assertion` can re-emit them with original spans,
    // landing a diagnostic on the caller's `-> Ty` instead of `#[hooks]`.
    let mut return_tokens: Vec<TokenTree> = Vec::new();
    let mut return_span = name.span();
    if matches!(tokens.get(i), Some(tt) if is_punct(tt, '-'))
        && matches!(tokens.get(i.wrapping_add(1)), Some(tt) if is_punct(tt, '>'))
    {
        collected.extend_from_slice(tokens.get(i..i.wrapping_add(2)).unwrap_or_default());
        i = i.wrapping_add(2);
        let ret_start = i;
        while let Some(tt) = tokens.get(i) {
            let stop = matches!(tt, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace)
                || matches!(tt, TokenTree::Ident(id) if id.to_string() == "where");
            if stop {
                break;
            }
            i = i.wrapping_add(1);
        }
        return_tokens = tokens.get(ret_start..i).unwrap_or_default().to_vec();
        if let Some(first) = return_tokens.first() {
            return_span = first.span();
        }
        collected.extend_from_slice(&return_tokens);
    }

    if matches!(tokens.get(i), Some(TokenTree::Ident(id)) if id.to_string() == "where") {
        let where_start = i;
        while let Some(tt) = tokens.get(i) {
            if matches!(tt, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace) {
                break;
            }
            i = i.wrapping_add(1);
        }
        collected.extend_from_slice(tokens.get(where_start..i).unwrap_or_default());
    }

    match tokens.get(i) {
        Some(g @ TokenTree::Group(group)) if group.delimiter() == Delimiter::Brace => {
            collected.push(g.clone());
        }
        Some(other) => return Err(err(other.span(), "#[hooks]: expected a function body")),
        None => return Err(err(name.span(), "#[hooks]: expected a function body")),
    }
    i = i.wrapping_add(1);

    Ok(ScannedFn {
        tokens: collected,
        name,
        has_generics,
        receiver,
        extra_args,
        args_group,
        return_tokens,
        return_span,
        next: i,
    })
}

/// The receiver shapes an argument list's leading `self` parameter can take,
/// classified per HOOKS_SELF_RECEIVER_DESIGN.md §3.1/§6.2. Only
/// [`SharedSelf`](Self::SharedSelf) is accepted; the other two select which
/// of the two §6.2 diagnostics a rejected shape gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiverClass {
    /// Bare `&self` — no lifetime, not `mut`. The only accepted receiver.
    SharedSelf,
    /// `&mut self` or `&'a mut self` — gets [`MUT_SELF_RECEIVER_MSG`].
    MutSelf,
    /// Every other self-receiver shape: `self`, `mut self`, `self: T`,
    /// `&'a self` — gets [`SELF_RECEIVER_MSG`].
    Rejected,
}

/// Classifies one token relevant to receiver-shape detection, for
/// [`classify_receiver_kinds`]'s `TokenTree`-free pure core — the same
/// "kind tag" pattern as [`qualifier_token_kind`], for unit-testability
/// without a live `proc_macro` context.
fn receiver_token_kind(tt: &TokenTree) -> &'static str {
    match tt {
        TokenTree::Ident(id) => match id.to_string().as_str() {
            "self" => "self",
            "mut" => "mut",
            _ => "<ident>",
        },
        _ if is_punct(tt, '&') => "&",
        _ if is_punct(tt, '\'') => "'",
        _ => "<other>",
    }
}

/// Pure decision logic behind [`detect_receiver`]: given the leading token
/// "kinds" of an argument list (as produced by [`receiver_token_kind`]),
/// decides whether it opens with a self-receiver shape and, if so, its
/// class plus the offset of the `self` token within `kinds` (so the caller
/// can recover its span). Returns `None` when the argument list does not
/// open with a self receiver at all — no receiver, or an ordinary first
/// parameter such as `mut x: i64` whose leading `mut` isn't followed by
/// `self`. The optional `: T` type ascription on `self: T` isn't inspected
/// beyond confirming `self` leads — the ascribed type doesn't change which
/// diagnostic applies.
fn classify_receiver_kinds(kinds: &[&str]) -> Option<(usize, ReceiverClass)> {
    match kinds.first() {
        Some(&"mut") => (kinds.get(1) == Some(&"self")).then_some((1, ReceiverClass::Rejected)),
        Some(&"self") => Some((0, ReceiverClass::Rejected)),
        Some(&"&") => {
            let mut i = 1usize;
            let mut has_lifetime = false;
            if kinds.get(i) == Some(&"'") {
                i = i.wrapping_add(1);
                has_lifetime = true;
                if kinds.get(i) == Some(&"<ident>") {
                    i = i.wrapping_add(1);
                }
            }
            let mut has_mut = false;
            if kinds.get(i) == Some(&"mut") {
                has_mut = true;
                i = i.wrapping_add(1);
            }
            if kinds.get(i) != Some(&"self") {
                return None;
            }
            let class = if has_mut {
                ReceiverClass::MutSelf
            } else if has_lifetime {
                ReceiverClass::Rejected
            } else {
                ReceiverClass::SharedSelf
            };
            Some((i, class))
        }
        _ => None,
    }
}

/// Extends [`classify_receiver_kinds`] to look past zero or more leading
/// attribute "kinds" — one `"attr"` or `"cfg_attr"` tag per `#[...]` pair,
/// as built by [`detect_receiver`] — before classifying the self-shape
/// that follows. HOOKS_SELF_RECEIVER_DESIGN.md §3.3: `#[allow(unused)]
/// &self` must classify exactly like bare `&self`. Returns the number of
/// leading attribute kinds skipped, whether any of them was
/// `"cfg_attr"`, and the self-shape classification of what followed —
/// `None` in the third slot means no receiver shape follows the attributes
/// (including the no-attributes case, which behaves exactly like
/// [`classify_receiver_kinds`] alone).
fn classify_receiver_prefix(kinds: &[&str]) -> (usize, bool, Option<(usize, ReceiverClass)>) {
    let mut attrs = 0usize;
    let mut has_cfg_attr = false;
    while let Some(&kind) = kinds
        .get(attrs)
        .filter(|&&k| k == "attr" || k == "cfg_attr")
    {
        has_cfg_attr |= kind == "cfg_attr";
        attrs = attrs.wrapping_add(1);
    }
    let receiver = classify_receiver_kinds(kinds.get(attrs..).unwrap_or_default());
    (attrs, has_cfg_attr, receiver)
}

/// Looks ahead from the start of an argument-list token run for a self
/// receiver (`self`, `mut self`, `&self`, `&mut self`, `&'a self`,
/// `&'a mut self`, or `self: T`) — the receiver shapes rustc itself
/// accepts, classified per [`ReceiverClass`]. Leading outer attributes
/// (`#[...]` pairs, e.g. `#[allow(unused)]`) are skipped before
/// classification — see [`classify_receiver_prefix`] for the
/// `TokenTree`-free decision core this follows. A `#[cfg]`/`#[cfg_attr]`
/// attribute found on an actual receiver is rejected outright
/// ([`RECEIVER_CFG_MSG`]): other attributes pass through untouched.
///
/// Returns the receiver's class, its `self` token's span, and the number
/// of leading tokens it consumes — any skipped attributes included — so
/// the caller can locate any further parameters. `Ok(None)` means no
/// receiver was found at all: an ordinary first parameter (possibly itself
/// attributed — those attributes are left for the caller to re-emit
/// untouched, since they weren't on a receiver), or an empty argument
/// list.
fn detect_receiver(
    tokens: &[TokenTree],
) -> Result<Option<(ReceiverClass, Span, usize)>, TokenStream> {
    let mut raw = 0usize;
    let mut kinds: Vec<&'static str> = Vec::new();
    let mut cfg_span: Option<Span> = None;
    while let Some(tt) = tokens.get(raw) {
        if !is_punct(tt, '#') {
            break;
        }
        let group = match tokens.get(raw.wrapping_add(1)) {
            Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket => g,
            // Not a well-formed `#[...]` attribute — leave it for the
            // ordinary parameter/receiver classification below to report.
            _ => break,
        };
        let inner: Vec<TokenTree> = group.stream().into_iter().collect();
        let cfg_ident = match inner.first() {
            Some(TokenTree::Ident(id)) if matches!(id.to_string().as_str(), "cfg" | "cfg_attr") => {
                Some(id.span())
            }
            _ => None,
        };
        if cfg_span.is_none() {
            cfg_span = cfg_ident;
        }
        kinds.push(if cfg_ident.is_some() {
            "cfg_attr"
        } else {
            "attr"
        });
        raw = raw.wrapping_add(2);
    }
    // At most 5 more tokens are ever consulted for the self-shape itself:
    // `&`, `'`, a lifetime ident, `mut`, `self`.
    kinds
        .extend((0..5).map_while(|off| tokens.get(raw.wrapping_add(off)).map(receiver_token_kind)));

    let (_, has_cfg_attr, receiver) = classify_receiver_prefix(&kinds);
    let Some((self_offset, class)) = receiver else {
        return Ok(None);
    };
    if has_cfg_attr {
        return Err(err(
            cfg_span.unwrap_or_else(Span::call_site),
            RECEIVER_CFG_MSG,
        ));
    }
    let offset = raw.wrapping_add(self_offset);
    let span = tokens
        .get(offset)
        .map_or_else(Span::call_site, TokenTree::span);
    Ok(Some((class, span, offset.wrapping_add(1))))
}

/// Consumes an associated `const` item (`const NAME: Ty = expr;`) starting
/// at `tokens[start]`, up through its top-level `;` — safe to scan flatly
/// since every nested brace/paren/bracket in `expr` is already an atomic
/// [`proc_macro::Group`], never a bare `;` `Punct`.
fn scan_const_item(
    tokens: &[TokenTree],
    start: usize,
) -> Result<(Vec<TokenTree>, usize), TokenStream> {
    let mut i = start;
    while let Some(tt) = tokens.get(i) {
        i = i.wrapping_add(1);
        if is_punct(tt, ';') {
            break;
        }
    }
    Ok((tokens.get(start..i).unwrap_or_default().to_vec(), i))
}

// ---------------------------------------------------------------------
// `#[hook(..)]` / `#[cbak(..)]` argument grammar
// ---------------------------------------------------------------------

/// Consumes the required leading positional index (`0..=9`) plus an
/// optional trailing `,`, returning the remaining argument tokens.
fn parse_index_arg<'a>(
    args: &'a [TokenTree],
    mac_name: &str,
    attr_ident: &Ident,
) -> Result<(u8, Span, &'a [TokenTree]), TokenStream> {
    let (index, lit_span) = match args.first() {
        Some(TokenTree::Literal(lit)) => {
            let text = lit.to_string();
            let index: u8 = text.parse().map_err(|_| {
                err(
                    lit.span(),
                    &format!(
                        "#[{mac_name}]: the leading index must be a plain integer \
                         literal `0..=9`"
                    ),
                )
            })?;
            (index, lit.span())
        }
        Some(other) => {
            return Err(err(
                other.span(),
                &format!(
                    "#[{mac_name}]: expected a positional index `0..=9` first, \
                     e.g. `#[{mac_name}(0, ..)]`"
                ),
            ));
        }
        None => {
            return Err(err(
                attr_ident.span(),
                &format!(
                    "#[{mac_name}]: expected a positional index `0..=9` first, \
                     e.g. `#[{mac_name}(0, ..)]`"
                ),
            ));
        }
    };
    if index > MAX_INDEX {
        return Err(err(
            lit_span,
            &format!("#[{mac_name}]: index must be in range 0..=9, found {index}"),
        ));
    }

    let mut rest = args.get(1..).unwrap_or_default();
    if let Some(tt) = rest.first() {
        if is_punct(tt, ',') {
            rest = rest.get(1..).unwrap_or_default();
        } else {
            return Err(err(
                tt.span(),
                &format!("#[{mac_name}]: expected `,` after the index"),
            ));
        }
    }
    Ok((index, lit_span, rest))
}

fn parse_cbak_attr(args: &[TokenTree], attr_ident: &Ident) -> Result<(u8, Span), TokenStream> {
    let (index, index_span, rest) = parse_index_arg(args, "cbak", attr_ident)?;
    if let Some(extra) = rest.first() {
        return Err(err(
            extra.span(),
            "#[cbak]: takes only the index — no other arguments",
        ));
    }
    Ok((index, index_span))
}

fn parse_hook_attr(
    args: &[TokenTree],
    attr_ident: &Ident,
) -> Result<(HookAttrData, u8, Span), TokenStream> {
    let (index, index_span, rest) = parse_index_arg(args, "hook", attr_ident)?;
    let entries = parse_attr_entries(rest, "#[hook]")?;

    let mut name: Option<(String, Span)> = None;
    let mut on_all_span: Option<Span> = None;
    let mut on_list: Option<Vec<(String, Span)>> = None;
    let mut on_incoming: Option<Vec<(String, Span)>> = None;
    let mut on_outgoing: Option<Vec<(String, Span)>> = None;
    let mut can_emit: Option<Vec<(String, Span)>> = None;
    let mut description: Option<String> = None;

    for AttrEntry {
        key,
        key_span,
        value,
    } in entries
    {
        match key.as_str() {
            "name" => {
                if name.is_some() {
                    return Err(err(key_span, "#[hook]: duplicate `name`"));
                }
                let s = parse_string_value(value.as_deref(), key_span, "#[hook]", "name")?;
                let chars = s.chars().count();
                if !(2..=8).contains(&chars) {
                    return Err(err(
                        key_span,
                        "#[hook]: `name` must contain 2 to 8 Unicode scalar values",
                    ));
                }
                name = Some((s, key_span));
            }
            "on" => {
                if on_list.is_some() || on_all_span.is_some() {
                    return Err(err(key_span, "#[hook]: duplicate `on`"));
                }
                let Some(toks) = value else {
                    return Err(err(
                        key_span,
                        "#[hook]: expected `on = all` or `on = [Tx, ..]`",
                    ));
                };
                if let [TokenTree::Ident(id)] = toks.as_slice() {
                    if id.to_string() == "all" {
                        on_all_span = Some(key_span);
                        continue;
                    }
                }
                on_list = Some(parse_tx_list(&toks, key_span, "#[hook]", "on")?);
            }
            "on_incoming" => {
                if on_incoming.is_some() {
                    return Err(err(key_span, "#[hook]: duplicate `on_incoming`"));
                }
                let Some(toks) = value else {
                    return Err(err(key_span, "#[hook]: expected `on_incoming = [Tx, ..]`"));
                };
                on_incoming = Some(parse_tx_list(&toks, key_span, "#[hook]", "on_incoming")?);
            }
            "on_outgoing" => {
                if on_outgoing.is_some() {
                    return Err(err(key_span, "#[hook]: duplicate `on_outgoing`"));
                }
                let Some(toks) = value else {
                    return Err(err(key_span, "#[hook]: expected `on_outgoing = [Tx, ..]`"));
                };
                on_outgoing = Some(parse_tx_list(&toks, key_span, "#[hook]", "on_outgoing")?);
            }
            "can_emit" => {
                if can_emit.is_some() {
                    return Err(err(key_span, "#[hook]: duplicate `can_emit`"));
                }
                let Some(toks) = value else {
                    return Err(err(key_span, "#[hook]: expected `can_emit = [Tx, ..]`"));
                };
                can_emit = Some(parse_tx_list(&toks, key_span, "#[hook]", "can_emit")?);
            }
            "description" => {
                if description.is_some() {
                    return Err(err(key_span, "#[hook]: duplicate `description`"));
                }
                description = Some(parse_string_value(
                    value.as_deref(),
                    key_span,
                    "#[hook]",
                    "description",
                )?);
            }
            other => {
                return Err(err(
                    key_span,
                    &format!(
                        "#[hook]: unknown argument `{other}` (expected `name`, `on`, \
                         `on_incoming`, `on_outgoing`, `can_emit` or `description`)"
                    ),
                ));
            }
        }
    }

    let on = match (on_all_span, on_list, on_incoming, on_outgoing) {
        (Some(_), None, None, None) => OnSpec::All,
        (None, Some(list), None, None) => OnSpec::List(list),
        (None, None, Some(inc), Some(out)) => {
            let inc_set: BTreeSet<&str> = inc.iter().map(|(n, _)| n.as_str()).collect();
            let out_set: BTreeSet<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
            if inc_set == out_set {
                return Err(err(
                    attr_ident.span(),
                    "#[hook]: `on_incoming` and `on_outgoing` must not contain the same \
                     transaction-type set; use `on` instead",
                ));
            }
            OnSpec::Directional(inc, out)
        }
        (None, None, None, None) => OnSpec::Omitted,
        (None, None, Some(_), None) | (None, None, None, Some(_)) => {
            return Err(err(
                attr_ident.span(),
                "#[hook]: `on_incoming` and `on_outgoing` must be specified together",
            ));
        }
        _ => {
            return Err(err(
                attr_ident.span(),
                "#[hook]: use either `on` or the `on_incoming` + `on_outgoing` pair, \
                 not both",
            ));
        }
    };

    Ok((
        HookAttrData {
            name,
            on,
            can_emit,
            description,
        },
        index,
        index_span,
    ))
}

/// Finds the index of the first token that would make a bracketed
/// transaction-type list carry an empty element — a leading `,` (`[,Foo]`),
/// or two `,` with nothing between them anywhere (`[Foo,,Bar]`, `[,,]`) —
/// once [`split_top_level_commas`]'s lossy split would otherwise silently
/// drop it. A single trailing `,` (`[Foo,]`) is not a violation. Returns
/// `None` when the list has no such violation, `[]` included.
///
/// Operates on a plain "is this position a comma" view rather than
/// `&[TokenTree]` purely for unit-testability (this crate's standard
/// `TokenTree`-free boundary pattern).
fn first_empty_tx_entry(is_comma: &[bool]) -> Option<usize> {
    if is_comma.first().copied() == Some(true) {
        return Some(0);
    }
    for i in 0..is_comma.len() {
        if is_comma.get(i).copied() != Some(true) {
            continue;
        }
        if is_comma.get(i.wrapping_add(1)).copied() == Some(true) {
            return Some(i.wrapping_add(1));
        }
    }
    None
}

/// Parses `[Tx, Tx, ..]` — a bracketed, comma-separated list of bare
/// transaction-type identifiers — rejecting a non-bracket value and a
/// duplicate entry.
fn parse_tx_list(
    toks: &[TokenTree],
    span: Span,
    mac: &str,
    field: &str,
) -> Result<Vec<(String, Span)>, TokenStream> {
    let group = match toks {
        [TokenTree::Group(g)] if g.delimiter() == Delimiter::Bracket => g.clone(),
        _ => {
            return Err(err(
                span,
                &format!(
                    "{mac}: `{field}` must be an array of bare transaction-type names, \
                     e.g. `{field} = [Payment]`"
                ),
            ));
        }
    };
    let inner: Vec<TokenTree> = group.stream().into_iter().collect();
    let is_comma: Vec<bool> = inner.iter().map(|tt| is_punct(tt, ',')).collect();
    if let Some(bad_idx) = first_empty_tx_entry(&is_comma) {
        let bad_span = inner.get(bad_idx).map_or(span, TokenTree::span);
        return Err(err(
            bad_span,
            &format!(
                "{mac}: `{field}` has an empty entry (a `,` with no transaction-type name \
                 before it) — only a single trailing `,` is allowed"
            ),
        ));
    }
    let groups = split_top_level_commas(&inner);
    let mut out: Vec<(String, Span)> = Vec::new();
    for g in groups {
        match g.as_slice() {
            [TokenTree::Ident(id)] => {
                let name = id.to_string();
                if out.iter().any(|(n, _)| n == &name) {
                    return Err(err(
                        id.span(),
                        &format!("{mac}: duplicate transaction type `{name}` in `{field}`"),
                    ));
                }
                out.push((name, id.span()));
            }
            [first, ..] => {
                return Err(err(
                    first.span(),
                    &format!("{mac}: `{field}` entries must be bare transaction-type names"),
                ));
            }
            [] => {}
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// cross-entry validation
// ---------------------------------------------------------------------

fn validate_cross(hooks: &[HookEntry], cbaks: &[CbakEntry]) -> Result<(), TokenStream> {
    if hooks.is_empty() {
        return Err(err(
            Span::call_site(),
            "#[hooks]: at least one #[hook(..)] entry is required in this impl block",
        ));
    }

    let mut seen_hook: BTreeSet<u8> = BTreeSet::new();
    for h in hooks {
        if !seen_hook.insert(h.index) {
            return Err(err(
                h.index_span,
                &format!(
                    "#[hook]: duplicate index {} (already declared by another #[hook] \
                     in this impl)",
                    h.index
                ),
            ));
        }
    }

    let mut seen_cbak: BTreeSet<u8> = BTreeSet::new();
    for c in cbaks {
        if !seen_cbak.insert(c.index) {
            return Err(err(
                c.index_span,
                &format!(
                    "#[cbak]: duplicate index {} (already declared by another #[cbak] \
                     in this impl)",
                    c.index
                ),
            ));
        }
        if !seen_hook.contains(&c.index) {
            return Err(err(
                c.index_span,
                &format!(
                    "#[cbak({idx})]: no #[hook({idx})] declared for this index — a \
                     cbak-only index is not allowed",
                    idx = c.index
                ),
            ));
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------
// codegen
// ---------------------------------------------------------------------

fn generate(
    parsed: &ParsedImpl,
    body_out: Vec<TokenTree>,
    mut hooks: Vec<HookEntry>,
    cbaks: Vec<CbakEntry>,
) -> TokenStream {
    hooks.sort_by_key(|h| h.index);
    let struct_name = parsed.struct_name.to_string();

    let entries: Vec<EntrySource<'_>> = hooks
        .iter()
        .map(|h| {
            let cbak = cbaks.iter().find(|c| c.index == h.index);
            EntrySource {
                index: h.index,
                hook_fn: h.fn_name.as_str(),
                cbak_fn: cbak.map(|c| c.fn_name.as_str()),
                hook: &h.attr,
                sig_args: &h.sig_args,
            }
        })
        .collect();

    let mut out = TokenStream::new();
    out.extend(parsed.leading_attrs.iter().cloned());
    out.extend([
        parsed.impl_kw.clone(),
        TokenTree::Ident(parsed.struct_name.clone()),
        TokenTree::Group(Group::new(Delimiter::Brace, body_out.into_iter().collect())),
    ]);

    let entries_json: Vec<EntryJson> = entries.iter().map(EntrySource::to_json).collect();
    let mod_src = match render_entries_module(&struct_name, &entries, &entries_json) {
        Ok(src) => src,
        Err(message) => return err(parsed.struct_name.span(), &message),
    };
    let mod_ts = match mod_src.parse::<TokenStream>() {
        Ok(ts) => ts,
        Err(_) => {
            return err(
                parsed.struct_name.span(),
                "rshooks-macros: internal #[hooks] impl expansion failed to parse",
            );
        }
    };
    out.extend(mod_ts);

    // Span-carrying `EntryReturn` diagnostic assertions: one per declared
    // entry fn, built from the captured `-> Ty` tokens and spliced into
    // `out` as real `TokenTree`s. They must land here, not in `mod_src`
    // above — that string is reparsed via `TokenStream::parse`, which only
    // ever produces call-site spans, so anything built from it can't point
    // a diagnostic at the caller's own source line.
    for h in &hooks {
        out.extend(build_entry_return_assertion(
            &h.return_tokens,
            h.return_span,
        ));
    }
    for c in &cbaks {
        out.extend(build_entry_return_assertion(
            &c.return_tokens,
            c.return_span,
        ));
    }

    out
}

/// Builds one `const _: fn(<ret>) -> i64 = <<ret> as
/// ::rshooks::exit::EntryReturn>::finish;` diagnostic assertion from an
/// entry fn's captured return-type tokens (`HookEntry`/`CbakEntry`'s
/// `return_tokens`/`return_span`).
///
/// `<RetType as ::rshooks::exit::EntryReturn>::finish` names an associated
/// function whose only generic parameter is `Self = RetType`; taking it as
/// a value (rather than calling it) coerces to the plain function pointer
/// type `fn(RetType) -> i64` (`finish`'s receiver is `self: RetType`), so
/// the assignment above type-checks exactly when `RetType: EntryReturn` —
/// the same requirement the generated entry body's own
/// `EntryReturn::finish(<call>)` imposes. A `RetType` that doesn't satisfy
/// it fails with the ordinary E0277 trait-bound diagnostic, located at
/// wherever `ret_tokens` (cloned into the assertion twice, spans intact)
/// came from: the caller's own `-> Ty`.
///
/// `ret_tokens` empty means an implicit `-> ()` — there are no user tokens
/// to reuse, so a synthetic `()` is built from `fallback_span` (the entry
/// fn's own name) instead, keeping the diagnostic on the right line.
///
/// Every other token in the assertion (`const`, `fn`, `as`, the
/// `::rshooks::exit::EntryReturn` path, ...) is glue: its span is
/// irrelevant because none of those tokens can themselves be the mismatched
/// type, so they're all built at `Span::call_site()`.
fn build_entry_return_assertion(ret_tokens: &[TokenTree], fallback_span: Span) -> Vec<TokenTree> {
    let ret_ty: Vec<TokenTree> = if ret_tokens.is_empty() {
        let mut unit = Group::new(Delimiter::Parenthesis, TokenStream::new());
        unit.set_span(fallback_span);
        vec![TokenTree::Group(unit)]
    } else {
        ret_tokens.to_vec()
    };

    let glue = Span::call_site();
    let ident = |s: &str| TokenTree::Ident(Ident::new(s, glue));
    let joint = |c: char| TokenTree::Punct(Punct::new(c, Spacing::Joint));
    let alone = |c: char| TokenTree::Punct(Punct::new(c, Spacing::Alone));
    let path_sep = |out: &mut Vec<TokenTree>| {
        out.push(joint(':'));
        out.push(alone(':'));
    };

    let mut out: Vec<TokenTree> = vec![ident("const"), ident("_"), alone(':'), ident("fn")];
    let fn_arg_stream: TokenStream = ret_ty.iter().cloned().collect();
    out.push(TokenTree::Group(Group::new(
        Delimiter::Parenthesis,
        fn_arg_stream,
    )));
    out.push(joint('-'));
    out.push(alone('>'));
    out.push(ident("i64"));
    out.push(alone('='));
    out.push(alone('<'));
    out.extend(ret_ty.iter().cloned());
    out.push(ident("as"));
    path_sep(&mut out);
    out.push(ident("rshooks"));
    path_sep(&mut out);
    out.push(ident("exit"));
    path_sep(&mut out);
    out.push(ident("EntryReturn"));
    out.push(alone('>'));
    path_sep(&mut out);
    out.push(ident("finish"));
    out.push(alone(';'));
    out
}

/// One entry, still borrowing the parsed attribute data (used for both the
/// wrapper codegen, which needs `struct_name`/`hook_fn`/`cbak_fn` text, and
/// the JSON carrier, via [`EntrySource::to_json`]).
struct EntrySource<'a> {
    index: u8,
    hook_fn: &'a str,
    cbak_fn: Option<&'a str>,
    hook: &'a HookAttrData,
    sig_args: &'a [SigArg],
}

impl EntrySource<'_> {
    fn to_json(&self) -> EntryJson {
        let (form, list, incoming, outgoing) = match &self.hook.on {
            OnSpec::Omitted => (OnForm::Omitted, None, None, None),
            OnSpec::All => (OnForm::All, None, None, None),
            OnSpec::List(list) => (
                OnForm::List,
                Some(list.iter().map(|(n, _)| n.clone()).collect()),
                None,
                None,
            ),
            OnSpec::Directional(inc, out) => (
                OnForm::Directional,
                None,
                Some(inc.iter().map(|(n, _)| n.clone()).collect()),
                Some(out.iter().map(|(n, _)| n.clone()).collect()),
            ),
        };
        EntryJson {
            index: self.index,
            hook_fn: self.hook_fn.to_string(),
            cbak_fn: self.cbak_fn.map(str::to_string),
            hook_name: self.hook.name.as_ref().map(|(n, _)| n.clone()),
            on_form: form,
            hook_on: list,
            hook_on_incoming: incoming,
            hook_on_outgoing: outgoing,
            hook_can_emit: self
                .hook
                .can_emit
                .as_ref()
                .map(|list| list.iter().map(|(n, _)| n.clone()).collect()),
            description: self.hook.description.clone(),
            sig_params: self
                .sig_args
                .iter()
                .enumerate()
                .map(|(idx, a)| {
                    let idx = u8::try_from(idx).unwrap_or(u8::MAX);
                    SigParamJson {
                        field: a.name.clone(),
                        type_text: a.type_text.clone(),
                        type_byte: a.type_byte,
                        name_hex: sig_param_name_hex(idx, a.type_byte, &a.name),
                    }
                })
                .collect(),
        }
    }
}

/// A `proc_macro`-free (plain-`String`) view of one entry's carrier JSON
/// entry — unit-testable without a live `proc_macro` context.
struct EntryJson {
    index: u8,
    hook_fn: String,
    cbak_fn: Option<String>,
    hook_name: Option<String>,
    on_form: OnForm,
    hook_on: Option<Vec<String>>,
    hook_on_incoming: Option<Vec<String>>,
    hook_on_outgoing: Option<Vec<String>>,
    hook_can_emit: Option<Vec<String>>,
    description: Option<String>,
    /// Declared signature parameters (`docs/PARAM_SIGNATURE_DESIGN.md` §1),
    /// in wire-index order. Empty for an ordinary entry.
    sig_params: Vec<SigParamJson>,
}

/// One declared signature parameter, as carried by [`EntryJson`]. Only
/// `field`/`type_byte`/`name_hex` are part of the wire carrier
/// (`docs/PARAM_SIGNATURE_DESIGN.md` §4) — `type_text` is codegen-only (the
/// exact type token text spliced into the generated prologue) and never
/// serialized into the carrier JSON.
struct SigParamJson {
    /// The declared name — the argument's own identifier.
    field: String,
    /// The argument's type, as source text (see [`tokens_to_string`]) —
    /// codegen-only, not part of the wire carrier.
    type_text: String,
    /// The `STI_*` type byte.
    type_byte: u8,
    /// The full declared `HookParameterName`, uppercase hex.
    name_hex: String,
}

#[derive(Clone, Copy)]
enum OnForm {
    Omitted,
    All,
    List,
    Directional,
}

impl OnForm {
    fn as_str(self) -> &'static str {
        match self {
            OnForm::Omitted => "omitted",
            OnForm::All => "all",
            OnForm::List => "list",
            OnForm::Directional => "directional",
        }
    }
}

/// Builds the full `mod __rshooks_entries { .. }` source (entry bodies +
/// forwarding wrappers + `TxType` existence checks + native entries table +
/// entries carrier + handshake back-half).
fn render_entries_module(
    struct_name: &str,
    entries: &[EntrySource<'_>],
    entries_json: &[EntryJson],
) -> Result<String, String> {
    let not_any_list: String = (0..=MAX_INDEX)
        .map(|n| format!("rshooks_entry = \"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");

    let mut mod_body = String::from("use super::*;\n");

    for e in entries_json {
        mod_body.push_str(&render_entry_body_and_wrappers(
            struct_name,
            e,
            &not_any_list,
        ));
    }

    let mut tx_names: BTreeSet<&str> = BTreeSet::new();
    for e in entries {
        match &e.hook.on {
            OnSpec::List(list) => {
                for (n, _) in list {
                    tx_names.insert(n.as_str());
                }
            }
            OnSpec::Directional(inc, out) => {
                for (n, _) in inc.iter().chain(out) {
                    tx_names.insert(n.as_str());
                }
            }
            OnSpec::Omitted | OnSpec::All => {}
        }
        if let Some(list) = &e.hook.can_emit {
            for (n, _) in list {
                tx_names.insert(n.as_str());
            }
        }
    }
    for name in tx_names {
        mod_body.push_str(&format!(
            "const _: ::rshooks::tx_type::TxType = ::rshooks::tx_type::TxType::{name};\n"
        ));
    }

    mod_body.push_str(&render_native_entries_table(struct_name, entries_json));

    let payload = encode_entries_json(struct_name, entries_json)?;
    let payload_hex = hex_upper(&payload);
    let digest = sha256::sha256(&payload);
    let carrier_ident = format!("__rshooks_hooks_{}", hex_lower(&digest));
    mod_body.push_str(&format!(
        "#[cfg(target_arch = \"wasm32\")]\n\
         #[doc(hidden)]\n\
         #[unsafe(export_name = \"{ENTRIES_EXPORT_PREFIX}{payload_hex}\")]\n\
         pub extern \"C\" fn {carrier_ident}(_reserved: u32) -> i64 {{ 0 }}\n"
    ));

    mod_body.push_str(&format!(
        "#[automatically_derived]\n\
         impl ::rshooks::__internal::HookChainImpl for {struct_name} {{}}\n\
         impl {struct_name} {{\n\
             #[doc(hidden)]\n\
             pub const __RSHOOKS_IMPL: () = ();\n\
         }}\n\
         #[doc(hidden)]\n\
         #[allow(dead_code)]\n\
         const __RSHOOKS_LINK: () = {struct_name}::__RSHOOKS_STRUCT;\n"
    ));

    Ok(format!(
        "#[doc(hidden)]\n\
         #[allow(unexpected_cfgs, dead_code)]\n\
         mod __rshooks_entries {{\n{mod_body}\n}}\n"
    ))
}

/// Renders one entry's plain-Rust-ABI body function(s) plus the wasm
/// `extern "C"` wrappers that forward to them — design §2.3's "one entry
/// body, two ABIs": the call expression (`{Struct}::{fn}(&{Struct})`) is
/// built exactly once per body, and both the selected
/// (`export_name = "hook"`/`"cbak"`) and discovery
/// (`export_name = "__rshooks_hook_{i}"`/`"__rshooks_cbak_{i}"`) wrappers
/// are one-line forwards to it. Operates on the Span-free [`EntryJson`]
/// view (plain strings only), so unit tests can pin the exact generated
/// text without a live macro invocation.
fn render_entry_body_and_wrappers(
    struct_name: &str,
    entry: &EntryJson,
    not_any_list: &str,
) -> String {
    let i = entry.index;
    // `&{struct_name}` works identically for both struct forms
    // (HOOKS_SELF_RECEIVER_DESIGN.md §3.2): a named-field struct's
    // generated same-named static, or a unit struct's constructor value.
    //
    // Declared signature parameters (`docs/PARAM_SIGNATURE_DESIGN.md` §1)
    // decode into locals before the call, one `otxn_sig_param` read per
    // argument, passed through in declaration order; empty `sig_params`
    // makes `prologue` empty and `call_args` exactly `&{struct_name}`.
    //
    // The call is always wrapped in `EntryReturn::finish`
    // (TYPED_ENTRY_RESULTS_DESIGN.md §1.3/§7), itself `#[inline(always)]`
    // — it's what makes a `HookResult` return type legal here. When the
    // entry body itself diverges (`accept!`/`rollback!` return `!`), that
    // divergence makes `finish`'s `Ok`/`Err` match dead code once inlined,
    // rather than leaving it as an unreachable-but-compiled match arm.
    let prologue = render_sig_param_prologue(&entry.sig_params);
    let call_args: String = std::iter::once(format!("&{struct_name}"))
        .chain(
            entry
                .sig_params
                .iter()
                .map(|p| format!("__rshooks_sig_{}", p.field)),
        )
        .collect::<Vec<_>>()
        .join(", ");
    let hook_call = format!(
        "::rshooks::exit::EntryReturn::finish({struct_name}::{}({call_args}))",
        entry.hook_fn
    );
    let mut out = format!(
        "#[inline(always)]\n\
         #[doc(hidden)]\n\
         pub fn __rshooks_entry_body_{i}(_reserved: u32) -> i64 {{ {prologue}{hook_call} }}\n\
         #[cfg(rshooks_entry = \"{i}\")]\n\
         #[unsafe(export_name = \"hook\")]\n\
         pub extern \"C\" fn __rshooks_hook_sel_{i}(_reserved: u32) -> i64 {{ \
             __rshooks_entry_body_{i}(_reserved) }}\n\
         #[cfg(all(target_arch = \"wasm32\", not(any({not_any_list}))))]\n\
         #[unsafe(export_name = \"__rshooks_hook_{i}\")]\n\
         pub extern \"C\" fn __rshooks_hook_disc_{i}(_reserved: u32) -> i64 {{ \
             __rshooks_entry_body_{i}(_reserved) }}\n"
    );
    if let Some(cbak_fn) = &entry.cbak_fn {
        let cbak_call = format!(
            "::rshooks::exit::EntryReturn::finish({struct_name}::{cbak_fn}(&{struct_name}))"
        );
        out.push_str(&format!(
            "#[inline(always)]\n\
             #[doc(hidden)]\n\
             pub fn __rshooks_cbak_body_{i}(_reserved: u32) -> i64 {{ {cbak_call} }}\n\
             #[cfg(rshooks_entry = \"{i}\")]\n\
             #[unsafe(export_name = \"cbak\")]\n\
             pub extern \"C\" fn __rshooks_cbak_sel_{i}(_reserved: u32) -> i64 {{ \
                 __rshooks_cbak_body_{i}(_reserved) }}\n\
             #[cfg(all(target_arch = \"wasm32\", not(any({not_any_list}))))]\n\
             #[unsafe(export_name = \"__rshooks_cbak_{i}\")]\n\
             pub extern \"C\" fn __rshooks_cbak_disc_{i}(_reserved: u32) -> i64 {{ \
                 __rshooks_cbak_body_{i}(_reserved) }}\n"
        ));
    }
    out
}

/// Renders the per-argument decode prologue for one entry's declared
/// signature parameters (`docs/PARAM_SIGNATURE_DESIGN.md` §1), in
/// declaration (= wire index) order. Empty input yields the empty string.
///
/// Per argument:
///
/// 1. A monomorphized `const` assert that the type token's own `type_byte`
///    (decided at macro time, purely from the token's shape — see
///    [`classify_sig_type`]) matches `<Ty as SigParamType>::TYPE_BYTE` —
///    catches a type alias resolving to a different wire type.
/// 2. Builds the declared name inside a `const { .. }` block (per
///    `docs/PARAM_SIGNATURE_DESIGN.md` §1's "Generated prologue": every MUST
///    of the wire format is `const`-evaluable, so a malformed name — dead
///    code here, since [`parse_sig_args`] already validated it — would be a
///    compile error, never a runtime panic) and reads it via
///    `otxn_sig_param`.
/// 3. On `Err`, rolls back with `b"rshooks: bad sig param '<name>'"` and the
///    argument's own 0-based index as the rollback code — `rollback!`
///    diverges (`!`), so it coerces to the match's `Ty` arm directly, no
///    separate early-return plumbing needed.
fn render_sig_param_prologue(sig_params: &[SigParamJson]) -> String {
    let mut out = String::new();
    for (idx, p) in sig_params.iter().enumerate() {
        let total_len = 7usize.wrapping_add(p.field.len());
        out.push_str(&format!(
            "const _: () = {{ assert!(<{ty} as ::rshooks::sig::SigParamType>::TYPE_BYTE == \
             {type_byte}u8, \"rshooks: signature parameter '{field}' resolves to a different \
             wire type than its declared type token (check for a type alias mismatch)\"); }};\n\
             let __rshooks_sig_{field}: {ty} = match ::rshooks::sig::otxn_sig_param::<{ty}>(&const {{ \
             ::rshooks::sig::sig_param_name::<{total_len}>({idx}, {type_byte}u8, b\"{field}\") \
             }}) {{\n\
             ::core::result::Result::Ok(__v) => __v,\n\
             ::core::result::Result::Err(_) => ::rshooks::rollback!(\
             b\"rshooks: bad sig param '{field}'\", {idx}i64),\n\
             }};\n",
            ty = p.type_text,
            type_byte = p.type_byte,
            field = p.field,
            total_len = total_len,
            idx = idx,
        ));
    }
    out
}

/// Renders one entry's row in the native `HookChainEntries::ENTRIES` table
/// (design §2.3): `hook` always points at the entry body function emitted
/// by [`render_entry_body_and_wrappers`]; `cbak` is `Some(..)` pointing at
/// the paired cbak body when one was declared, `None` otherwise; `can_emit`
/// preserves [`rshooks::decl::NativeEntry::can_emit`]'s three-state
/// distinction — `None` when `#[hook(.., can_emit = [..])]` was never
/// declared on this entry (unrestricted), `Some(&[..])` (an empty slice
/// included) when it was, rendering each transaction type as an
/// `::rshooks::tx_type::TxType::` path. Span-free — see
/// [`render_entry_body_and_wrappers`]'s doc comment.
fn render_native_entry_row(entry: &EntryJson) -> String {
    let i = entry.index;
    let cbak_expr = if entry.cbak_fn.is_some() {
        format!("Some(__rshooks_cbak_body_{i})")
    } else {
        "None".to_string()
    };
    let can_emit_expr = match &entry.hook_can_emit {
        None => "None".to_string(),
        Some(list) => {
            let items: String = list
                .iter()
                .map(|n| format!("::rshooks::tx_type::TxType::{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("Some(&[{items}])")
        }
    };
    format!(
        "::rshooks::decl::NativeEntry {{\n\
             index: {i},\n\
             name: \"{}\",\n\
             hook: __rshooks_entry_body_{i},\n\
             cbak: {cbak_expr},\n\
             can_emit: {can_emit_expr},\n\
         }},\n",
        entry.hook_fn
    )
}

/// Renders the whole native `impl HookChainEntries for {Struct}` block
/// (design §2.3), unconditional on non-wasm targets — no feature gate:
/// generated code cannot see a downstream crate's Cargo features, per
/// `rshooks::decl::HookChainEntries`'s doc comment. Lives inside the
/// generated `__rshooks_entries` module (`use super::*;`), which is fine —
/// a trait impl is visible crate-wide regardless of the defining module's
/// privacy.
fn render_native_entries_table(struct_name: &str, entries: &[EntryJson]) -> String {
    let rows: String = entries.iter().map(render_native_entry_row).collect();
    format!(
        "#[cfg(not(target_arch = \"wasm32\"))]\n\
         impl ::rshooks::decl::HookChainEntries for {struct_name} {{\n\
             const ENTRIES: &'static [::rshooks::decl::NativeEntry] = &[\n{rows}    ];\n\
         }}\n"
    )
}

/// Encodes the entries carrier JSON payload (contract §B2) from a
/// plain-`String` entry list, sorted by index ascending (the caller already
/// sorted `hooks` before building `entries_json`).
fn encode_entries_json(struct_name: &str, entries: &[EntryJson]) -> Result<Vec<u8>, String> {
    let mut arr = Vec::new();
    for e in entries {
        let mut obj = serde_json::Map::new();
        obj.insert("index".into(), u64::from(e.index).into());
        obj.insert("hook_fn".into(), e.hook_fn.clone().into());
        obj.insert(
            "cbak_fn".into(),
            e.cbak_fn
                .clone()
                .map_or(serde_json::Value::Null, Into::into),
        );
        obj.insert(
            "HookName".into(),
            e.hook_name
                .clone()
                .map_or(serde_json::Value::Null, Into::into),
        );

        let mut on_obj = serde_json::Map::new();
        on_obj.insert("form".into(), e.on_form.as_str().into());
        on_obj.insert("HookOn".into(), names_or_null(e.hook_on.as_ref()));
        on_obj.insert(
            "HookOnIncoming".into(),
            names_or_null(e.hook_on_incoming.as_ref()),
        );
        on_obj.insert(
            "HookOnOutgoing".into(),
            names_or_null(e.hook_on_outgoing.as_ref()),
        );
        obj.insert("on".into(), serde_json::Value::Object(on_obj));

        obj.insert(
            "HookCanEmit".into(),
            names_or_null(e.hook_can_emit.as_ref()),
        );
        obj.insert(
            "description".into(),
            e.description
                .clone()
                .map_or(serde_json::Value::Null, Into::into),
        );

        // Only `field`/`type_byte`/`name_hex` are part of the wire carrier
        // (`docs/PARAM_SIGNATURE_DESIGN.md` §4) — `SigParamJson::type_text`
        // is codegen-only and deliberately not serialized here.
        let sig_params: Vec<serde_json::Value> = e
            .sig_params
            .iter()
            .map(|p| {
                let mut sp = serde_json::Map::new();
                sp.insert("field".into(), p.field.clone().into());
                sp.insert("type_byte".into(), u64::from(p.type_byte).into());
                sp.insert("name_hex".into(), p.name_hex.clone().into());
                serde_json::Value::Object(sp)
            })
            .collect();
        obj.insert("sig_params".into(), sig_params.into());

        arr.push(serde_json::Value::Object(obj));
    }

    let mut object = serde_json::Map::new();
    object.insert("schema".into(), "rshooks-hooks-v2".into());
    object.insert("impl".into(), struct_name.into());
    object.insert("entries".into(), arr.into());

    serde_json::to_vec(&serde_json::Value::Object(object))
        .map_err(|e| format!("#[hooks]: failed to serialize entries carrier JSON: {e}"))
}

fn names_or_null(list: Option<&Vec<String>>) -> serde_json::Value {
    list.map_or(serde_json::Value::Null, |l| l.to_vec().into())
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::indexing_slicing)] // tests are exempt, docs/DESIGN.md §8
    use super::*;

    // --- F6: `first_empty_tx_entry` ---

    #[test]
    fn empty_tx_entry_accepts_empty_list() {
        assert_eq!(first_empty_tx_entry(&[]), None);
    }

    #[test]
    fn empty_tx_entry_accepts_single_entry() {
        // `[Payment]`
        assert_eq!(first_empty_tx_entry(&[false]), None);
    }

    #[test]
    fn empty_tx_entry_accepts_single_trailing_comma() {
        // `[Payment,]`
        assert_eq!(first_empty_tx_entry(&[false, true]), None);
    }

    #[test]
    fn empty_tx_entry_accepts_multiple_entries_with_trailing_comma() {
        // `[Payment, Invoke,]`
        assert_eq!(first_empty_tx_entry(&[false, true, false, true]), None);
    }

    #[test]
    fn empty_tx_entry_rejects_consecutive_commas_mid_list() {
        // `[Payment,,Invoke]`
        assert_eq!(first_empty_tx_entry(&[false, true, true, false]), Some(2));
    }

    #[test]
    fn empty_tx_entry_rejects_lone_comma() {
        // `[,]`
        assert_eq!(first_empty_tx_entry(&[true]), Some(0));
    }

    #[test]
    fn empty_tx_entry_rejects_double_comma() {
        // `[,,]`
        assert_eq!(first_empty_tx_entry(&[true, true]), Some(0));
    }

    #[test]
    fn empty_tx_entry_rejects_leading_comma_before_entry() {
        // `[,Payment]`
        assert_eq!(first_empty_tx_entry(&[true, false]), Some(0));
    }

    // --- F7: `classify_fn_qualifiers` ---

    #[test]
    fn fn_qualifiers_plain_fn_has_no_qualifiers() {
        assert_eq!(classify_fn_qualifiers(&["fn"]), Some((0, false)));
    }

    #[test]
    fn fn_qualifiers_const_fn_is_a_qualified_fn_not_a_const_item() {
        assert_eq!(classify_fn_qualifiers(&["const", "fn"]), Some((1, true)));
    }

    #[test]
    fn fn_qualifiers_async_fn() {
        assert_eq!(classify_fn_qualifiers(&["async", "fn"]), Some((1, true)));
    }

    #[test]
    fn fn_qualifiers_unsafe_fn() {
        assert_eq!(classify_fn_qualifiers(&["unsafe", "fn"]), Some((1, true)));
    }

    #[test]
    fn fn_qualifiers_extern_abi_fn() {
        // `extern "C" fn`
        assert_eq!(
            classify_fn_qualifiers(&["extern", "<lit>", "fn"]),
            Some((2, true))
        );
    }

    #[test]
    fn fn_qualifiers_unsafe_extern_abi_fn() {
        // `unsafe extern "C" fn`
        assert_eq!(
            classify_fn_qualifiers(&["unsafe", "extern", "<lit>", "fn"]),
            Some((3, true))
        );
    }

    #[test]
    fn fn_qualifiers_plain_const_item_is_not_a_fn() {
        // `const NAME: Ty = ..;` — `const` is not followed by `fn`.
        assert_eq!(classify_fn_qualifiers(&["const", "<other>"]), None);
    }

    #[test]
    fn fn_qualifiers_non_fn_item_is_not_classified() {
        assert_eq!(classify_fn_qualifiers(&["type"]), None);
    }

    // --- `classify_receiver_kinds` (HOOKS_SELF_RECEIVER_DESIGN.md §3.1/§6.2) ---

    #[test]
    fn receiver_no_receiver_ordinary_param() {
        // `fn f(x: i64)`
        assert_eq!(classify_receiver_kinds(&["<ident>", "<other>"]), None);
    }

    #[test]
    fn receiver_no_receiver_empty_args() {
        assert_eq!(classify_receiver_kinds(&[]), None);
    }

    #[test]
    fn receiver_mut_binding_that_is_not_self() {
        // `fn f(mut x: i64)` — `mut` not followed by `self`.
        assert_eq!(classify_receiver_kinds(&["mut", "<ident>"]), None);
    }

    #[test]
    fn receiver_bare_self_is_rejected() {
        // `fn f(self)`
        assert_eq!(
            classify_receiver_kinds(&["self"]),
            Some((0, ReceiverClass::Rejected))
        );
    }

    #[test]
    fn receiver_self_with_type_ascription_is_rejected() {
        // `fn f(self: Box<Self>)` — the ascribed type isn't inspected.
        assert_eq!(
            classify_receiver_kinds(&["self", "<other>"]),
            Some((0, ReceiverClass::Rejected))
        );
    }

    #[test]
    fn receiver_mut_self_is_rejected() {
        // `fn f(mut self)`
        assert_eq!(
            classify_receiver_kinds(&["mut", "self"]),
            Some((1, ReceiverClass::Rejected))
        );
    }

    #[test]
    fn receiver_shared_self_is_accepted() {
        // `fn f(&self)`
        assert_eq!(
            classify_receiver_kinds(&["&", "self"]),
            Some((1, ReceiverClass::SharedSelf))
        );
    }

    #[test]
    fn receiver_mut_ref_self_is_rejected_as_mut() {
        // `fn f(&mut self)`
        assert_eq!(
            classify_receiver_kinds(&["&", "mut", "self"]),
            Some((2, ReceiverClass::MutSelf))
        );
    }

    #[test]
    fn receiver_lifetime_self_is_rejected_as_general() {
        // `fn f(&'a self)` — only bare `&self` is accepted.
        assert_eq!(
            classify_receiver_kinds(&["&", "'", "<ident>", "self"]),
            Some((3, ReceiverClass::Rejected))
        );
    }

    #[test]
    fn receiver_lifetime_mut_self_is_rejected_as_mut() {
        // `fn f(&'a mut self)`
        assert_eq!(
            classify_receiver_kinds(&["&", "'", "<ident>", "mut", "self"]),
            Some((4, ReceiverClass::MutSelf))
        );
    }

    #[test]
    fn receiver_ref_to_non_self_is_not_a_receiver() {
        // A leading `&` not followed by a self shape (not valid as a real
        // first parameter, but the classifier must not misclassify it).
        assert_eq!(classify_receiver_kinds(&["&", "<ident>"]), None);
    }

    // --- `classify_receiver_prefix` (HOOKS_SELF_RECEIVER_DESIGN.md §3.3, R1) ---

    #[test]
    fn receiver_prefix_plain_forms_unchanged() {
        // `fn f(&self)` — no leading attribute at all.
        assert_eq!(
            classify_receiver_prefix(&["&", "self"]),
            (0, false, Some((1, ReceiverClass::SharedSelf)))
        );
    }

    #[test]
    fn receiver_prefix_attributed_shared_self() {
        // `fn f(#[allow(unused)] &self)`
        assert_eq!(
            classify_receiver_prefix(&["attr", "&", "self"]),
            (1, false, Some((1, ReceiverClass::SharedSelf)))
        );
    }

    #[test]
    fn receiver_prefix_attributed_mut_self() {
        // `fn f(#[allow(unused)] &mut self)` — must still hit the MutSelf
        // rejection, not silently classify as "no receiver".
        assert_eq!(
            classify_receiver_prefix(&["attr", "&", "mut", "self"]),
            (1, false, Some((2, ReceiverClass::MutSelf)))
        );
    }

    #[test]
    fn receiver_prefix_multiple_attributes_are_all_skipped() {
        // `fn f(#[allow(unused)] #[doc = "x"] &self)`
        assert_eq!(
            classify_receiver_prefix(&["attr", "attr", "&", "self"]),
            (2, false, Some((1, ReceiverClass::SharedSelf)))
        );
    }

    #[test]
    fn receiver_prefix_cfg_attr_on_receiver_is_flagged() {
        // `fn f(#[cfg(feature = "x")] &self)` — the caller rejects this;
        // the classifier just surfaces the fact for it.
        assert_eq!(
            classify_receiver_prefix(&["cfg_attr", "&", "self"]),
            (1, true, Some((1, ReceiverClass::SharedSelf)))
        );
    }

    #[test]
    fn receiver_prefix_cfg_attr_among_others_is_still_flagged() {
        assert_eq!(
            classify_receiver_prefix(&["attr", "cfg_attr", "&", "mut", "self"]),
            (2, true, Some((2, ReceiverClass::MutSelf)))
        );
    }

    #[test]
    fn receiver_prefix_attributed_ordinary_param_is_not_a_receiver() {
        // `fn f(#[allow(unused)] x: i64)` — attributes precede a param
        // that isn't `self` at all; not a receiver, cfg-ness irrelevant.
        assert_eq!(
            classify_receiver_prefix(&["attr", "<ident>", "<other>"]),
            (1, false, None)
        );
    }

    fn sample() -> EntryJson {
        EntryJson {
            index: 0,
            hook_fn: "deposit".to_string(),
            cbak_fn: None,
            hook_name: Some("deposit".to_string()),
            on_form: OnForm::List,
            hook_on: Some(vec!["Payment".to_string()]),
            hook_on_incoming: None,
            hook_on_outgoing: None,
            hook_can_emit: Some(Vec::new()),
            description: Some("a \"quoted\" desc".to_string()),
            sig_params: Vec::new(),
        }
    }

    #[test]
    fn entries_json_is_deterministic() {
        let a = encode_entries_json("Vault", &[sample()]).expect("json");
        let b = encode_entries_json("Vault", &[sample()]).expect("json");
        assert_eq!(a, b);
    }

    #[test]
    fn entries_json_escapes_and_shape() {
        let bytes = encode_entries_json("Vault", &[sample()]).expect("json");
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(text.contains("a \\\"quoted\\\" desc"));
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["schema"], "rshooks-hooks-v2");
        assert_eq!(value["impl"], "Vault");
        let entry = &value["entries"][0];
        assert_eq!(entry["index"], 0);
        assert_eq!(entry["hook_fn"], "deposit");
        assert!(entry["cbak_fn"].is_null());
        assert_eq!(entry["on"]["form"], "list");
        assert_eq!(entry["on"]["HookOn"], serde_json::json!(["Payment"]));
        assert!(entry["on"]["HookOnIncoming"].is_null());
        // declared-empty can_emit must serialize as `[]`, not `null`.
        assert_eq!(entry["HookCanEmit"], serde_json::json!([]));
    }

    #[test]
    fn entries_json_three_state_can_emit() {
        let mut omitted = sample();
        omitted.hook_can_emit = None;
        let bytes = encode_entries_json("Vault", &[omitted]).expect("json");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        assert!(value["entries"][0]["HookCanEmit"].is_null());
    }

    #[test]
    fn entries_json_on_all_has_no_lists() {
        let mut e = sample();
        e.on_form = OnForm::All;
        e.hook_on = None;
        let bytes = encode_entries_json("Vault", &[e]).expect("json");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        assert_eq!(value["entries"][0]["on"]["form"], "all");
        assert!(value["entries"][0]["on"]["HookOn"].is_null());
    }

    #[test]
    fn not_any_list_covers_the_fixed_0_to_9_domain() {
        let not_any_list: String = (0..=MAX_INDEX)
            .map(|n| format!("rshooks_entry = \"{n}\""))
            .collect::<Vec<_>>()
            .join(", ");
        for n in 0..=MAX_INDEX {
            assert!(not_any_list.contains(&format!("rshooks_entry = \"{n}\"")));
        }
        assert_eq!(not_any_list.matches("rshooks_entry").count(), 10);
    }

    #[test]
    fn on_form_wire_spellings() {
        assert_eq!(OnForm::Omitted.as_str(), "omitted");
        assert_eq!(OnForm::All.as_str(), "all");
        assert_eq!(OnForm::List.as_str(), "list");
        assert_eq!(OnForm::Directional.as_str(), "directional");
    }

    // --- `render_entry_body_and_wrappers` / `render_native_entry_row`
    // / `render_native_entries_table` — the "one entry body, two ABIs" shape
    // (TESTENV_DESIGN.md §2.3) ---

    fn not_any_list() -> String {
        (0..=MAX_INDEX)
            .map(|n| format!("rshooks_entry = \"{n}\""))
            .collect::<Vec<_>>()
            .join(", ")
    }

    #[test]
    fn entry_body_is_emitted_exactly_once_and_wrappers_forward() {
        let entry = sample(); // index 0, hook_fn "deposit", no cbak
        let out = render_entry_body_and_wrappers("Vault", &entry, &not_any_list());

        // The call expression appears exactly once — in the body fn, never
        // duplicated into either wrapper — wrapped in `EntryReturn::finish`
        // unconditionally (TYPED_ENTRY_RESULTS_DESIGN.md §1.3).
        assert_eq!(
            out.matches("::rshooks::exit::EntryReturn::finish(Vault::deposit(&Vault))")
                .count(),
            1
        );
        assert_eq!(out.matches("pub fn __rshooks_entry_body_0").count(), 1);

        // Both wrappers are one-line forwards to the body fn, not
        // reimplementations of the call.
        assert!(out.contains(
            "pub extern \"C\" fn __rshooks_hook_sel_0(_reserved: u32) -> i64 { \
             __rshooks_entry_body_0(_reserved) }"
        ));
        assert!(out.contains(
            "pub extern \"C\" fn __rshooks_hook_disc_0(_reserved: u32) -> i64 { \
             __rshooks_entry_body_0(_reserved) }"
        ));
        assert!(out.contains("#[cfg(rshooks_entry = \"0\")]"));
        assert!(out.contains("#[unsafe(export_name = \"hook\")]"));
        assert!(out.contains("#[unsafe(export_name = \"__rshooks_hook_0\")]"));

        // The discovery wrapper carries a crate-global export symbol, so it
        // is wasm-only: on native targets two chains in one binary must not
        // collide, and no extern "C" frame may sit in the native call path
        // (unwind safety, design §2.2/§2.3).
        assert!(out.contains("#[cfg(all(target_arch = \"wasm32\", not(any(rshooks_entry = \"0\""));

        // No cbak declared: no cbak body or wrapper text at all.
        assert!(!out.contains("cbak"));
    }

    #[test]
    fn entry_without_cbak_emits_no_cbak_text() {
        let mut entry = sample();
        entry.cbak_fn = None;
        let out = render_entry_body_and_wrappers("Vault", &entry, &not_any_list());
        assert!(!out.contains("__rshooks_cbak_body_0"));
    }

    #[test]
    fn cbak_body_is_emitted_exactly_once_and_wrappers_forward() {
        let mut entry = sample();
        entry.cbak_fn = Some("deposit_cbak".to_string());
        let out = render_entry_body_and_wrappers("Vault", &entry, &not_any_list());

        assert_eq!(
            out.matches("::rshooks::exit::EntryReturn::finish(Vault::deposit_cbak(&Vault))")
                .count(),
            1
        );
        assert_eq!(out.matches("pub fn __rshooks_cbak_body_0").count(), 1);
        assert!(out.contains(
            "pub extern \"C\" fn __rshooks_cbak_sel_0(_reserved: u32) -> i64 { \
             __rshooks_cbak_body_0(_reserved) }"
        ));
        assert!(out.contains(
            "pub extern \"C\" fn __rshooks_cbak_disc_0(_reserved: u32) -> i64 { \
             __rshooks_cbak_body_0(_reserved) }"
        ));
        assert!(out.contains("#[unsafe(export_name = \"cbak\")]"));
        assert!(out.contains("#[unsafe(export_name = \"__rshooks_cbak_0\")]"));
    }

    #[test]
    fn native_entry_row_without_cbak_or_can_emit() {
        let mut entry = sample();
        entry.hook_can_emit = None;
        let row = render_native_entry_row(&entry);
        assert!(row.contains("index: 0,"));
        assert!(row.contains("name: \"deposit\","));
        assert!(row.contains("hook: __rshooks_entry_body_0,"));
        assert!(row.contains("cbak: None,"));
        // `can_emit` omitted entirely (never declared) renders `None` —
        // unrestricted, distinct from a declared-empty list.
        assert!(row.contains("can_emit: None,"));
    }

    #[test]
    fn native_entry_row_declared_empty_can_emit_renders_some_empty_slice() {
        // `can_emit = []` (declared, but empty) is deny-all, and must
        // render distinctly from `can_emit` being absent altogether — see
        // `NativeEntry::can_emit`'s doc comment for the three-state
        // contract this pins.
        let entry = sample(); // hook_can_emit: Some(vec![])
        let row = render_native_entry_row(&entry);
        assert!(row.contains("can_emit: Some(&[]),"));
    }

    #[test]
    fn native_entry_row_with_cbak_and_can_emit() {
        let mut entry = sample();
        entry.cbak_fn = Some("deposit_cbak".to_string());
        entry.hook_can_emit = Some(vec!["Payment".to_string(), "Invoke".to_string()]);
        let row = render_native_entry_row(&entry);
        assert!(row.contains("cbak: Some(__rshooks_cbak_body_0),"));
        assert!(row.contains(
            "can_emit: Some(&[::rshooks::tx_type::TxType::Payment, \
             ::rshooks::tx_type::TxType::Invoke]),"
        ));
    }

    #[test]
    fn native_entries_table_has_one_row_per_entry_gated_off_wasm() {
        let mut e0 = sample();
        e0.cbak_fn = Some("deposit_cbak".to_string());
        let mut e1 = sample();
        e1.index = 1;
        e1.hook_fn = "withdraw".to_string();
        e1.cbak_fn = None;
        e1.hook_can_emit = None;

        let table = render_native_entries_table("Vault", &[e0, e1]);
        assert!(table.starts_with("#[cfg(not(target_arch = \"wasm32\"))]\n"));
        assert!(table.contains("impl ::rshooks::decl::HookChainEntries for Vault {"));
        assert!(table.contains("const ENTRIES: &'static [::rshooks::decl::NativeEntry] = &[\n"));
        assert_eq!(table.matches("::rshooks::decl::NativeEntry {").count(), 2);
        assert!(table.contains("name: \"deposit\","));
        assert!(table.contains("name: \"withdraw\","));
        assert!(table.contains("cbak: Some(__rshooks_cbak_body_0),"));
        assert!(table.contains("cbak: None,"));
    }

    // --- Signature parameters (`docs/PARAM_SIGNATURE_DESIGN.md` §1) ---

    #[test]
    fn sig_arg_name_accepts_the_interface_charset() {
        assert!(is_valid_sig_arg_name("account"));
        assert!(is_valid_sig_arg_name("count"));
        assert!(is_valid_sig_arg_name("a"));
        assert!(is_valid_sig_arg_name("abcdefghijklmnop")); // exactly 16
        assert!(is_valid_sig_arg_name("A1"));
    }

    #[test]
    fn sig_arg_name_rejects_underscore_digit_start_and_overlength() {
        assert!(!is_valid_sig_arg_name("my_count")); // underscore
        assert!(!is_valid_sig_arg_name("_count")); // leading underscore
        assert!(!is_valid_sig_arg_name("1count")); // leading digit
        assert!(!is_valid_sig_arg_name("")); // empty
        assert!(!is_valid_sig_arg_name("abcdefghijklmnopq")); // 17 bytes
    }

    #[test]
    fn sig_type_text_covers_every_declared_table_entry() {
        // docs/PARAM_SIGNATURE_DESIGN.md §2's table, minus `Blob<N>` (needs
        // real tokens — see `classify_sig_type`, exercised via the trybuild
        // pass fixture instead).
        assert_eq!(classify_sig_type_text("u8"), Some(0x10));
        assert_eq!(classify_sig_type_text("u16"), Some(0x01));
        assert_eq!(classify_sig_type_text("u32"), Some(0x02));
        assert_eq!(classify_sig_type_text("u64"), Some(0x03));
        assert_eq!(classify_sig_type_text("[u8;16]"), Some(0x04));
        assert_eq!(classify_sig_type_text("[u8;32]"), Some(0x05));
        assert_eq!(classify_sig_type_text("Hash"), Some(0x05));
        assert_eq!(classify_sig_type_text("AmountBytes"), Some(0x06));
        assert_eq!(classify_sig_type_text("AccountId"), Some(0x08));
        assert_eq!(classify_sig_type_text("[u8;20]"), Some(0x11));
        assert_eq!(classify_sig_type_text("IssueBytes"), Some(0x18));
        assert_eq!(classify_sig_type_text("CurrencyCode"), Some(0x1A));
    }

    #[test]
    fn sig_type_text_rejects_unsupported_type() {
        assert_eq!(classify_sig_type_text("i64"), None);
        assert_eq!(classify_sig_type_text("String"), None);
        assert_eq!(classify_sig_type_text("Blob<32>"), None); // Blob needs real tokens
    }

    #[test]
    fn sig_param_name_hex_matches_the_wire_vector() {
        // Same vector as `rshooks::sig::sig_param_name`'s own doctest:
        // account(0), STI_ACCOUNT (0x08), 7-byte name.
        assert_eq!(
            sig_param_name_hex(0, 0x08, "account"),
            "5F5053000008076163636F756E74"
        );
        // count(1), STI_UINT16 (0x01), 5-byte name.
        assert_eq!(
            sig_param_name_hex(1, 0x01, "count"),
            "5F505300010105636F756E74"
        );
    }

    fn sig_param(field: &str, type_text: &str, type_byte: u8, index: u8) -> SigParamJson {
        SigParamJson {
            field: field.to_string(),
            type_text: type_text.to_string(),
            type_byte,
            name_hex: sig_param_name_hex(index, type_byte, field),
        }
    }

    #[test]
    fn entries_json_includes_sig_params_field_type_byte_and_name_hex_only() {
        let mut entry = sample();
        entry.sig_params = vec![
            sig_param("account", "AccountId", 0x08, 0),
            sig_param("count", "u16", 0x01, 1),
        ];
        let bytes = encode_entries_json("Vault", &[entry]).expect("json");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        let sig_params = value["entries"][0]["sig_params"]
            .as_array()
            .expect("sig_params array");
        assert_eq!(sig_params.len(), 2);
        assert_eq!(sig_params[0]["field"], "account");
        assert_eq!(sig_params[0]["type_byte"], 0x08);
        assert_eq!(sig_params[0]["name_hex"], "5F5053000008076163636F756E74");
        assert_eq!(sig_params[1]["field"], "count");
        assert_eq!(sig_params[1]["type_byte"], 0x01);
        // `type_text` is codegen-only and must never leak into the carrier.
        assert!(sig_params[0].get("type_text").is_none());
    }

    #[test]
    fn entries_json_sig_params_empty_by_default() {
        let bytes = encode_entries_json("Vault", &[sample()]).expect("json");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        assert_eq!(value["entries"][0]["sig_params"], serde_json::json!([]));
    }

    #[test]
    fn no_sig_params_prologue_is_empty() {
        assert_eq!(render_sig_param_prologue(&[]), "");
    }

    #[test]
    fn entry_body_with_sig_params_decodes_before_the_call_in_declaration_order() {
        let mut entry = sample(); // index 0, hook_fn "deposit"
        entry.sig_params = vec![
            sig_param("account", "AccountId", 0x08, 0),
            sig_param("count", "u16", 0x01, 1),
        ];
        let out = render_entry_body_and_wrappers("Vault", &entry, &not_any_list());

        // The call passes decoded locals through, in declaration order,
        // after `&Vault`.
        assert!(out.contains(
            "::rshooks::exit::EntryReturn::finish(Vault::deposit(&Vault, __rshooks_sig_account, \
             __rshooks_sig_count))"
        ));

        // Each argument's prologue: the type-byte const assert, the
        // `const { sig_param_name::<N>(..) }` name build, and the
        // documented rollback message/code on `Err`.
        assert!(out.contains("<AccountId as ::rshooks::sig::SigParamType>::TYPE_BYTE == 8u8"));
        assert!(out.contains("::rshooks::sig::sig_param_name::<14>(0, 8u8, b\"account\")"));
        assert!(out.contains("b\"rshooks: bad sig param 'account'\", 0i64"));

        assert!(out.contains("<u16 as ::rshooks::sig::SigParamType>::TYPE_BYTE == 1u8"));
        assert!(out.contains("::rshooks::sig::sig_param_name::<12>(1, 1u8, b\"count\")"));
        assert!(out.contains("b\"rshooks: bad sig param 'count'\", 1i64"));

        // The account decode's prologue textually precedes count's, which
        // in turn precedes the call — declaration order is preserved.
        let account_pos = out
            .find("__rshooks_sig_account:")
            .expect("account decode present");
        let count_pos = out
            .find("__rshooks_sig_count:")
            .expect("count decode present");
        let call_pos = out
            .find("EntryReturn::finish(Vault::deposit")
            .expect("call present");
        assert!(account_pos < count_pos);
        assert!(count_pos < call_pos);
    }

    #[test]
    fn entry_body_without_sig_params_is_byte_identical_to_legacy_call_shape() {
        // Parity requirement (PARAM_SIGNATURE_DESIGN.md §1): a no-arg entry
        // must generate exactly the pre-signature-params call shape.
        let out = render_entry_body_and_wrappers("Vault", &sample(), &not_any_list());
        assert!(out.contains("::rshooks::exit::EntryReturn::finish(Vault::deposit(&Vault))"));
        assert!(!out.contains("__rshooks_sig_"));
        assert!(!out.contains("otxn_sig_param"));
    }
}
