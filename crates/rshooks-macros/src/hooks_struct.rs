//! `#[hooks]` on a struct — the chain-declaration half of the multi-hook
//! attribute macro pair. See `docs/MULTI_HOOK_STRUCT_DESIGN.md` §5.1/§5.4
//! and the v0.2 implementation contract §B1 for the normative grammar and
//! generated-output shape; this module implements it.
//!
//! A `#[hooks]` struct is a **declaration container**, never a runtime
//! value with real fields (see the design doc §4.3): every declared field is
//! a zero-sized handle (`State<V>` / `HookParam<V>` / `OtxnParam<V>`) whose
//! *type* the macro rewrites to carry a field-unique marker (§5.4) so that
//! two fields sharing a value type still get distinct, statically-checked
//! accessors.
//!
//! Declared fields are grouped by kind into per-kind namespace structs, and
//! the outer struct holds one field per kind that has at least one declared
//! entry: `{Struct}State` for `#[state]` fields (`self.state.<field>`),
//! `{Struct}HookParams` for `#[hook_param]` fields
//! (`self.hook_param.<field>`), and `{Struct}OtxnParams` for
//! `#[otxn_param]` fields (`self.otxn_param.<field>`). A kind with no
//! declared fields contributes no namespace struct and no outer field.
//!
//! # Why hand-rolled, not `syn`/`quote`
//!
//! The accepted struct/field shape is small and fixed (§5.1's shape
//! table), so a single bounded-lookahead pass over the token buffer is
//! enough. [`crate::shape`] is not reused here because it derives structs
//! meant to be read back (`FromBytes`/`ToBytes` on real fields); this
//! module's fields carry no bytes at all and its field grammar
//! (attribute-driven key/name specs) is materially different.

use proc_macro::{Delimiter, Ident, Span, TokenStream, TokenTree};

use crate::hooks_shared::{
    AttrEntry, is_punct, parse_attr_entries, parse_balanced_angle, parse_byte_string_value,
    parse_string_value, split_top_level_commas, step_angle_depth, to_upper_camel,
};
#[cfg(feature = "unstable-state-interface")]
use crate::hooks_shared::{classify_fixed_sti_type_text, is_valid_interface_name};
use crate::shape::tokens_to_string;
use crate::{err, sha256};

/// Wasm export-name prefix for the struct ("chain declaration") carrier —
/// see contract §B1 item 6.
const CHAIN_EXPORT_PREFIX: &str = "__rshooks_chain_v2_";

/// Entry point for `#[hooks]` applied to a `struct` item, dispatched from
/// [`crate::hooks`].
pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let description = match parse_struct_attr(attr) {
        Ok(d) => d,
        Err(e) => return e,
    };

    let parsed = match parse_struct_item(item) {
        Ok(p) => p,
        Err(e) => return e,
    };

    generate(&parsed, description.as_deref())
}

/// `#[hooks(description = "...")]`'s argument grammar: optional
/// `description = "<str>"`, nothing else.
fn parse_struct_attr(attr: TokenStream) -> Result<Option<String>, TokenStream> {
    let mac = "#[hooks]";
    let tokens: Vec<TokenTree> = attr.into_iter().collect();
    let entries = parse_attr_entries(&tokens, mac)?;

    let mut description = None;
    for AttrEntry {
        key,
        key_span,
        value,
    } in entries
    {
        match key.as_str() {
            "description" => {
                if description.is_some() {
                    return Err(err(key_span, "#[hooks]: duplicate `description`"));
                }
                description = Some(parse_string_value(
                    value.as_deref(),
                    key_span,
                    mac,
                    "description",
                )?);
            }
            other => {
                return Err(err(
                    key_span,
                    &format!(
                        "#[hooks]: unknown struct attribute `{other}` \
                         (only `description = \"...\"` is accepted here)"
                    ),
                ));
            }
        }
    }
    Ok(description)
}

/// One accepted struct shape (§5.1's shape table): a non-generic unit
/// struct, or a non-generic named-field struct (empty `{}` allowed).
enum StructBody {
    Unit,
    Named(Vec<ParsedField>),
}

struct ParsedStruct {
    /// Leading attributes on the struct item, preserved verbatim (doc
    /// comments, etc. — never the consumed `#[hooks(..)]` itself, which
    /// arrives as the separate `attr` parameter, not part of `item`).
    leading_attrs: Vec<TokenTree>,
    vis: Vec<TokenTree>,
    name: Ident,
    body: StructBody,
}

struct ParsedField {
    /// Non-consumed attributes (doc comments, ...), preserved verbatim.
    other_attrs: Vec<TokenTree>,
    vis: Vec<TokenTree>,
    name: Ident,
    /// The `V` tokens from `State<V>`/`HookParam<V>`/`OtxnParam<V>`,
    /// preserved verbatim (span-preserving — the one exception to
    /// "rewrite nothing" is the surrounding wrapper, never this).
    value_ty: Vec<TokenTree>,
    decl: FieldDecl,
}

enum FieldDecl {
    State {
        key: KeySpec,
    },
    Param {
        param_kind: ParamKind,
        spec: ParamSpecDecl,
    },
    /// `#[state_interface(id = .., key(..), value(..))]` — gated behind the
    /// `unstable-state-interface` feature; see
    /// `docs/STATE_INTERFACE_DESIGN.md`. Lives in the same `State`
    /// namespace as an ordinary `#[state]` field. Never constructed when
    /// the feature is off (`parse_state_interface_decl`, its only
    /// constructor, is itself feature-gated) — `allow(dead_code)` in that
    /// configuration instead of contorting the feature-gate rejection into
    /// an always-taken-when-off `if`, the way `hooks_impl.rs`'s
    /// `SIG_FEATURE_GATE_MSG` check does, since there is no comparable
    /// data-dependent condition here to hang that `if` on.
    #[cfg_attr(not(feature = "unstable-state-interface"), allow(dead_code))]
    StateInterface {
        id: u8,
        id_span: Span,
        key_fields: Vec<SiFieldSpec>,
        value_fields: Vec<SiFieldSpec>,
    },
}

/// One `name: Type` entry in a `#[state_interface(..)]` `key(..)`/`value(..)`
/// list, already classified against the version-0 fixed-width type table
/// (`docs/STATE_INTERFACE_DESIGN.md` §1.5).
struct SiFieldSpec {
    name: String,
    /// The type token(s) exactly as written, for splicing into the
    /// generated value struct's field and the marker's key-encoding code.
    ty_tokens: Vec<TokenTree>,
    type_byte: u8,
    width: usize,
}

/// The three per-kind namespaces a declared field is grouped into on the
/// generated outer struct (module doc comment above).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Namespace {
    State,
    HookParam,
    OtxnParam,
}

impl Namespace {
    /// The namespace struct's name suffix: `{Struct}{suffix}`.
    fn struct_suffix(self) -> &'static str {
        match self {
            Namespace::State => "State",
            Namespace::HookParam => "HookParams",
            Namespace::OtxnParam => "OtxnParams",
        }
    }

    /// The outer struct's field name for this namespace.
    fn field_name(self) -> &'static str {
        match self {
            Namespace::State => "state",
            Namespace::HookParam => "hook_param",
            Namespace::OtxnParam => "otxn_param",
        }
    }

    /// The field-level attribute name that routes a field into this
    /// namespace (`#[state]` / `#[hook_param]` / `#[otxn_param]`). Sourced
    /// separately from [`Namespace::field_name`] so generated doc comments
    /// name the attribute, not the outer struct's field.
    fn attr_name(self) -> &'static str {
        match self {
            Namespace::State => "state",
            Namespace::HookParam => "hook_param",
            Namespace::OtxnParam => "otxn_param",
        }
    }

    /// All namespaces, in the fixed order they appear on the outer struct.
    const ALL: [Namespace; 3] = [Namespace::State, Namespace::HookParam, Namespace::OtxnParam];
}

impl FieldDecl {
    fn namespace(&self) -> Namespace {
        match self {
            FieldDecl::State { .. } | FieldDecl::StateInterface { .. } => Namespace::State,
            FieldDecl::Param {
                param_kind: ParamKind::HookParam,
                ..
            } => Namespace::HookParam,
            FieldDecl::Param {
                param_kind: ParamKind::OtxnParam,
                ..
            } => Namespace::OtxnParam,
        }
    }
}

#[derive(Clone, Copy)]
enum ParamKind {
    HookParam,
    OtxnParam,
}

impl ParamKind {
    fn wrapper(self) -> &'static str {
        match self {
            ParamKind::HookParam => "HookParam",
            ParamKind::OtxnParam => "OtxnParam",
        }
    }
    fn attr_name(self) -> &'static str {
        match self {
            ParamKind::HookParam => "hook_param",
            ParamKind::OtxnParam => "otxn_param",
        }
    }
}

enum KeySpec {
    /// `#[state(key = <expr>)]` — `KeyArgs = ()`.
    Const { expr: Vec<TokenTree> },
    /// `#[state(key_by = <TypePath>)]` — `KeyArgs = TypePath`.
    Keyed { ty: Vec<TokenTree> },
}

enum NameSpec {
    /// `name = <byte-string literal>` — `NameArgs = ()`.
    Literal { literal: TokenTree },
    /// `name_by = <TypePath>` — `NameArgs = TypePath`.
    Family { ty: Vec<TokenTree> },
}

struct ParamSpecDecl {
    name: NameSpec,
    required: bool,
    default: Option<Vec<TokenTree>>,
}

