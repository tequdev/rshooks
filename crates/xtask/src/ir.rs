//! `hook_api.json` intermediate representation.
//!
//! [`build`] parses the vendored xahaud `hook/*.h` headers (via
//! [`crate::parse`]) exactly once into a single serializable [`HookApiSpec`]
//! tree. `gen_core` round-trips that tree through JSON — serializing it to
//! `crates/rshooks-core/hook_api.json`, then deserializing it back — before
//! handing it to the per-file generators in [`crate::codegen`], which never
//! touch header text or [`crate::parse`] types directly: `hook_api.json` is
//! the pipeline's real intermediate artifact
//! (`scripts/sync-vendor.sh` -> parse -> `hook_api.json` -> per-file codegen
//! -> `rustfmt`), not a documentation side-effect of it.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::parse::{ExternFn, scan_defines, scan_enum_groups, scan_extern_fns};

/// One Hook API function parameter, in declaration order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamSpec {
    /// The parameter's name, verbatim from `extern.h`.
    pub name: String,
    /// The parameter's C type token (e.g. `uint32_t`), verbatim from
    /// `extern.h` — unmapped, so `hook_api.json` genuinely quotes the header
    /// rather than a Rust-side reinterpretation of it.
    pub c_type: String,
}

/// One Hook API host function, from `extern.h`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSpec {
    /// The function's name, verbatim from `extern.h`.
    pub name: String,
    /// Ordered parameters.
    pub params: Vec<ParamSpec>,
    /// The C return type token (e.g. `int64_t`), unmapped.
    pub ret_c_type: String,
    /// The doc-comment text codegen attaches to this function: the full C
    /// prototype text (`<ret> <name>(<params>)`), quoting `extern.h`
    /// byte-for-byte modulo whitespace normalization.
    pub doc: String,
    /// The `HookApiVersion` this function was introduced in. Every function
    /// in the current vendored headers is `HookApiVersion` 0 (Guard-type);
    /// this field exists so a future Gas-type (`HookApiVersion` 1) surface
    /// can be modeled in this same schema without a breaking change.
    pub hook_api_version: u32,
}

/// One named constant, carrying its *unrendered* C literal/expression text
/// verbatim — [`crate::codegen`] renders it per family (plain decimal, 8-hex
/// -digit grouped, `(a << 16) + b` shift-add, or `ls_flags` alias path), so
/// the IR itself stays a faithful, un-opinionated quote of the header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstSpec {
    /// The constant's name, verbatim from its header.
    pub name: String,
    /// The constant's literal/expression text, verbatim from its header.
    pub c_expr: String,
}

/// A named group of related constants — one C enum block (`ls_flags.h`,
/// `tx_flags.h`) or one hand-picked `#define` family (`consts.rs`'s
/// `KEYLET_*`/`COMPARE_*`/... sections).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstGroup {
    /// The group's name (an enum's C name, or a family label such as
    /// `"KEYLET"`).
    pub name: String,
    /// The group's members, in declaration order.
    pub items: Vec<ConstSpec>,
}

/// The complete Hook API surface, parsed from the vendored headers: every
/// host function plus every constant family `rshooks-core` translates.
/// Serialized verbatim as `crates/rshooks-core/hook_api.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookApiSpec {
    /// The `HookApiVersion` this snapshot of the vendored headers describes.
    /// Currently always 0 (Guard-type) — the only `HookApiVersion` the
    /// vendored `hook/*.h` headers and this toolchain support.
    pub hook_api_version: u32,
    /// Every `extern.h` host function (`_g` plus the 74 Hook API functions),
    /// in header order.
    pub functions: Vec<FunctionSpec>,
    /// `error.h` error codes.
    pub error_codes: Vec<ConstSpec>,
    /// `tts.h` transaction type codes.
    pub tts: Vec<ConstSpec>,
    /// `sfcodes.h` serialized field codes.
    pub sfcodes: Vec<ConstSpec>,
    /// `ls_flags.h` ledger entry flag enums, one group per C enum.
    pub ls_flags: Vec<ConstGroup>,
    /// `tx_flags.h` transaction/account flag enums, one group per C enum.
    pub tx_flags: Vec<ConstGroup>,
    /// `hookapi.h` `KEYLET_*` defines.
    pub keylet: Vec<ConstSpec>,
    /// `hookapi.h` `COMPARE_*` defines.
    pub compare: Vec<ConstSpec>,
    /// `macro.h` `tfCANONICAL` define.
    pub canonical: Vec<ConstSpec>,
    /// `macro.h` `atACCOUNT` family defines.
    pub at_family: Vec<ConstSpec>,
    /// `macro.h` `amAMOUNT` family defines.
    pub am_family: Vec<ConstSpec>,
}

