//! `#[hooks]` on a struct — the chain-declaration half of the multi-hook
//! attribute macro pair. See `docs/MULTI_HOOK_STRUCT_DESIGN.md` §5.1/§5.4
//! and the v0.2 implementation contract §B1 for the normative grammar and
//! generated-output shape; this module implements it.
//!
//! A `#[hooks]` struct is a **declaration container**, never a runtime
//! value with real fields (see the design doc §4.3): every field is a
//! zero-sized handle (`State<V>` / `HookParam<V>` / `OtxnParam<V>`) whose
//! *type* the macro rewrites to carry a field-unique marker (§5.4) so that
//! two fields sharing a value type still get distinct, statically-checked
//! accessors.
//!
//! # Why hand-rolled, not `syn`/`quote`
//!
//! Same rationale as every other macro in this crate (see the crate doc
//! comment in `lib.rs`): the accepted struct/field shape is small and fixed
//! (§5.1's shape table), so a single bounded-lookahead pass over the token
//! buffer is enough — no general item/type parser needed. [`crate::shape`]
//! is not reused here because it derives structs meant to be *read back*
//! (`FromBytes`/`ToBytes` on real fields); this module's fields carry no
//! bytes at all and its field grammar (attribute-driven key/name specs) is
//! materially different.

use proc_macro::{Delimiter, Ident, Span, TokenStream, TokenTree};