/// Parses the struct item's tokens into a [`ParsedStruct`], rejecting every
/// non-accepted shape from §5.1's table with a dedicated diagnostic.
fn parse_struct_item(item: TokenStream) -> Result<ParsedStruct, TokenStream> {
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
            _ => {
                return Err(err(
                    Span::call_site(),
                    "malformed attribute before `struct`",
                ));
            }
        }
        i = i.wrapping_add(2);
    }

    let mut vis = Vec::new();
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

    match tokens.get(i) {
        Some(TokenTree::Ident(id)) if id.to_string() == "struct" => {}
        Some(other) => {
            return Err(err(
                other.span(),
                "#[hooks]: expected a `struct` item here (this attribute form only \
                 applies to a chain-declaration struct)",
            ));
        }
        None => return Err(err(Span::call_site(), "#[hooks]: expected a `struct` item")),
    }
    i = i.wrapping_add(1);

    let name = match tokens.get(i) {
        Some(TokenTree::Ident(id)) => id.clone(),
        Some(other) => return Err(err(other.span(), "#[hooks]: expected a struct name")),
        None => return Err(err(Span::call_site(), "#[hooks]: expected a struct name")),
    };
    i = i.wrapping_add(1);

    if let Some(tt) = tokens.get(i) {
        if is_punct(tt, '<') {
            return Err(err(
                tt.span(),
                "#[hooks]: a chain-declaration struct cannot be generic \
                 (no type parameters or lifetimes)",
            ));
        }
    }
    if let Some(TokenTree::Ident(id)) = tokens.get(i) {
        if id.to_string() == "where" {
            return Err(err(
                id.span(),
                "#[hooks]: a chain-declaration struct cannot have a `where` clause",
            ));
        }
    }

    let body = match tokens.get(i) {
        Some(tt) if is_punct(tt, ';') => {
            if let Some(extra) = tokens.get(i.wrapping_add(1)) {
                return Err(err(
                    extra.span(),
                    "#[hooks]: unexpected tokens after the unit struct",
                ));
            }
            let _ = tt;
            StructBody::Unit
        }
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Brace => {
            if let Some(extra) = tokens.get(i.wrapping_add(1)) {
                return Err(err(
                    extra.span(),
                    "#[hooks]: unexpected tokens after the struct body",
                ));
            }
            StructBody::Named(parse_named_fields(g.stream())?)
        }
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
            return Err(err(
                g.span(),
                "#[hooks]: tuple structs are not accepted — use a named-field struct \
                 (or a unit struct `struct X;` if it declares nothing)",
            ));
        }
        Some(other) => {
            return Err(err(
                other.span(),
                "#[hooks]: expected `;` or a `{ .. }` field list after the struct name",
            ));
        }
        None => {
            return Err(err(
                name.span(),
                "#[hooks]: expected `;` or a `{ .. }` field list after the struct name",
            ));
        }
    };

    Ok(ParsedStruct {
        leading_attrs,
        vis,
        name,
        body,
    })
}

/// Parses a struct body's fields, each requiring exactly one of
/// `#[state(..)]`/`#[hook_param(..)]`/`#[otxn_param(..)]` and a matching
/// field type.
fn parse_named_fields(stream: TokenStream) -> Result<Vec<ParsedField>, TokenStream> {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut i = 0usize;
    let mut fields = Vec::new();

    while i < tokens.len() {
        let mut other_attrs = Vec::new();
        let mut decl_attr: Option<(&'static str, TokenTree, Vec<TokenTree>)> = None;

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
                        "#[hooks]: malformed attribute on a struct field",
                    ));
                }
            };
            let inner: Vec<TokenTree> = group.stream().into_iter().collect();
            let head = inner.first();
            match head {
                Some(TokenTree::Ident(id))
                    if matches!(id.to_string().as_str(), "cfg" | "cfg_attr") =>
                {
                    return Err(err(
                        id.span(),
                        "#[hooks]: `#[cfg]`/`#[cfg_attr]` are not allowed on a chain-struct \
                         field (v0.2 does not support conditional chain declarations)",
                    ));
                }
                Some(TokenTree::Ident(id))
                    if matches!(
                        id.to_string().as_str(),
                        "state" | "hook_param" | "otxn_param" | "state_interface"
                    ) =>
                {
                    if decl_attr.is_some() {
                        return Err(err(
                            id.span(),
                            "#[hooks]: a field must carry exactly one of `#[state]`, \
                             `#[hook_param]`, `#[otxn_param]`, `#[state_interface]`",
                        ));
                    }
                    let args = match inner.get(1) {
                        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => {
                            g.stream().into_iter().collect::<Vec<_>>()
                        }
                        _ => {
                            return Err(err(
                                id.span(),
                                &format!(
                                    "#[hooks]: `#[{id}(..)]` requires parenthesized arguments"
                                ),
                            ));
                        }
                    };
                    let kind: &'static str = match id.to_string().as_str() {
                        "state" => "state",
                        "hook_param" => "hook_param",
                        "state_interface" => "state_interface",
                        _ => "otxn_param",
                    };
                    decl_attr = Some((kind, TokenTree::Ident(id.clone()), args));
                }
                _ => {
                    other_attrs.push(hash);
                    other_attrs.push(TokenTree::Group(group));
                }
            }
            i = i.wrapping_add(2);
        }

        if i >= tokens.len() {
            break;
        }

        let mut vis = Vec::new();
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

        let field_name = match tokens.get(i) {
            Some(TokenTree::Ident(id)) => id.clone(),
            Some(other) => return Err(err(other.span(), "#[hooks]: expected a field name")),
            None => return Err(err(Span::call_site(), "#[hooks]: expected a field name")),
        };
        i = i.wrapping_add(1);

        match tokens.get(i) {
            Some(tt) if is_punct(tt, ':') => {}
            Some(other) => {
                return Err(err(
                    other.span(),
                    "#[hooks]: expected `:` after the field name",
                ));
            }
            None => {
                return Err(err(
                    field_name.span(),
                    "#[hooks]: expected `:` after the field name",
                ));
            }
        }
        i = i.wrapping_add(1);

        let ty_start = i;
        let mut ty_depth = 0i32;
        while i < tokens.len()
            && !(ty_depth == 0 && matches!(tokens.get(i), Some(tt) if is_punct(tt, ',')))
        {
            let (consumed, new_depth) = step_angle_depth(&tokens, i, ty_depth);
            ty_depth = new_depth;
            i = i.wrapping_add(consumed);
        }
        let ty_tokens = tokens.get(ty_start..i).unwrap_or_default().to_vec();
        if i < tokens.len() {
            i = i.wrapping_add(1); // trailing `,`
        }

        let Some((kind, attr_ident, args)) = decl_attr else {
            return Err(err(
                field_name.span(),
                "#[hooks]: every chain-struct field must carry exactly one of \
                 `#[state(..)]`, `#[hook_param(..)]`, `#[otxn_param(..)]`, \
                 `#[state_interface(..)]`",
            ));
        };

        let (wrapper, value_ty) = parse_field_type(&ty_tokens, kind, &field_name)?;
        // `parse_field_decl` first, so its feature-gate rejection (kind ==
        // "state_interface" with `unstable-state-interface` off) wins over
        // this shape check — same gate-ordering rule the sig interface
        // follows for `#[cbak(..)]` extra arguments (its own unconditional
        // rejection is checked first, the feature gate second).
        let decl = parse_field_decl(kind, &args, &attr_ident, wrapper)?;
        if kind == "state_interface" && !matches!(value_ty.as_slice(), [TokenTree::Ident(_)]) {
            return Err(err(
                field_name.span(),
                "#[state_interface]: the field type must be `State<VName>` where `VName` is a \
                 bare identifier — the macro generates `struct VName` from the `value(..)` schema",
            ));
        }

        fields.push(ParsedField {
            other_attrs,
            vis,
            name: field_name,
            value_ty,
            decl,
        });
    }

    Ok(fields)
}

/// Recognizes `[::][rshooks::][decl::]<Wrapper><V>` (trailing comma on `V`
/// allowed) and confirms `<Wrapper>` matches the field's declared attribute
/// kind (`state` -> `State`, `hook_param` -> `HookParam`,
/// `otxn_param` -> `OtxnParam`).
fn parse_field_type<'a>(
    tokens: &'a [TokenTree],
    kind: &str,
    field_name: &Ident,
) -> Result<(&'a str, Vec<TokenTree>), TokenStream> {
    let expected_wrapper = match kind {
        "state" | "state_interface" => "State",
        "hook_param" => "HookParam",
        _ => "OtxnParam",
    };

    let mut i = 0usize;
    if is_punct(
        tokens.first().ok_or_else(|| bad_field_type(field_name))?,
        ':',
    ) && matches!(tokens.get(1), Some(tt) if is_punct(tt, ':'))
    {
        i = 2;
    }
    loop {
        match (tokens.get(i), tokens.get(i.wrapping_add(1))) {
            (Some(TokenTree::Ident(seg)), Some(next))
                if matches!(seg.to_string().as_str(), "rshooks" | "decl")
                    && is_punct(next, ':')
                    && matches!(tokens.get(i.wrapping_add(2)), Some(tt) if is_punct(tt, ':')) =>
            {
                i = i.wrapping_add(3);
            }
            _ => break,
        }
    }

    let wrapper_id = match tokens.get(i) {
        Some(TokenTree::Ident(id)) => id,
        _ => return Err(bad_field_type(field_name)),
    };
    if wrapper_id.to_string() != expected_wrapper {
        return Err(err(
            wrapper_id.span(),
            &format!(
                "#[hooks]: `#[{attr}]` requires a `{expected_wrapper}<V>` field type, found \
                 `{found}`",
                attr = match kind {
                    "state" => "state",
                    "hook_param" => "hook_param",
                    "state_interface" => "state_interface",
                    _ => "otxn_param",
                },
                found = wrapper_id,
            ),
        ));
    }
    i = i.wrapping_add(1);

    if !matches!(tokens.get(i), Some(tt) if is_punct(tt, '<')) {
        return Err(err(
            wrapper_id.span(),
            &format!("#[hooks]: expected `{expected_wrapper}<V>` (missing `<V>`)"),
        ));
    }
    let Some((inner, after)) = parse_balanced_angle(tokens, i) else {
        return Err(err(
            wrapper_id.span(),
            &format!("#[hooks]: unbalanced `<..>` in `{expected_wrapper}<V>`"),
        ));
    };
    if let Some(extra) = tokens.get(after) {
        return Err(err(
            extra.span(),
            "#[hooks]: unexpected tokens after the field type",
        ));
    }

    let args = split_top_level_commas(&inner);
    if args.len() != 1 {
        return Err(err(
            wrapper_id.span(),
            &format!(
                "#[hooks]: `{expected_wrapper}<V>` takes exactly one type argument, found {}",
                args.len()
            ),
        ));
    }
    let value_ty = args.into_iter().next().unwrap_or_default();

    Ok((expected_wrapper, value_ty))
}

fn bad_field_type(field_name: &Ident) -> TokenStream {
    err(
        field_name.span(),
        "#[hooks]: expected a `State<V>` / `HookParam<V>` / `OtxnParam<V>` field type \
         (optionally qualified as `rshooks::..` / `::rshooks::..` / `decl::..`)",
    )
}