/// The original `extern.h` prototype text, as it appears in the doc comment
/// provenance line (`/// C: \`<this>\` (extern.h)`) — built from the C (not
/// Rust-mapped) types so the doc genuinely quotes the header.
fn c_prototype_text(f: &ExternFn) -> String {
    let params = f
        .params
        .iter()
        .map(|(ty, name)| format!("{ty} {name}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{} {}({params})", f.ret_c_ty, f.name)
}

fn function_spec(f: &ExternFn) -> FunctionSpec {
    FunctionSpec {
        name: f.name.clone(),
        params: f
            .params
            .iter()
            .map(|(ty, name)| ParamSpec {
                name: name.clone(),
                c_type: ty.clone(),
            })
            .collect(),
        ret_c_type: f.ret_c_ty.clone(),
        doc: c_prototype_text(f),
        // All vendored `extern.h` functions are HookApiVersion 0 today.
        hook_api_version: 0,
    }
}

/// Parses `defines` into [`ConstSpec`]s, in file order.
fn const_specs(defines: &[crate::parse::Define]) -> Vec<ConstSpec> {
    defines
        .iter()
        .map(|d| ConstSpec {
            name: d.name.clone(),
            c_expr: d.value.clone(),
        })
        .collect()
}

/// Parses `groups` into [`ConstGroup`]s, in file order.
fn const_groups(groups: &[crate::parse::EnumGroup]) -> Vec<ConstGroup> {
    groups
        .iter()
        .map(|g| ConstGroup {
            name: g.name.clone(),
            items: g
                .members
                .iter()
                .map(|m| ConstSpec {
                    name: m.name.clone(),
                    c_expr: m.value.clone(),
                })
                .collect(),
        })
        .collect()
}

/// Builds the complete [`HookApiSpec`] from the eight vendored headers'
/// source text (`docs/DESIGN.md` §4).
#[allow(clippy::too_many_arguments)]
pub fn build(
    error_h: &str,
    tts_h: &str,
    ls_flags_h: &str,
    tx_flags_h: &str,
    sfcodes_h: &str,
    hookapi_h: &str,
    macro_h: &str,
    extern_h: &str,
) -> Result<HookApiSpec> {
    let functions = scan_extern_fns(extern_h)?
        .iter()
        .map(function_spec)
        .collect();

    let error_codes = const_specs(&scan_defines(error_h));
    let tts = const_specs(&scan_defines(tts_h));
    let sfcodes = const_specs(&scan_defines(sfcodes_h));
    let ls_flags = const_groups(&scan_enum_groups(ls_flags_h)?);
    let tx_flags = const_groups(&scan_enum_groups(tx_flags_h)?);

    let hookapi_defines = scan_defines(hookapi_h);
    let keylet = const_specs(
        &hookapi_defines
            .iter()
            .filter(|d| d.name.starts_with("KEYLET_"))
            .cloned()
            .collect::<Vec<_>>(),
    );
    let compare = const_specs(
        &hookapi_defines
            .iter()
            .filter(|d| d.name.starts_with("COMPARE_"))
            .cloned()
            .collect::<Vec<_>>(),
    );

    let macro_defines = scan_defines(macro_h);
    let canonical = const_specs(
        &macro_defines
            .iter()
            .filter(|d| d.name == "tfCANONICAL")
            .cloned()
            .collect::<Vec<_>>(),
    );
    let at_family = const_specs(
        &macro_defines
            .iter()
            .filter(|d| d.name.starts_with("at"))
            .cloned()
            .collect::<Vec<_>>(),
    );
    let am_family = const_specs(
        &macro_defines
            .iter()
            .filter(|d| d.name.starts_with("am"))
            .cloned()
            .collect::<Vec<_>>(),
    );

    Ok(HookApiSpec {
        hook_api_version: 0,
        functions,
        error_codes,
        tts,
        sfcodes,
        ls_flags,
        tx_flags,
        keylet,
        compare,
        canonical,
        at_family,
        am_family,
    })
}
