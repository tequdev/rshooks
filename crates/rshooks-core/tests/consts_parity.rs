//! Parity tests: `vendor/xahaud-hook/hookapi.h`'s `KEYLET_*`/`COMPARE_*`
//! defines and `vendor/xahaud-hook/macro.h`'s constant-like defines
//! (`tfCANONICAL`, the `atACCOUNT` family, the `amAMOUNT` family) vs
//! `src/consts.rs` (`docs/DESIGN.md` §4).
//!
//! `macro.h` also defines plenty of function-like macros (`SBUF`,
//! `BUFFER_EQUAL`, `GUARD`, ...) and a couple of unrelated single-value
//! defines (`HOOKMACROS_INCLUDED`, `DEBUG`) that are deliberately NOT ported
//! to `consts.rs` (see that module's doc comment) — the function-like ones
//! are dropped by `extract_c_defines` itself, and the unrelated ones are
//! filtered out here by name.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

mod common;
use common::{assert_maps_match, build_env, extract_c_defines, extract_rust_consts};

const HOOKAPI_HEADER: &str = include_str!("../vendor/xahaud-hook/hookapi.h");
const MACRO_HEADER: &str = include_str!("../vendor/xahaud-hook/macro.h");
const RUST: &str = include_str!("../src/consts.rs");

/// `(name, type, expr)` triples, as returned by `extract_rust_consts`.
type RustConsts = Vec<(String, String, String)>;

fn rust_env() -> (RustConsts, BTreeMap<String, i64>) {
    let consts = extract_rust_consts(RUST);
    for (name, ty, _) in &consts {
        assert_eq!(ty, "u32", "{name}: expected type u32, found {ty}");
    }
    let defs: Vec<(String, String)> = consts
        .iter()
        .map(|(n, _, e)| (n.clone(), e.clone()))
        .collect();
    let env = build_env(&defs);
    (consts, env)
}

#[test]
fn hookapi_h_keylet_compare_match_consts() {
    let header_defs: Vec<(String, String)> = extract_c_defines(HOOKAPI_HEADER)
        .into_iter()
        .filter(|(name, _)| name.starts_with("KEYLET_") || name.starts_with("COMPARE_"))
        .collect();
    let header_env = build_env(&header_defs);

    let (all_rust_consts, all_rust_env) = rust_env();
    let rust_env: BTreeMap<String, i64> = all_rust_consts
        .iter()
        .filter(|(name, _, _)| name.starts_with("KEYLET_") || name.starts_with("COMPARE_"))
        .map(|(name, _, _)| (name.clone(), all_rust_env[name]))
        .collect();

    assert_maps_match(
        "hookapi.h (KEYLET_*/COMPARE_*)",
        &header_env,
        "consts.rs (KEYLET_*/COMPARE_*)",
        &rust_env,
    );
}

#[test]
fn macro_h_constants_match_consts() {
    let header_defs: Vec<(String, String)> = extract_c_defines(MACRO_HEADER)
        .into_iter()
        .filter(|(name, _)| {
            name == "tfCANONICAL" || name.starts_with("at") || name.starts_with("am")
        })
        .collect();
    let header_env = build_env(&header_defs);

    let (all_rust_consts, all_rust_env) = rust_env();
    let rust_env: BTreeMap<String, i64> = all_rust_consts
        .iter()
        .filter(|(name, _, _)| {
            name == "tfCANONICAL" || name.starts_with("at") || name.starts_with("am")
        })
        .map(|(name, _, _)| (name.clone(), all_rust_env[name]))
        .collect();

    assert_maps_match(
        "macro.h (tfCANONICAL/at*/am*)",
        &header_env,
        "consts.rs (tfCANONICAL/at*/am*)",
        &rust_env,
    );
}