/// Parses a
/// `#[state(..)]`/`#[hook_param(..)]`/`#[otxn_param(..)]`/`#[state_interface(..)]`
/// argument list into a [`FieldDecl`].
fn parse_field_decl(
    kind: &str,
    args: &[TokenTree],
    attr_ident: &TokenTree,
    _wrapper: &str,
) -> Result<FieldDecl, TokenStream> {
    // `#[state_interface(id = .., key(..), value(..))]`'s `key(..)`/
    // `value(..)` arguments are groups, not `key = value` pairs, so it
    // cannot share `parse_attr_entries`'s flat `key = value` grammar —
    // branch out before that call rather than after.
    if kind == "state_interface" {
        #[cfg(not(feature = "unstable-state-interface"))]
        {
            return Err(err(attr_ident.span(), SI_FEATURE_GATE_MSG));
        }
        #[cfg(feature = "unstable-state-interface")]
        {
            return parse_state_interface_decl(args, attr_ident);
        }
    }

    let mac = &format!("#[{kind}]");
    let entries = parse_attr_entries(args, mac)?;

    if kind == "state" {
        let mut key: Option<KeySpec> = None;
        for AttrEntry {
            key: k,
            key_span,
            value,
        } in entries
        {
            match k.as_str() {
                "key" => {
                    if key.is_some() {
                        return Err(err(
                            key_span,
                            "#[state]: specify exactly one of `key` or `key_by`",
                        ));
                    }
                    let Some(expr) = value else {
                        return Err(err(
                            key_span,
                            "#[state]: `key` requires a value: `key = <expr>`",
                        ));
                    };
                    key = Some(KeySpec::Const { expr });
                }
                "key_by" => {
                    if key.is_some() {
                        return Err(err(
                            key_span,
                            "#[state]: specify exactly one of `key` or `key_by`",
                        ));
                    }
                    let Some(ty) = value else {
                        return Err(err(
                            key_span,
                            "#[state]: `key_by` requires a value: `key_by = <TypePath>`",
                        ));
                    };
                    key = Some(KeySpec::Keyed { ty });
                }
                other => {
                    return Err(err(
                        key_span,
                        &format!(
                            "#[state]: unknown argument `{other}` (expected `key` or `key_by`)"
                        ),
                    ));
                }
            }
        }
        let Some(key) = key else {
            return Err(err(
                attr_ident.span(),
                "#[state]: missing required `key = <expr>` or `key_by = <TypePath>`",
            ));
        };
        return Ok(FieldDecl::State { key });
    }

    let param_kind = if kind == "hook_param" {
        ParamKind::HookParam
    } else {
        ParamKind::OtxnParam
    };

    let mut name: Option<NameSpec> = None;
    let mut required = false;
    let mut required_span: Option<Span> = None;
    let mut default: Option<Vec<TokenTree>> = None;
    let mut default_span: Option<Span> = None;

    for AttrEntry {
        key,
        key_span,
        value,
    } in entries
    {
        match key.as_str() {
            "name" => {
                if name.is_some() {
                    return Err(err(
                        key_span,
                        &format!(
                            "#[{}]: specify exactly one of `name` or `name_by`",
                            param_kind.attr_name()
                        ),
                    ));
                }
                let Some(tokens) = value else {
                    return Err(err(
                        key_span,
                        "expected `name = b\"...\"` (a byte-string literal)",
                    ));
                };
                let mac = format!("#[{}]", param_kind.attr_name());
                let (literal, decoded_len) =
                    parse_byte_string_value(Some(&tokens), key_span, &mac, "name")?;
                if !(1..=32).contains(&decoded_len) {
                    return Err(err(
                        key_span,
                        &format!(
                            "{mac}: `name` must decode to 1..=32 bytes (the Hook API's \
                             parameter-name length limit), found {decoded_len}"
                        ),
                    ));
                }
                name = Some(NameSpec::Literal { literal });
            }
            "name_by" => {
                if name.is_some() {
                    return Err(err(
                        key_span,
                        &format!(
                            "#[{}]: specify exactly one of `name` or `name_by`",
                            param_kind.attr_name()
                        ),
                    ));
                }
                let Some(ty) = value else {
                    return Err(err(key_span, "expected `name_by = <TypePath>`"));
                };
                name = Some(NameSpec::Family { ty });
            }
            "required" => {
                if value.is_some() {
                    return Err(err(key_span, "`required` takes no value"));
                }
                if required {
                    return Err(err(
                        key_span,
                        &format!("#[{}]: duplicate `required`", param_kind.attr_name()),
                    ));
                }
                required = true;
                required_span = Some(key_span);
            }
            "default" => {
                let Some(expr) = value else {
                    return Err(err(key_span, "expected `default = <expr>`"));
                };
                if default.is_some() {
                    return Err(err(
                        key_span,
                        &format!("#[{}]: duplicate `default`", param_kind.attr_name()),
                    ));
                }
                default = Some(expr);
                default_span = Some(key_span);
            }
            other => {
                return Err(err(
                    key_span,
                    &format!(
                        "#[{}]: unknown argument `{other}` (expected `name`, `name_by`, \
                         `required` or `default`)",
                        param_kind.attr_name()
                    ),
                ));
            }
        }
    }

    if let (Some(rs), Some(_)) = (required_span, default_span) {
        return Err(err(
            rs,
            &format!(
                "#[{}]: `required` and `default` are mutually exclusive",
                param_kind.attr_name()
            ),
        ));
    }

    let Some(name) = name else {
        return Err(err(
            attr_ident.span(),
            &format!(
                "#[{}]: missing required `name = b\"...\"` or `name_by = <TypePath>`",
                param_kind.attr_name()
            ),
        ));
    };

    Ok(FieldDecl::Param {
        param_kind,
        spec: ParamSpecDecl {
            name,
            required,
            default,
        },
    })
}

// ---------------------------------------------------------------------
// `#[state_interface(id = .., key(..), value(..))]` — see
// `docs/STATE_INTERFACE_DESIGN.md`. Gated behind the
// `unstable-state-interface` feature; the feature-off rejection lives in
// `parse_field_decl`'s `kind == "state_interface"` branch, mirroring
// `hooks_impl.rs`'s `SIG_FEATURE_GATE_MSG` pattern for signature
// parameters.
// ---------------------------------------------------------------------

/// Diagnostic for `#[state_interface(..)]` when the `unstable-state-interface`
/// feature is off.
#[cfg(not(feature = "unstable-state-interface"))]
const SI_FEATURE_GATE_MSG: &str = "#[hooks]: `#[state_interface(..)]` declares Hook State \
                                    Interface fields (draft spec) — enable rshooks's \
                                    `unstable-state-interface` feature to use them (the \
                                    interface may change while the spec is a draft)";

/// The diagnostic listing every supported `#[state_interface]` field type —
/// shared by [`classify_si_type`]'s only call site so the list can't drift
/// out of sync with the match arms below it.
#[cfg(feature = "unstable-state-interface")]
const SI_TYPE_MSG: &str = "#[state_interface]: unsupported field type — supported types are u8, \
                            u16, u32, u64, [u8; 16], [u8; 32], Hash, AccountId, [u8; 20], \
                            CurrencyCode, XFL (write the type as a bare name — a path-qualified or \
                            aliased spelling is not resolved)";

/// The interface draft's field-name diagnostic — shared by every rejection
/// [`is_valid_interface_name`] backs.
#[cfg(feature = "unstable-state-interface")]
const SI_NAME_MSG: &str = "#[state_interface]: a key/value field name must be 1..=16 ASCII \
                            bytes matching [A-Za-z][A-Za-z0-9]* (the Hook State Interface's \
                            field-name rule) — rename the field";

/// Maximum total encoded width, in bytes, of a state interface key's field
/// payload (`docs/STATE_INTERFACE_DESIGN.md` §1.6) — the 32-byte key minus
/// the 1-byte State ID prefix.
#[cfg(feature = "unstable-state-interface")]
const SI_MAX_KEY_PAYLOAD: usize = 31;

/// The protocol's `HookParameterValue` byte-length limit — xahaud
/// `include/xrpl/hook/Enum.h`'s `maxHookParameterValueSize()`.
#[cfg(feature = "unstable-state-interface")]
const SI_MAX_VALUE_LEN: usize = 256;

/// Classifies a `#[state_interface]` key/value field's type tokens into its
/// `(XAS-010d type code, byte width)` pair (`docs/STATE_INTERFACE_DESIGN.md`
/// §1.5's table), matching on the token *shape* (a whitespace-normalized
/// textual comparison). Does not resolve type aliases: an aliased type is
/// caught later by the monomorphized `const` assert the generated code
/// emits alongside these values.
#[cfg(feature = "unstable-state-interface")]
fn classify_si_type(tokens: &[TokenTree]) -> Option<(u8, usize)> {
    classify_fixed_sti_type_text(&tokens_to_string(tokens).replace(' ', ""))
}

/// Parses one `key(..)`/`value(..)` group's inner `name: Type, ..` list.
/// `list_kind` (`"key"`/`"value"`) only feeds diagnostics.
#[cfg(feature = "unstable-state-interface")]
/// Returns each field paired with its name identifier's own span — kept
/// alongside, rather than inside, [`SiFieldSpec`] (which stays
/// `proc_macro`-`Span`-free so it can be built directly in unit tests,
/// mirroring [`ChainFieldJson`]'s own "kept `TokenTree`-free where testable"
/// boundary) — callers that need to blame a size-limit diagnostic on a
/// specific field (`parse_state_interface_decl`'s key-payload/name-length
/// checks) use the span; callers that don't (`value(..)`) just discard it.
fn parse_si_field_list(
    tokens: &[TokenTree],
    list_kind: &'static str,
) -> Result<Vec<(SiFieldSpec, Span)>, TokenStream> {
    let groups = split_top_level_commas(tokens);
    let mut out = Vec::with_capacity(groups.len());
    let mut seen_names: Vec<String> = Vec::new();

    for group in &groups {
        let (name_id, after_name) = match group.split_first() {
            Some((TokenTree::Ident(id), rest)) => (id.clone(), rest),
            Some((other, _)) => {
                return Err(err(
                    other.span(),
                    &format!(
                        "#[state_interface]: expected a {list_kind} field here, e.g. `amount: u64`"
                    ),
                ));
            }
            None => {
                return Err(err(
                    Span::call_site(),
                    &format!("#[state_interface]: expected a {list_kind} field"),
                ));
            }
        };
        let after_colon = match after_name.split_first() {
            Some((tt, after)) if is_punct(tt, ':') => after,
            _ => {
                return Err(err(
                    name_id.span(),
                    "#[state_interface]: expected `: Type` after the field name",
                ));
            }
        };
        if after_colon.is_empty() {
            return Err(err(
                name_id.span(),
                "#[state_interface]: expected a type after `:`",
            ));
        }

        let name = name_id.to_string();
        if !is_valid_interface_name(&name) {
            return Err(err(name_id.span(), SI_NAME_MSG));
        }
        if seen_names.iter().any(|n| n == &name) {
            return Err(err(
                name_id.span(),
                &format!("#[state_interface]: duplicate {list_kind} field name `{name}`"),
            ));
        }
        seen_names.push(name.clone());

        let Some((type_byte, width)) = classify_si_type(after_colon) else {
            let span = after_colon.first().map_or(name_id.span(), TokenTree::span);
            return Err(err(span, SI_TYPE_MSG));
        };

        out.push((
            SiFieldSpec {
                name,
                ty_tokens: after_colon.to_vec(),
                type_byte,
                width,
            },
            name_id.span(),
        ));
    }
    Ok(out)
}