use crate::hooks_shared::{
    AttrEntry, is_punct, parse_attr_entries, parse_balanced_angle, parse_string_value,
    split_top_level_commas, to_upper_camel,
};
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
                description = Some(parse_string_value(value.as_deref(), key_span, mac, "description")?);
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
    State { key: KeySpec },
    Param { param_kind: ParamKind, spec: ParamSpecDecl },
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
            _ => return Err(err(Span::call_site(), "malformed attribute before `struct`")),
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
                    if matches!(id.to_string().as_str(), "state" | "hook_param" | "otxn_param") =>
                {
                    if decl_attr.is_some() {
                        return Err(err(
                            id.span(),
                            "#[hooks]: a field must carry exactly one of `#[state]`, \
                             `#[hook_param]`, `#[otxn_param]`",
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
                return Err(err(other.span(), "#[hooks]: expected `:` after the field name"));
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
        while tokens.get(i).is_some_and(|tt| !is_punct(tt, ',')) {
            i = i.wrapping_add(1);
        }
        let ty_tokens = tokens.get(ty_start..i).unwrap_or_default().to_vec();
        if i < tokens.len() {
            i = i.wrapping_add(1); // trailing `,`
        }

        let Some((kind, attr_ident, args)) = decl_attr else {
            return Err(err(
                field_name.span(),
                "#[hooks]: every chain-struct field must carry exactly one of \
                 `#[state(..)]`, `#[hook_param(..)]`, `#[otxn_param(..)]`",
            ));
        };

        let (wrapper, value_ty) = parse_field_type(&ty_tokens, kind, &field_name)?;
        let decl = parse_field_decl(kind, &args, &attr_ident, wrapper)?;

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
        "state" => "State",
        "hook_param" => "HookParam",
        _ => "OtxnParam",
    };

    let mut i = 0usize;
    if is_punct(tokens.first().ok_or_else(|| bad_field_type(field_name))?, ':')
        && matches!(tokens.get(1), Some(tt) if is_punct(tt, ':'))
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

/// Parses a `#[state(..)]`/`#[hook_param(..)]`/`#[otxn_param(..)]`
/// argument list into a [`FieldDecl`].
fn parse_field_decl(
    kind: &str,
    args: &[TokenTree],
    attr_ident: &TokenTree,
    _wrapper: &str,
) -> Result<FieldDecl, TokenStream> {
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
                        return Err(err(key_span, "#[state]: `key` requires a value: `key = <expr>`"));
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
                let literal = match tokens.as_slice() {
                    [tt @ TokenTree::Literal(lit)] if lit.to_string().starts_with("b\"") => {
                        tt.clone()
                    }
                    _ => {
                        return Err(err(
                            key_span,
                            "expected a byte-string literal, e.g. `name = b\"CFG\"`",
                        ));
                    }
                };
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
                required = true;
                required_span = Some(key_span);
            }
            "default" => {
                let Some(expr) = value else {
                    return Err(err(key_span, "expected `default = <expr>`"));
                };
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

    let mut out = String::new();

    // 1. The struct itself, fields rewritten to the marker-injected type.
    out.push_str(&leading_attrs_text);
    out.push('\n');
    match &parsed.body {
        StructBody::Unit => {
            out.push_str(&format!("{vis_text} struct {struct_name};\n"));
        }
        StructBody::Named(fields) => {
            out.push_str(&format!("{vis_text} struct {struct_name} {{\n"));
            for f in fields {
                out.push_str(&tokens_to_string(&f.other_attrs));
                out.push('\n');
                out.push_str(&tokens_to_string(&f.vis));
                out.push(' ');
                out.push_str(&f.name.to_string());
                out.push_str(": ");
                out.push_str(&rewritten_field_type(&struct_name, f));
                out.push_str(",\n");
            }
            out.push_str("}\n");
        }
    }

    // 2. Per-field marker ZST + spec trait impl(s).
    for f in fields {
        out.push_str(&field_marker_and_impls(&struct_name, f));
    }

    // 3. Named-field structs only: a same-name static value binding.
    if let StructBody::Named(fields) = &parsed.body {
        out.push_str(&format!(
            "#[doc(hidden)]\n#[allow(non_upper_case_globals)]\n\
             {vis_text} static {struct_name}: {struct_name} = {struct_name} {{\n"
        ));
        for f in fields {
            let wrapper = match &f.decl {
                FieldDecl::State { .. } => "State",
                FieldDecl::Param { param_kind, .. } => param_kind.wrapper(),
            };
            out.push_str(&format!(
                "{field}: ::rshooks::decl::{wrapper}::new(),\n",
                field = f.name
            ));
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

/// The rewritten field type text: `::rshooks::decl::<Wrapper><V, __Marker>`.
fn rewritten_field_type(struct_name: &str, f: &ParsedField) -> String {
    let wrapper = match &f.decl {
        FieldDecl::State { .. } => "State",
        FieldDecl::Param { param_kind, .. } => param_kind.wrapper(),
    };
    let marker = marker_name(struct_name, &f.name.to_string());
    let value_ty = tokens_to_string(&f.value_ty);
    format!("::rshooks::decl::{wrapper}<{value_ty}, {marker}>")
}

fn marker_name(struct_name: &str, field_name: &str) -> String {
    format!("__RshooksSpec{struct_name}{}", to_upper_camel(field_name))
}

/// The marker ZST declaration plus its `StateSpec`/`ParamSpec` (+
/// `ParamDefault`/`ParamRequired`) impl(s) for one field.
fn field_marker_and_impls(struct_name: &str, f: &ParsedField) -> String {
    let marker = marker_name(struct_name, &f.name.to_string());
    let value_ty = tokens_to_string(&f.value_ty);
    let mut out = format!("#[doc(hidden)]\npub struct {marker};\n");

    match &f.decl {
        FieldDecl::State { key } => {
            let (key_args, encode_body) = match key {
                KeySpec::Const { expr } => {
                    let expr_text = tokens_to_string(expr);
                    (
                        "()".to_string(),
                        format!("::rshooks::state::StateKeyEncode::encode({expr_text})"),
                    )
                }
                KeySpec::Keyed { ty } => {
                    let ty_text = tokens_to_string(ty);
                    (
                        ty_text,
                        "::rshooks::state::StateKeyEncode::encode(args)".to_string(),
                    )
                }
            };
            let args_pat = if key_args == "()" { "_args" } else { "args" };
            out.push_str(&format!(
                "#[automatically_derived]\n\
                 impl ::rshooks::decl::StateSpec for {marker} {{\n\
                     type Value = {value_ty};\n\
                     type KeyArgs = {key_args};\n\
                     #[inline(always)]\n\
                     fn encode_key({args_pat}: &Self::KeyArgs) -> ::rshooks::state::EncodedStateKey {{\n\
                         {encode_body}\n\
                     }}\n\
                 }}\n"
            ));
        }
        FieldDecl::Param { spec, .. } => {
            let (name_args, with_name_body, extra) = match &spec.name {
                NameSpec::Literal { literal } => (
                    "()".to_string(),
                    format!("f({})", literal),
                    String::new(),
                ),
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
    }

    out
}

/// A `proc_macro`-free (plain-`String`) view of one field's chain-carrier
/// JSON entry.
///
/// Kept separate from [`ParsedField`] (which holds live `proc_macro`
/// `TokenTree`s) purely so [`encode_chain_json`] — and its determinism and
/// JSON-escaping guarantees — can be unit tested: every `proc_macro` type
/// (`Span`, `Ident`, `TokenStream::parse`, ...) panics outside an active
/// macro invocation, which is why every *other* module in this crate that
/// has unit tests (`sha256`, `base58`, `xfl_literal`, `metadata`'s
/// `canonical_name`) tests a plain-Rust-typed core, never `proc_macro`
/// types directly — this follows the same convention.
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
        has_default: bool,
    },
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
                has_default: spec.default.is_some(),
            }
        }
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
                has_default,
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
                obj.insert("has_default".into(), (*has_default).into());
                let entry = serde_json::Value::Object(obj);
                match role {
                    ParamKind::HookParam => hook_params.push(entry),
                    ParamKind::OtxnParam => otxn_params.push(entry),
                }
            }
        }
    }

    let mut decls = serde_json::Map::new();
    decls.insert("state".into(), state.into());
    decls.insert("hook_params".into(), hook_params.into());
    decls.insert("otxn_params".into(), otxn_params.into());

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

    #[test]
    fn marker_name_shape() {
        assert_eq!(marker_name("Vault", "deposits"), "__RshooksSpecVaultDeposits");
    }

    /// `encode_chain_json` operates on the plain-`String` [`ChainFieldJson`]
    /// view precisely so it can be exercised here without any live
    /// `proc_macro` context (`Span`/`Ident`/`TokenStream::parse` all panic
    /// outside an actual macro invocation — see [`ChainFieldJson`]'s doc
    /// comment).
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
        assert_eq!(bytes1, bytes2, "chain JSON generation must be deterministic");
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
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).expect("valid json");
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
            has_default: true,
        };
        let bytes = encode_chain_json("Vault", None, &[entry]).expect("json");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid json");
        let param = &value["decls"]["hook_params"][0];
        assert_eq!(param["field"], "config");
        assert_eq!(param["required"], false);
        assert_eq!(param["has_default"], true);
        assert!(param["name_by"].is_null());
    }
}
