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

/// One named constant and a named group of related constants, both
/// identical, field-for-field, to their [`crate::parse`] counterparts —
/// [`crate::parse::Define`]/[`crate::parse::EnumGroup`] — so the artifact
/// reuses the parsed shapes directly instead of duplicating them.
pub use crate::parse::{Define as ConstSpec, EnumGroup as ConstGroup};

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
}

/// The complete Hook API surface, parsed from the vendored headers: every
/// host function plus every constant family `rshooks-core` translates.
/// Serialized verbatim as `crates/rshooks-core/hook_api.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookApiSpec {
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
    }
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

    let error_codes = scan_defines(error_h);
    let tts = scan_defines(tts_h);
    let sfcodes = scan_defines(sfcodes_h);
    let ls_flags = scan_enum_groups(ls_flags_h)?;
    let tx_flags = scan_enum_groups(tx_flags_h)?;

    let hookapi_defines = scan_defines(hookapi_h);
    let keylet = hookapi_defines
        .iter()
        .filter(|d| d.name.starts_with("KEYLET_"))
        .cloned()
        .collect::<Vec<_>>();
    let compare = hookapi_defines
        .iter()
        .filter(|d| d.name.starts_with("COMPARE_"))
        .cloned()
        .collect::<Vec<_>>();

    let macro_defines = scan_defines(macro_h);
    let canonical = macro_defines
        .iter()
        .filter(|d| d.name == "tfCANONICAL")
        .cloned()
        .collect::<Vec<_>>();
    let at_family = macro_defines
        .iter()
        .filter(|d| d.name.starts_with("at"))
        .cloned()
        .collect::<Vec<_>>();
    let am_family = macro_defines
        .iter()
        .filter(|d| d.name.starts_with("am"))
        .cloned()
        .collect::<Vec<_>>();

    Ok(HookApiSpec {
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