/// Parses `id = <0..=255>`'s value tokens.
#[cfg(feature = "unstable-state-interface")]
fn parse_si_id(tokens: &[TokenTree], span: Span) -> Result<u8, TokenStream> {
    let bad = || {
        err(
            span,
            "#[state_interface]: `id` must be an integer literal 0..=255",
        )
    };
    let [TokenTree::Literal(lit)] = tokens else {
        return Err(bad());
    };
    let mut stream = TokenStream::new();
    stream.extend([TokenTree::Literal(lit.clone())]);
    syn::parse::<syn::LitInt>(stream)
        .ok()
        .and_then(|l| l.base10_parse::<u8>().ok())
        .ok_or_else(bad)
}

/// Parses a `#[state_interface(id = .., key(..), value(..))]` argument list
/// into a [`FieldDecl::StateInterface`].
#[cfg(feature = "unstable-state-interface")]
fn parse_state_interface_decl(
    args: &[TokenTree],
    attr_ident: &TokenTree,
) -> Result<FieldDecl, TokenStream> {
    let groups = split_top_level_commas(args);

    let mut id: Option<(u8, Span)> = None;
    let mut key_fields: Vec<SiFieldSpec> = Vec::new();
    // Parallel to `key_fields` — each field's own name span, so the
    // key-payload/declared-name-length overflow checks below can blame the
    // specific field that pushed the running total over the limit.
    let mut key_field_spans: Vec<Span> = Vec::new();
    let mut key_seen = false;
    let mut value_fields: Vec<SiFieldSpec> = Vec::new();
    let mut value_seen = false;

    for group in &groups {
        let Some((TokenTree::Ident(head), rest)) = group.split_first() else {
            let span = group
                .first()
                .map_or_else(|| attr_ident.span(), TokenTree::span);
            return Err(err(
                span,
                "#[state_interface]: expected `id = ..`, `key(..)` or `value(..)`",
            ));
        };
        match head.to_string().as_str() {
            "id" => {
                if id.is_some() {
                    return Err(err(head.span(), "#[state_interface]: duplicate `id`"));
                }
                let value_tokens = match rest.split_first() {
                    Some((eq, after)) if is_punct(eq, '=') => after,
                    _ => {
                        return Err(err(
                            head.span(),
                            "#[state_interface]: expected `id = <0..=255>`",
                        ));
                    }
                };
                id = Some((parse_si_id(value_tokens, head.span())?, head.span()));
            }
            "key" => {
                if key_seen {
                    return Err(err(head.span(), "#[state_interface]: duplicate `key(..)`"));
                }
                key_seen = true;
                let inner = match rest {
                    [TokenTree::Group(g)] if g.delimiter() == Delimiter::Parenthesis => {
                        g.stream().into_iter().collect::<Vec<_>>()
                    }
                    _ => {
                        return Err(err(
                            head.span(),
                            "#[state_interface]: expected `key(name: Type, ..)`",
                        ));
                    }
                };
                (key_fields, key_field_spans) =
                    parse_si_field_list(&inner, "key")?.into_iter().unzip();
            }
            "value" => {
                if value_seen {
                    return Err(err(
                        head.span(),
                        "#[state_interface]: duplicate `value(..)`",
                    ));
                }
                value_seen = true;
                let inner = match rest {
                    [TokenTree::Group(g)] if g.delimiter() == Delimiter::Parenthesis => {
                        g.stream().into_iter().collect::<Vec<_>>()
                    }
                    _ => {
                        return Err(err(
                            head.span(),
                            "#[state_interface]: expected `value(name: Type, ..)`",
                        ));
                    }
                };
                value_fields = parse_si_field_list(&inner, "value")?
                    .into_iter()
                    .map(|(f, _)| f)
                    .collect();
            }
            other => {
                return Err(err(
                    head.span(),
                    &format!(
                        "#[state_interface]: unknown argument `{other}` (expected `id`, `key` \
                         or `value`)"
                    ),
                ));
            }
        }
    }

    let Some((id, id_span)) = id else {
        return Err(err(
            attr_ident.span(),
            "#[state_interface]: missing required `id = <0..=255>`",
        ));
    };

    if !value_seen || value_fields.is_empty() {
        return Err(err(
            attr_ident.span(),
            "#[state_interface]: missing required `value(name: Type, ..)` (at least one value \
             field)",
        ));
    }

    // Both checks below blame the specific key field whose inclusion first
    // pushed the running total over its limit (via `key_field_spans`,
    // parallel to `key_fields`), not the `id = ..` argument — matching
    // every other per-field diagnostic in this function.
    let mut key_payload = 0usize;
    for (f, &span) in key_fields.iter().zip(&key_field_spans) {
        key_payload = key_payload.wrapping_add(f.width);
        if key_payload > SI_MAX_KEY_PAYLOAD {
            return Err(err(
                span,
                &format!(
                    "#[state_interface]: key payload is {key_payload} bytes, exceeds the \
                     {SI_MAX_KEY_PAYLOAD}-byte limit (the 32-byte key minus the 1-byte State ID)"
                ),
            ));
        }
    }

    let mut name_total = 6usize;
    for (f, &span) in key_fields.iter().zip(&key_field_spans) {
        name_total = name_total.wrapping_add(2usize.wrapping_add(f.name.len()));
        if name_total > 32 {
            return Err(err(
                span,
                &format!(
                    "#[state_interface]: the declared HookParameterName would be {name_total} \
                     bytes, exceeds the protocol's 32-byte HookParameterName limit"
                ),
            ));
        }
    }

    let value_total: usize = 1usize.wrapping_add(
        value_fields
            .iter()
            .map(|f| 2usize.wrapping_add(f.name.len()))
            .sum(),
    );
    if value_total > SI_MAX_VALUE_LEN {
        return Err(err(
            id_span,
            &format!(
                "#[state_interface]: the declared HookParameterValue would be {value_total} \
                 bytes, exceeds the protocol's {SI_MAX_VALUE_LEN}-byte HookParameterValue limit"
            ),
        ));
    }

    Ok(FieldDecl::StateInterface {
        id,
        id_span,
        key_fields,
        value_fields,
    })
}

/// Renders the complete expansion for an already-validated
/// [`ParsedStruct`].
fn generate(parsed: &ParsedStruct, description: Option<&str>) -> TokenStream {
    let struct_name = parsed.name.to_string();
    let vis_text = tokens_to_string(&parsed.vis);
    let leading_attrs_text = tokens_to_string(&parsed.leading_attrs);

    let fields: &[ParsedField] = match &parsed.body {
        StructBody::Unit => &[],
        StructBody::Named(fields) => fields,
    };

    // State IDs must be unique across every `#[state_interface(..)]` field
    // on this struct (`docs/STATE_INTERFACE_DESIGN.md` §1.3) — a
    // cross-field rule, so it's checked here rather than at each field's
    // own parse time.
    let mut seen_si_ids: Vec<u8> = Vec::new();
    for f in fields {
        if let FieldDecl::StateInterface { id, id_span, .. } = &f.decl {
            if seen_si_ids.contains(id) {
                return err(
                    *id_span,
                    &format!(
                        "#[state_interface]: duplicate State ID {id} (State IDs must be unique across every `#[state_interface(..)]` field on this struct)"
                    ),
                );
            }
            seen_si_ids.push(*id);
        }
    }

    let mut out = String::new();

    // 1. The namespace structs (one per non-empty kind) plus the outer
    //    struct, whose fields are the non-empty namespaces (module doc
    //    comment's shape). Fields keep their marker-injected type, keyed to
    //    their ordinal in the original flat declaration (`field_index`) —
    //    grouping into namespaces never renumbers markers. The struct
    //    item's own leading attributes attach to the OUTER struct only,
    //    never to a namespace struct, which is a macro-generated
    //    implementation detail the user's attributes were never written
    //    against.
    match &parsed.body {
        StructBody::Unit => {
            out.push_str(&leading_attrs_text);
            out.push('\n');
            out.push_str(&format!("{vis_text} struct {struct_name};\n"));
        }
        StructBody::Named(fields) => {
            let mut namespace_structs_text = String::new();
            for ns in Namespace::ALL {
                let ns_fields: Vec<(usize, &ParsedField)> = fields
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.decl.namespace() == ns)
                    .collect();
                if ns_fields.is_empty() {
                    continue;
                }
                let ns_name = format!("{struct_name}{}", ns.struct_suffix());
                namespace_structs_text.push_str(&format!(
                    "/// The `#[{attr}]` entries declared on [`{struct_name}`].\n\
                     {vis_text} struct {ns_name} {{\n",
                    attr = ns.attr_name(),
                ));
                for (field_index, f) in ns_fields {
                    namespace_structs_text.push_str(&tokens_to_string(&f.other_attrs));
                    namespace_structs_text.push('\n');
                    namespace_structs_text.push_str(&tokens_to_string(&f.vis));
                    namespace_structs_text.push(' ');
                    namespace_structs_text.push_str(&f.name.to_string());
                    namespace_structs_text.push_str(": ");
                    namespace_structs_text.push_str(&rewritten_field_type(
                        &struct_name,
                        field_index,
                        f,
                    ));
                    namespace_structs_text.push_str(",\n");
                }
                namespace_structs_text.push_str("}\n");
            }

            let mut outer_struct_text = format!("{vis_text} struct {struct_name} {{\n");
            for ns in Namespace::ALL {
                if !fields.iter().any(|f| f.decl.namespace() == ns) {
                    continue;
                }
                outer_struct_text.push_str(&format!(
                    "/// The `#[{attr}]` entries declared on this struct.\n\
                     {vis_text} {field}: {struct_name}{suffix},\n",
                    attr = ns.attr_name(),
                    field = ns.field_name(),
                    suffix = ns.struct_suffix(),
                ));
            }
            outer_struct_text.push_str("}\n");

            out.push_str(&assemble_named_body(
                &leading_attrs_text,
                &namespace_structs_text,
                &outer_struct_text,
            ));
        }
    }

    // 2. Per-field marker ZST + spec trait impl(s).
    let struct_is_pub = !parsed.vis.is_empty();
    for (field_index, f) in fields.iter().enumerate() {
        out.push_str(&field_marker_and_impls(
            &struct_name,
            field_index,
            struct_is_pub,
            f,
        ));
    }

    // 3. Named-field structs only: a same-name static value binding, one
    //    nested namespace-struct literal per non-empty kind.
    if let StructBody::Named(fields) = &parsed.body {
        out.push_str(&format!(
            "#[doc(hidden)]\n#[allow(non_upper_case_globals)]\n\
             {vis_text} static {struct_name}: {struct_name} = {struct_name} {{\n"
        ));
        for ns in Namespace::ALL {
            let ns_fields: Vec<&ParsedField> =
                fields.iter().filter(|f| f.decl.namespace() == ns).collect();
            if ns_fields.is_empty() {
                continue;
            }
            out.push_str(&format!(
                "{field}: {struct_name}{suffix} {{\n",
                field = ns.field_name(),
                suffix = ns.struct_suffix(),
            ));
            for f in ns_fields {
                let wrapper = match &f.decl {
                    FieldDecl::State { .. } | FieldDecl::StateInterface { .. } => "State",
                    FieldDecl::Param { param_kind, .. } => param_kind.wrapper(),
                };
                out.push_str(&format!(
                    "{field}: ::rshooks::decl::{wrapper}::new(),\n",
                    field = f.name
                ));
            }
            out.push_str("},\n");
        }
        out.push_str("};\n");
    }

    // 4. Handshake: an associated const the impl-side macro requires, plus
    //    an assertion that the impl-side macro implemented `HookChainImpl`.
    let snake = crate::hooks_shared::to_snake_case(&struct_name);
    out.push_str(&format!(
        "impl {struct_name} {{\n\
             #[doc(hidden)]\n\
             pub const __RSHOOKS_STRUCT: () = ();\n\
         }}\n\
         #[doc(hidden)]\n\
         #[allow(dead_code)]\n\
         fn __rshooks_assert_chain_impl_{snake}() {{\n\
             fn assert<T: ::rshooks::__internal::HookChainImpl>() {{}}\n\
             assert::<{struct_name}>();\n\
         }}\n"
    ));

    // 5. One-per-crate link marker (wasm only).
    out.push_str(
        "#[cfg(target_arch = \"wasm32\")]\n\
         #[doc(hidden)]\n\
         #[unsafe(no_mangle)]\n\
         pub static __rshooks_chain: u8 = 0;\n",
    );

    // 6. Struct carrier (wasm only).
    let chain_entries: Vec<ChainFieldJson> = fields.iter().map(field_to_chain_json).collect();
    match encode_chain_json(&struct_name, description, &chain_entries) {
        Ok(payload) => {
            let payload_hex = hex_upper(&payload);
            let digest = sha256::sha256(&payload);
            let carrier_ident = format!("__rshooks_chain_{}", hex_lower(&digest));
            out.push_str(&format!(
                "#[cfg(target_arch = \"wasm32\")]\n\
                 #[doc(hidden)]\n\
                 #[unsafe(export_name = \"{CHAIN_EXPORT_PREFIX}{payload_hex}\")]\n\
                 pub extern \"C\" fn {carrier_ident}(_reserved: u32) -> i64 {{ 0 }}\n"
            ));
        }
        Err(message) => return err(parsed.name.span(), &message),
    }

    out.parse::<TokenStream>().unwrap_or_else(|_| {
        err(
            parsed.name.span(),
            "rshooks-macros: internal #[hooks] struct expansion failed to parse",
        )
    })
}

/// Assembles a named-field body's struct-declaration text: the namespace
/// structs first, then the struct item's own leading attributes immediately
/// followed by the outer struct's declaration. `leading_attrs_text` must
/// attach to the outer struct the user actually wrote `#[hooks]` on, never
/// to a namespace struct. Kept `proc_macro`-free (plain strings only) so
/// this ordering invariant can be pinned by a unit test — `proc_macro`
/// types panic outside an active macro invocation.
fn assemble_named_body(
    leading_attrs_text: &str,
    namespace_structs_text: &str,
    outer_struct_text: &str,
) -> String {
    let mut out = String::new();
    out.push_str(namespace_structs_text);
    out.push_str(leading_attrs_text);
    out.push('\n');
    out.push_str(outer_struct_text);
    out
}

/// The rewritten field type text: `::rshooks::decl::<Wrapper><V, __Marker>`.
fn rewritten_field_type(struct_name: &str, field_index: usize, f: &ParsedField) -> String {
    let wrapper = match &f.decl {
        FieldDecl::State { .. } | FieldDecl::StateInterface { .. } => "State",
        FieldDecl::Param { param_kind, .. } => param_kind.wrapper(),
    };
    let marker = marker_name(struct_name, field_index, &f.name.to_string());
    let value_ty = tokens_to_string(&f.value_ty);
    format!("::rshooks::decl::{wrapper}<{value_ty}, {marker}>")
}

/// Builds the field's marker type name: `__RshooksSpec{Struct}Field{N}{Name}`.
///
/// Includes the field's ordinal position (`field_index`) because two
/// distinct field names can collapse to the same `UpperCamelCase` text
/// under [`to_upper_camel`] (e.g. `foo_bar` and `foo__bar`, both `FooBar`)
/// — the ordinal keeps every marker name unique regardless. `field_name` is
/// sanitized by stripping a leading `r#` raw-identifier prefix first
/// (`r#type` -> `type` -> `Type`) so a raw identifier field still produces
/// a valid marker identifier.
fn marker_name(struct_name: &str, field_index: usize, field_name: &str) -> String {
    let sanitized = field_name.strip_prefix("r#").unwrap_or(field_name);
    format!(
        "__RshooksSpec{struct_name}Field{field_index}{}",
        to_upper_camel(sanitized)
    )
}

/// `true` when `expr` is exactly one byte-string literal token
/// (`b"..."`/`br"..."`) — the `KeySpec::Const` shape
/// [`field_marker_and_impls`] promotes to a compile-time, `'static`
/// `EncodedStateKey` via `EncodedStateKey::from_short` instead of
/// re-encoding at runtime on every access. `false` for any other key
/// expression, which keeps the runtime `StateKeyEncode::encode` path via
/// `StateSpec::with_key`'s default — a normal, supported shape, not an
/// error.
fn is_byte_string_literal(expr: &[TokenTree]) -> bool {
    let [TokenTree::Literal(lit)] = expr else {
        return false;
    };
    let mut stream = TokenStream::new();
    stream.extend([TokenTree::Literal(lit.clone())]);
    syn::parse::<syn::LitByteStr>(stream).is_ok()
}

/// The marker ZST declaration plus its `StateSpec`/`ParamSpec` (+
/// `ParamDefault`/`ParamRequired`) impl(s) for one field.
fn field_marker_and_impls(
    struct_name: &str,
    field_index: usize,
    struct_is_pub: bool,
    f: &ParsedField,
) -> String {
    let marker = marker_name(struct_name, field_index, &f.name.to_string());
    let value_ty = tokens_to_string(&f.value_ty);
    // The marker's visibility follows the field's own declared visibility
    // — avoiding `E0446` ("private type in public interface") when a
    // private `#[state]`/`#[hook_param]`/`#[otxn_param]` value/key/name
    // type would otherwise leak through the marker's `StateSpec`/
    // `ParamSpec` associated types — unless the struct itself carries no
    // `pub` token at all, in which case the marker is forced private
    // regardless of the field's visibility. Without that override, a
    // `pub` field on a private struct would give its marker wider reach
    // than the struct is ever actually reachable at: the marker should be
    // exactly as reachable as the field it backs, never more.
    let field_vis = if struct_is_pub {
        tokens_to_string(&f.vis)
    } else {
        String::new()
    };
    let mut out = format!("#[doc(hidden)]\n{field_vis} struct {marker};\n");

    match &f.decl {
        FieldDecl::State { key } => {
            let (key_args, encode_body, with_key_override) = match key {
                KeySpec::Const { expr } => {
                    let expr_text = tokens_to_string(expr);
                    // A byte-string-literal key (the common case) is
                    // promoted to a compile-time `'static` `EncodedStateKey`
                    // via `with_key` below instead of re-encoding at
                    // runtime on every access; any other const expression
                    // keeps the runtime `encode_key` path via `with_key`'s
                    // default.
                    let override_method = is_byte_string_literal(expr).then(|| {
                        format!(
                            "#[inline(always)]\n\
                             fn with_key<__R>(_args: &Self::KeyArgs, f: impl ::core::ops::FnOnce(&::rshooks::state::EncodedStateKey) -> __R) -> __R {{\n\
                                 f(const {{ &::rshooks::state::EncodedStateKey::from_short({expr_text}) }})\n\
                             }}\n"
                        )
                    });
                    (
                        "()".to_string(),
                        format!("::rshooks::state::StateKeyEncode::encode({expr_text})"),
                        override_method,
                    )
                }
                KeySpec::Keyed { ty } => {
                    let ty_text = tokens_to_string(ty);
                    (
                        ty_text,
                        "::rshooks::state::StateKeyEncode::encode(args)".to_string(),
                        None,
                    )
                }
            };
            let args_pat = if key_args == "()" { "_args" } else { "args" };
            let with_key_method = with_key_override.unwrap_or_default();
            out.push_str(&format!(
                "#[automatically_derived]\n\
                 impl ::rshooks::decl::StateSpec for {marker} {{\n\
                     type Value = {value_ty};\n\
                     type KeyArgs = {key_args};\n\
                     #[inline(always)]\n\
                     fn encode_key({args_pat}: &Self::KeyArgs) -> ::rshooks::state::EncodedStateKey {{\n\
                         {encode_body}\n\
                     }}\n\
                     {with_key_method}\
                 }}\n"
            ));
        }
        FieldDecl::Param { spec, .. } => {
            let (name_args, with_name_body, extra) = match &spec.name {
                NameSpec::Literal { literal } => {
                    ("()".to_string(), format!("f({})", literal), String::new())
                }
                NameSpec::Family { ty } => {
                    let ty_text = tokens_to_string(ty);
                    let body = format!(
                        "let mut __buf = [0u8; <{ty_text} as ::rshooks::convert::ToBytes>::MAX_LEN];\n\
                         let __n = <{ty_text} as ::rshooks::convert::ToBytes>::write(args, &mut __buf);\n\
                         f(__buf.get(..__n).unwrap_or(&[]))"
                    );
                    let assert = crate::param_name::param_name_length_assert(&ty_text);
                    (ty_text, body, assert)
                }
            };
            let args_pat = if name_args == "()" { "_args" } else { "args" };
            out.push_str(&extra);
            out.push_str(&format!(
                "#[automatically_derived]\n\
                 impl ::rshooks::decl::ParamSpec for {marker} {{\n\
                     type Value = {value_ty};\n\
                     type NameArgs = {name_args};\n\
                     #[inline(always)]\n\
                     fn with_name_bytes<__R>({args_pat}: &Self::NameArgs, f: impl ::core::ops::FnOnce(&[u8]) -> __R) -> __R {{\n\
                         {with_name_body}\n\
                     }}\n\
                 }}\n"
            ));
            if let Some(default_expr) = &spec.default {
                let default_text = tokens_to_string(default_expr);
                out.push_str(&format!(
                    "#[automatically_derived]\n\
                     impl ::rshooks::decl::ParamDefault for {marker} {{\n\
                         #[inline(always)]\n\
                         fn default_value() -> Self::Value {{ {default_text} }}\n\
                     }}\n"
                ));
            }
            if spec.required {
                out.push_str(&format!(
                    "#[automatically_derived]\n\
                     impl ::rshooks::decl::ParamRequired for {marker} {{}}\n"
                ));
            }
        }
        FieldDecl::StateInterface {
            id,
            key_fields,
            value_fields,
            ..
        } => {
            out.push_str(&state_interface_field_codegen(
                &field_vis,
                &marker,
                &value_ty,
                *id,
                key_fields,
                value_fields,
            ));
        }
    }

    out
}

/// The value struct, `SiFieldType` alias-drift const-asserts, and
/// `StateSpec` impl for one `#[state_interface(..)]` field
/// (`docs/STATE_INTERFACE_DESIGN.md` §3). `value_ty` is the bare
/// user-chosen value-struct identifier (already validated as a single
/// `Ident` by the caller).
///
/// Not itself feature-gated (unlike the `#[state_interface(..)]` parser
/// above) — mirrors `hooks_impl.rs`'s `render_sig_param_prologue`: its only
/// call site is an ordinary, unconditionally-compiled match arm on
/// [`FieldDecl::StateInterface`], which the parser never constructs unless
/// `unstable-state-interface` is on, so this is unreachable rather than
/// unused when the feature is off.
fn state_interface_field_codegen(
    field_vis: &str,
    marker: &str,
    value_ty: &str,
    id: u8,
    key_fields: &[SiFieldSpec],
    value_fields: &[SiFieldSpec],
) -> String {
    let mut out = String::new();

    // --- the value struct: pub fields, `ToBytes`/`FromBytes`/`FixedRead`
    // mirroring `#[derive(HookData)]`'s codegen shape, except every field
    // is encoded via `si::SiFieldType` (big-endian ints, verbatim byte
    // arrays) at macro-computed const offsets (docs/STATE_INTERFACE_DESIGN.md
    // §3 item 1). Widths come from `classify_si_type`, so offsets are
    // ordinary `usize` literals rather than an `<Ty as ToBytes>::MAX_LEN`
    // const chain.
    // `missing_docs` (workspace lint, denied) applies to every reachable
    // item this macro emits, this struct and its fields included when
    // `field_vis` makes them `pub` — so every one gets a real doc comment,
    // not just the marker's `#[doc(hidden)]` escape.
    out.push_str(&format!(
        "/// Generated by `#[state_interface(id = {id}, ..)]`: the declared value \
         schema `{value_schema}`.\n\
         #[derive(Clone, Copy)]\n{field_vis} struct {value_ty} {{\n",
        value_schema = si_field_list_display(value_fields),
    ));
    for f in value_fields {
        out.push_str(&format!(
            "/// `{name}: {ty}`.\n{field_vis} {name}: {ty},\n",
            name = f.name,
            ty = tokens_to_string(&f.ty_tokens),
        ));
    }
    out.push_str("}\n");

    let value_len: usize = value_fields.iter().map(|f| f.width).sum();
    let value_offsets = field_offsets(0, value_fields);

    let mut write_body = String::new();
    let mut read_body = String::new();
    for (f, (off, next)) in value_fields.iter().zip(&value_offsets) {
        write_body.push_str(&format!(
            "if let Some(__f) = __dst.get_mut({off}..{next}) {{ \
             ::rshooks::si::SiFieldType::write_si(&self.{name}, __f); }}\n",
            name = f.name,
        ));
        read_body.push_str(&format!(
            "{name}: <{ty} as ::rshooks::si::SiFieldType>::read_si(__src.get({off}..{next}).unwrap_or(&[])),\n",
            name = f.name,
            ty = tokens_to_string(&f.ty_tokens),
        ));
    }

    out.push_str(&format!(
        "#[automatically_derived]\n\
         impl ::rshooks::convert::ToBytes for {value_ty} {{\n\
             const MAX_LEN: usize = {value_len}usize;\n\
             #[inline(always)]\n\
             fn write(&self, buf: &mut [u8]) -> usize {{\n\
                 match buf.get_mut(..<Self as ::rshooks::convert::ToBytes>::MAX_LEN) {{\n\
                     ::core::option::Option::Some(__dst) => {{\n\
                         {write_body}\
                         <Self as ::rshooks::convert::ToBytes>::MAX_LEN\n\
                     }}\n\
                     ::core::option::Option::None => 0,\n\
                 }}\n\
             }}\n\
         }}\n\
         #[automatically_derived]\n\
         impl ::rshooks::convert::FromBytes for {value_ty} {{\n\
             #[inline(always)]\n\
             fn read(buf: &[u8]) -> ::rshooks::error::Result<Self> {{\n\
                 let __src = buf.get(..<Self as ::rshooks::convert::ToBytes>::MAX_LEN)\n\
                     .ok_or(::rshooks::error::HookError::TooSmall)?;\n\
                 ::core::result::Result::Ok(Self {{\n\
                     {read_body}\
                 }})\n\
             }}\n\
         }}\n\
         #[automatically_derived]\n\
         impl ::rshooks::convert::FixedRead for {value_ty} {{\n\
             #[inline(always)]\n\
             fn read_exact(\n\
                 read: impl FnOnce(&mut [u8]) -> ::rshooks::error::Result<usize>,\n\
             ) -> ::rshooks::error::Result<Self> {{\n\
                 let mut __buf = [0u8; <Self as ::rshooks::convert::ToBytes>::MAX_LEN];\n\
                 let __written = read(&mut __buf)?;\n\
                 if __written == <Self as ::rshooks::convert::ToBytes>::MAX_LEN {{\n\
                     <Self as ::rshooks::convert::FromBytes>::read(&__buf)\n\
                 }} else {{\n\
                     ::core::result::Result::Err(::rshooks::error::HookError::TooSmall)\n\
                 }}\n\
             }}\n\
         }}\n\
         impl {value_ty} {{\n\
             /// Total encoded length in bytes (`docs/STATE_INTERFACE_DESIGN.md` §1.7:\n\
             /// fields concatenated in declaration order, no separators or length\n\
             /// prefixes).\n\
             pub const LEN: usize = <Self as ::rshooks::convert::ToBytes>::MAX_LEN;\n\
         }}\n"
    ));

    // --- alias-drift const asserts (§3 item 3's "pinned against alias
    // drift" rule) for every declared field, key and value alike.
    out.push_str("const _: () = {\n");
    for f in key_fields.iter().chain(value_fields.iter()) {
        let ty = tokens_to_string(&f.ty_tokens);
        out.push_str(&format!(
            "assert!(<{ty} as ::rshooks::si::SiFieldType>::TYPE_BYTE == {byte}u8, \
             \"rshooks: state_interface field '{name}' resolves to a different wire type than \
             its declared type token (check for a type alias mismatch)\");\n\
             assert!(<{ty} as ::rshooks::si::SiFieldType>::WIDTH == {width}usize, \
             \"rshooks: state_interface field '{name}' resolves to a different width than its \
             declared type token (check for a type alias mismatch)\");\n",
            byte = f.type_byte,
            width = f.width,
            name = f.name,
        ));
    }
    out.push_str("};\n");

    // --- the marker's `StateSpec` impl.
    let key_offsets = field_offsets(STATE_ID_PREFIX_LEN, key_fields);
    let (key_args, args_pat, key_body) = match (key_fields.first(), key_fields.len()) {
        (None, _) => ("()".to_string(), "_args", String::new()),
        (Some(f), 1) => {
            let (off, next) = key_offsets.first().copied().unwrap_or((0, 0));
            (
                tokens_to_string(&f.ty_tokens),
                "args",
                format!(
                    "if let Some(__f) = __buf.get_mut({off}..{next}) {{ \
                     ::rshooks::si::SiFieldType::write_si(args, __f); }}\n"
                ),
            )
        }
        (Some(_), _) => {
            let tys: Vec<String> = key_fields
                .iter()
                .map(|f| tokens_to_string(&f.ty_tokens))
                .collect();
            let mut body = String::new();
            for (i, (off, next)) in key_offsets.iter().enumerate() {
                body.push_str(&format!(
                    "if let Some(__f) = __buf.get_mut({off}..{next}) {{ \
                     ::rshooks::si::SiFieldType::write_si(&args.{i}, __f); }}\n"
                ));
            }
            (format!("({},)", tys.join(", ")), "args", body)
        }
    };

    let with_key_method = if key_fields.is_empty() {
        // 31 zero bytes: the 32-byte key minus the 1-byte State ID
        // (`docs/STATE_INTERFACE_DESIGN.md` §1.6 — a singleton's key is
        // `StateID || 31 zero bytes`).
        let zeros = vec!["0u8"; 31].join(", ");
        format!(
            "#[inline(always)]\n\
             fn with_key<__R>(_args: &Self::KeyArgs, f: impl ::core::ops::FnOnce(&::rshooks::state::EncodedStateKey) -> __R) -> __R {{\n\
                 f(const {{ &::rshooks::state::EncodedStateKey::from_short(&[{id}u8, {zeros}]) }})\n\
             }}\n"
        )
    } else {
        String::new()
    };

    out.push_str(&format!(
        "#[automatically_derived]\n\
         impl ::rshooks::decl::StateSpec for {marker} {{\n\
             type Value = {value_ty};\n\
             type KeyArgs = {key_args};\n\
             #[inline(always)]\n\
             fn encode_key({args_pat}: &Self::KeyArgs) -> ::rshooks::state::EncodedStateKey {{\n\
                 let mut __buf = [0u8; 32];\n\
                 if let Some(__b) = __buf.get_mut(0) {{ *__b = {id}u8; }}\n\
                 {key_body}\
                 ::rshooks::state::EncodedStateKey::new(__buf, 32)\n\
             }}\n\
             {with_key_method}\
         }}\n"
    ));

    out
}

/// Length in bytes of the state interface key's leading State ID byte
/// (`docs/STATE_INTERFACE_DESIGN.md` §1.6) — the macro-time twin of
/// `rshooks::si::STATE_ID_PREFIX_LEN`. Not feature-gated — see
/// [`state_interface_field_codegen`]'s doc comment for why.
const STATE_ID_PREFIX_LEN: usize = 1;

/// Computes `(start, end)` byte offsets for a sequence of
/// [`SiFieldSpec`]s, starting at `base`.
fn field_offsets(base: usize, fields: &[SiFieldSpec]) -> Vec<(usize, usize)> {
    let mut offsets = Vec::with_capacity(fields.len());
    let mut cursor = base;
    for f in fields {
        let next = cursor.wrapping_add(f.width);
        offsets.push((cursor, next));
        cursor = next;
    }
    offsets
}

/// A `proc_macro`-free (plain-`String`) view of one field's chain-carrier
/// JSON entry.
///
/// Kept separate from [`ParsedField`] (which holds live `proc_macro`
/// `TokenTree`s) so [`encode_chain_json`] — and its determinism and
/// JSON-escaping guarantees — can be unit tested: every `proc_macro` type
/// panics outside an active macro invocation.
enum ChainFieldJson {
    State {
        field: String,
        kind: &'static str,
        key: String,
        value: String,
    },
    Param {
        role: ParamKind,
        field: String,
        name: Option<String>,
        name_by: Option<String>,
        value: String,
        required: bool,
        /// Normalized token text of the `default = <expr>` expression, if
        /// declared, so downstream consumers can compare the actual
        /// default value, not merely its presence.
        default: Option<String>,
    },
    /// `#[state_interface(id = .., key(..), value(..))]`
    /// (`docs/STATE_INTERFACE_DESIGN.md` §5's `SiDecl`).
    StateInterface {
        field: String,
        id: u8,
        /// The declared `HookParameterName` bytes, uppercase hex.
        name_hex: String,
        /// The declared `HookParameterValue` bytes, uppercase hex.
        value_hex: String,
        /// Human-readable `(name: Type, ..)` display form of `key(..)`
        /// (`"()"` for a singleton).
        key: String,
        /// Human-readable `(name: Type, ..)` display form of `value(..)`.
        value: String,
    },
}

/// Builds one `#[state_interface(..)]` declaration's `HookParameterName`
/// bytes (`docs/STATE_INTERFACE_DESIGN.md` §1.3): `_SI` + version `0x00` +
/// State ID + key field count + one `(type byte, name length, name)`
/// descriptor per key field.
fn si_declaration_name_bytes(id: u8, key_fields: &[SiFieldSpec]) -> Vec<u8> {
    let mut out = vec![0x5Fu8, 0x53, 0x49, 0x00, id, key_fields.len() as u8];
    for f in key_fields {
        out.push(f.type_byte);
        out.push(f.name.len() as u8);
        out.extend_from_slice(f.name.as_bytes());
    }
    out
}

/// Builds one `#[state_interface(..)]` declaration's `HookParameterValue`
/// bytes (`docs/STATE_INTERFACE_DESIGN.md` §1.4): value field count + one
/// `(type byte, name length, name)` descriptor per value field. Distinct
/// from the runtime `HookStateData` encoding (§1.7, no count prefix) —
/// this is schema metadata, never written to a hook's own state.
fn si_declaration_value_bytes(value_fields: &[SiFieldSpec]) -> Vec<u8> {
    let mut out = vec![value_fields.len() as u8];
    for f in value_fields {
        out.push(f.type_byte);
        out.push(f.name.len() as u8);
        out.extend_from_slice(f.name.as_bytes());
    }
    out
}

/// Human-readable `(name: Type, ..)` display form of a key/value field
/// list — `"()"` for an empty list (a singleton's `key(..)`).
fn si_field_list_display(fields: &[SiFieldSpec]) -> String {
    let parts: Vec<String> = fields
        .iter()
        .map(|f| format!("{}: {}", f.name, tokens_to_string(&f.ty_tokens)))
        .collect();
    format!("({})", parts.join(", "))
}

/// Converts one already-parsed field into its plain-`String` JSON view.
fn field_to_chain_json(f: &ParsedField) -> ChainFieldJson {
    let field_name = f.name.to_string();
    let value = tokens_to_string(&f.value_ty);
    match &f.decl {
        FieldDecl::State { key } => match key {
            KeySpec::Const { expr } => ChainFieldJson::State {
                field: field_name,
                kind: "const",
                key: tokens_to_string(expr),
                value,
            },
            KeySpec::Keyed { ty } => ChainFieldJson::State {
                field: field_name,
                kind: "keyed",
                key: tokens_to_string(ty),
                value,
            },
        },
        FieldDecl::Param { param_kind, spec } => {
            let (name, name_by) = match &spec.name {
                NameSpec::Literal { literal } => (Some(literal.to_string()), None),
                NameSpec::Family { ty } => (None, Some(tokens_to_string(ty))),
            };
            ChainFieldJson::Param {
                role: *param_kind,
                field: field_name,
                name,
                name_by,
                value,
                required: spec.required,
                default: spec.default.as_ref().map(|expr| tokens_to_string(expr)),
            }
        }
        FieldDecl::StateInterface {
            id,
            key_fields,
            value_fields,
            ..
        } => ChainFieldJson::StateInterface {
            field: field_name,
            id: *id,
            name_hex: hex_upper(&si_declaration_name_bytes(*id, key_fields)),
            value_hex: hex_upper(&si_declaration_value_bytes(value_fields)),
            key: si_field_list_display(key_fields),
            value: si_field_list_display(value_fields),
        },
    }
}

/// Encodes the struct carrier JSON payload (contract §B1 item 6) from a
/// plain-`String` field list — see [`ChainFieldJson`] for why this boundary
/// exists.
fn encode_chain_json(
    struct_name: &str,
    description: Option<&str>,
    entries: &[ChainFieldJson],
) -> Result<Vec<u8>, String> {
    let mut state = Vec::new();
    let mut hook_params = Vec::new();
    let mut otxn_params = Vec::new();
    let mut state_interface = Vec::new();

    for entry in entries {
        match entry {
            ChainFieldJson::State {
                field,
                kind,
                key,
                value,
            } => {
                let mut obj = serde_json::Map::new();
                obj.insert("field".into(), field.clone().into());
                obj.insert("kind".into(), (*kind).into());
                obj.insert("key".into(), key.clone().into());
                obj.insert("value".into(), value.clone().into());
                state.push(serde_json::Value::Object(obj));
            }
            ChainFieldJson::Param {
                role,
                field,
                name,
                name_by,
                value,
                required,
                default,
            } => {
                let mut obj = serde_json::Map::new();
                obj.insert("field".into(), field.clone().into());
                obj.insert(
                    "name".into(),
                    name.clone().map_or(serde_json::Value::Null, Into::into),
                );
                obj.insert(
                    "name_by".into(),
                    name_by.clone().map_or(serde_json::Value::Null, Into::into),
                );
                obj.insert("value".into(), value.clone().into());
                obj.insert("required".into(), (*required).into());
                obj.insert(
                    "default".into(),
                    default.clone().map_or(serde_json::Value::Null, Into::into),
                );
                let entry = serde_json::Value::Object(obj);
                match role {
                    ParamKind::HookParam => hook_params.push(entry),
                    ParamKind::OtxnParam => otxn_params.push(entry),
                }
            }
            ChainFieldJson::StateInterface {
                field,
                id,
                name_hex,
                value_hex,
                key,
                value,
            } => {
                let mut obj = serde_json::Map::new();
                obj.insert("field".into(), field.clone().into());
                obj.insert("id".into(), (*id).into());
                obj.insert("name_hex".into(), name_hex.clone().into());
                obj.insert("value_hex".into(), value_hex.clone().into());
                obj.insert("key".into(), key.clone().into());
                obj.insert("value".into(), value.clone().into());
                state_interface.push(serde_json::Value::Object(obj));
            }
        }
    }

    let mut decls = serde_json::Map::new();
    decls.insert("state".into(), state.into());
    decls.insert("hook_params".into(), hook_params.into());
    decls.insert("otxn_params".into(), otxn_params.into());
    // Byte-identity with a pre-`unstable-state-interface` build: only emit
    // this key when non-empty, so a struct with no `#[state_interface]`
    // fields carries the exact same carrier JSON (and therefore the exact
    // same wasm) it always did.
    if !state_interface.is_empty() {
        decls.insert("state_interface".into(), state_interface.into());
    }

    let mut object = serde_json::Map::new();
    object.insert("schema".into(), "rshooks-chain-v2".into());
    object.insert("struct".into(), struct_name.into());
    object.insert(
        "description".into(),
        description.map_or(serde_json::Value::Null, |d| d.into()),
    );
    object.insert("decls".into(), decls.into());

    serde_json::to_vec(&serde_json::Value::Object(object))
        .map_err(|e| format!("#[hooks]: failed to serialize chain carrier JSON: {e}"))
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

    #[test]
    fn upper_camel_conversion() {
        assert_eq!(to_upper_camel("deposits"), "Deposits");
        assert_eq!(to_upper_camel("max_len"), "MaxLen");
        assert_eq!(to_upper_camel("a_b_c"), "ABC");
    }

    /// Pins the ordering `assemble_named_body` exists to guarantee: the
    /// struct item's own leading attributes appear directly before the
    /// outer struct's declaration (with nothing but a newline between
    /// them), and never before a namespace struct.
    #[test]
    fn leading_attrs_attach_only_to_the_outer_struct() {
        let rendered = assemble_named_body(
            "#[allow(dead_code)]",
            "struct VaultState {\nfoo: i32,\n}\n",
            "struct Vault {\nstate: VaultState,\n}\n",
        );

        let ns_pos = rendered
            .find("struct VaultState")
            .expect("namespace struct present");
        let attrs_pos = rendered
            .find("#[allow(dead_code)]")
            .expect("leading attrs present");
        let outer_pos = rendered
            .find("struct Vault {")
            .expect("outer struct present");

        assert!(
            ns_pos < attrs_pos,
            "the namespace struct must be emitted before the leading attrs, \
             not after: {rendered}"
        );
        assert!(
            attrs_pos < outer_pos,
            "the leading attrs must precede the outer struct: {rendered}"
        );
        assert_eq!(
            &rendered[attrs_pos + "#[allow(dead_code)]".len()..outer_pos],
            "\n",
            "only a newline may separate the leading attrs from the outer \
             struct they attach to: {rendered}"
        );
    }

    #[test]
    fn marker_name_shape() {
        assert_eq!(
            marker_name("Vault", 0, "deposits"),
            "__RshooksSpecVaultField0Deposits"
        );
    }

    #[test]
    fn marker_name_disambiguates_names_that_collapse_to_the_same_upper_camel() {
        // `foo_bar` and `foo__bar` both render to `FooBar` under
        // `to_upper_camel` — only the ordinal keeps their markers distinct.
        let a = marker_name("Vault", 0, "foo_bar");
        let b = marker_name("Vault", 1, "foo__bar");
        assert_ne!(a, b);
        assert_eq!(a, "__RshooksSpecVaultField0FooBar");
        assert_eq!(b, "__RshooksSpecVaultField1FooBar");
    }

    #[test]
    fn marker_name_strips_raw_identifier_prefix() {
        assert_eq!(
            marker_name("Vault", 2, "r#type"),
            "__RshooksSpecVaultField2Type"
        );
    }

    /// `encode_chain_json` operates on the plain-`String` [`ChainFieldJson`]
    /// view so it can be exercised here without a live `proc_macro` context.
    fn sample_entry() -> ChainFieldJson {
        ChainFieldJson::State {
            field: "deposits".to_string(),
            kind: "const",
            key: "KEY".to_string(),
            value: "DepositValue".to_string(),
        }
    }

    #[test]
    fn chain_json_is_deterministic() {
        let bytes1 =
            encode_chain_json("Vault", Some("a \"quoted\" desc"), &[sample_entry()]).expect("json");
        let bytes2 =
            encode_chain_json("Vault", Some("a \"quoted\" desc"), &[sample_entry()]).expect("json");
        assert_eq!(
            bytes1, bytes2,
            "chain JSON generation must be deterministic"
        );
    }

    #[test]
    fn chain_json_escapes_and_parses_back() {
        let bytes =
            encode_chain_json("Vault", Some("a \"quoted\" desc"), &[sample_entry()]).expect("json");
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(
            text.contains("a \\\"quoted\\\" desc"),
            "must escape embedded quotes: {text}"
        );
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        assert_eq!(value["schema"], "rshooks-chain-v2");
        assert_eq!(value["struct"], "Vault");
        assert_eq!(value["decls"]["state"][0]["field"], "deposits");
        assert_eq!(value["decls"]["state"][0]["kind"], "const");
        assert_eq!(value["decls"]["hook_params"], serde_json::json!([]));
    }

    #[test]
    fn chain_json_three_state_can_emit_style_presence() {
        // Omitted description -> JSON null, not absent and not "".
        let bytes = encode_chain_json("Vault", None, &[]).expect("json");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        assert!(value["description"].is_null());
        assert_eq!(value["decls"]["state"], serde_json::json!([]));
    }

    #[test]
    fn chain_json_param_entry_shape() {
        let entry = ChainFieldJson::Param {
            role: ParamKind::HookParam,
            field: "config".to_string(),
            name: Some("b\"CFG\"".to_string()),
            name_by: None,
            value: "Config".to_string(),
            required: false,
            default: Some("[0u8; 4]".to_string()),
        };
        let bytes = encode_chain_json("Vault", None, &[entry]).expect("json");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        let param = &value["decls"]["hook_params"][0];
        assert_eq!(param["field"], "config");
        assert_eq!(param["required"], false);
        assert_eq!(param["default"], "[0u8; 4]");
        assert!(param["name_by"].is_null());
    }

    #[test]
    fn chain_json_param_entry_without_default_is_null() {
        let entry = ChainFieldJson::Param {
            role: ParamKind::OtxnParam,
            field: "seat".to_string(),
            name: Some("b\"SEAT\"".to_string()),
            name_by: None,
            value: "AccountId".to_string(),
            required: false,
            default: None,
        };
        let bytes = encode_chain_json("Vault", None, &[entry]).expect("json");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        let param = &value["decls"]["otxn_params"][0];
        assert!(param["default"].is_null());
    }

    // --- `#[state_interface(..)]` (docs/STATE_INTERFACE_DESIGN.md) ---

    // This module's own import of the shared `classify_fixed_sti_type_text`/
    // `is_valid_interface_name` helpers (`crate::hooks_shared`) is
    // `#[cfg(feature = "unstable-state-interface")]`-gated (their only
    // caller here, `parse_state_interface_decl`, is too) — mirror that gate
    // here rather than on the whole module, so these three tests still
    // exist in both feature configurations at least as source, cfg'd out
    // cleanly.
    #[cfg(feature = "unstable-state-interface")]
    #[test]
    fn si_type_table_matches_the_design_docs_ten_rows() {
        assert_eq!(classify_fixed_sti_type_text("u8"), Some((0x10, 1)));
        assert_eq!(classify_fixed_sti_type_text("u16"), Some((0x01, 2)));
        assert_eq!(classify_fixed_sti_type_text("u32"), Some((0x02, 4)));
        assert_eq!(classify_fixed_sti_type_text("u64"), Some((0x03, 8)));
        assert_eq!(classify_fixed_sti_type_text("[u8;16]"), Some((0x04, 16)));
        assert_eq!(classify_fixed_sti_type_text("[u8;32]"), Some((0x05, 32)));
        assert_eq!(classify_fixed_sti_type_text("Hash"), Some((0x05, 32)));
        assert_eq!(classify_fixed_sti_type_text("AccountId"), Some((0x08, 20)));
        assert_eq!(classify_fixed_sti_type_text("[u8;20]"), Some((0x11, 20)));
        assert_eq!(
            classify_fixed_sti_type_text("CurrencyCode"),
            Some((0x1A, 20))
        );
        assert_eq!(classify_fixed_sti_type_text("XFL"), Some((0x80, 8)));
    }

    #[cfg(feature = "unstable-state-interface")]
    #[test]
    fn si_type_table_rejects_variable_width_and_unknown_types() {
        // `AmountBytes`/`Blob<N>`/`IssueBytes` are in the *signature*
        // parameter table but deliberately not this one (§1.5).
        assert_eq!(classify_fixed_sti_type_text("AmountBytes"), None);
        assert_eq!(classify_fixed_sti_type_text("IssueBytes"), None);
        assert_eq!(classify_fixed_sti_type_text("i64"), None);
        assert_eq!(classify_fixed_sti_type_text("String"), None);
    }

    #[cfg(feature = "unstable-state-interface")]
    #[test]
    fn si_field_name_validation_matches_the_signature_interfaces_charset() {
        assert!(is_valid_interface_name("account"));
        assert!(is_valid_interface_name("token"));
        assert!(is_valid_interface_name("abcdefghijklmnop")); // exactly 16
        assert!(!is_valid_interface_name("min_amount")); // underscore
        assert!(!is_valid_interface_name("1field")); // leading digit
        assert!(!is_valid_interface_name("")); // empty
        assert!(!is_valid_interface_name("abcdefghijklmnopq")); // 17 bytes
    }

    /// Builds a test-only `SiFieldSpec` with no type tokens — constructing
    /// real `proc_macro::TokenTree`s needs an active macro invocation (see
    /// `ChainFieldJson`'s own doc comment), so every test here only checks
    /// `name`/`type_byte`/`width`, never the rendered type text.
    fn si_field(name: &str, type_byte: u8, width: usize) -> SiFieldSpec {
        SiFieldSpec {
            name: name.to_string(),
            ty_tokens: Vec::new(),
            type_byte,
            width,
        }
    }

    /// Pins the design doc §7 spec vector: declaration name
    /// `5F534900000208076163636F756E740205746F6B656E`, declaration value
    /// `020306616D6F756E74020775706461746564`, for
    /// `key(account: AccountId, token: u32), value(amount: u64, updated: u32)`
    /// at State ID `0`.
    #[test]
    fn si_declaration_bytes_match_the_design_docs_spec_vector() {
        let key_fields = vec![si_field("account", 0x08, 20), si_field("token", 0x02, 4)];
        let value_fields = vec![si_field("amount", 0x03, 8), si_field("updated", 0x02, 4)];

        let name_hex = hex_upper(&si_declaration_name_bytes(0, &key_fields));
        assert_eq!(name_hex, "5F534900000208076163636F756E740205746F6B656E");

        let value_hex = hex_upper(&si_declaration_value_bytes(&value_fields));
        assert_eq!(value_hex, "020306616D6F756E74020775706461746564");
    }

    /// Pins a declaration using the `0x80` `XFL` type code (XAS-010d):
    /// declaration name `5F5349000301800472617465`, declaration value
    /// `010305636F756E74`, for `key(rate: XFL), value(count: u64)` at
    /// State ID `3`.
    #[test]
    fn si_declaration_bytes_cover_the_xfl_type_code() {
        let key_fields = vec![si_field("rate", 0x80, 8)];
        let value_fields = vec![si_field("count", 0x03, 8)];

        let name_hex = hex_upper(&si_declaration_name_bytes(3, &key_fields));
        assert_eq!(name_hex, "5F5349000301800472617465");

        let value_hex = hex_upper(&si_declaration_value_bytes(&value_fields));
        assert_eq!(value_hex, "010305636F756E74");
    }

    #[test]
    fn si_declaration_name_bytes_singleton_has_zero_key_fields() {
        let name_hex = hex_upper(&si_declaration_name_bytes(1, &[]));
        // `_SI\x00` + State ID 0x01 + key field count 0x00.
        assert_eq!(name_hex, "5F5349000100");
    }

    #[test]
    fn si_field_list_display_is_empty_parens_for_a_singleton() {
        assert_eq!(si_field_list_display(&[]), "()");
    }

    #[test]
    fn si_field_list_display_joins_name_type_pairs_with_commas() {
        // `ty_tokens` is empty in this helper (constructing real
        // `proc_macro::TokenTree`s needs an active macro invocation — see
        // `ChainFieldJson`'s own doc comment for why this module keeps its
        // unit-testable helpers `TokenTree`-shape-agnostic where it can);
        // this only pins the field separator/parenthesization shape.
        let fields = vec![si_field("account", 0x08, 20), si_field("token", 0x02, 4)];
        assert_eq!(si_field_list_display(&fields), "(account: , token: )");
    }
}
