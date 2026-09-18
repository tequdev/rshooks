//! Table-driven parity tests: vendored xahaud-hook headers' `#define`s /
//! enum members vs the corresponding `pub const`s in `rshooks-core`
//! (`docs/DESIGN.md` §4). One row per header/translation pair; the shared
//! [`check`] runner does extraction, type checking, and name-set/value
//! comparison via `common::assert_maps_match`.
//!
//! `tx_flags.h` is irregular: some of its members (`MPTokenIssuanceCreateFlags`)
//! alias `ls_flags.h` values (`tfMPTCanLock = lsfMPTCanLock`), so its row
//! seeds the evaluation environment with `ls_flags` first, then filters the
//! comparison back down to just the `tx_flags` names.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeSet;

mod common;
use common::{
    assert_maps_match, build_env, extract_c_defines, extract_c_enum_members, extract_rust_consts,
};

const TTS_HEADER: &str = include_str!("../vendor/xahaud-hook/tts.h");
const TTS_RUST: &str = include_str!("../src/tts.rs");
const SFCODES_HEADER: &str = include_str!("../vendor/xahaud-hook/sfcodes.h");
const SFCODES_RUST: &str = include_str!("../src/sfcodes.rs");
const ERROR_HEADER: &str = include_str!("../vendor/xahaud-hook/error.h");
const ERROR_RUST: &str = include_str!("../src/error.rs");
const LS_FLAGS_HEADER: &str = include_str!("../vendor/xahaud-hook/ls_flags.h");
const LS_FLAGS_RUST: &str = include_str!("../src/ls_flags.rs");
const TX_FLAGS_HEADER: &str = include_str!("../vendor/xahaud-hook/tx_flags.h");
const TX_FLAGS_RUST: &str = include_str!("../src/tx_flags.rs");

struct Case {
    header_label: &'static str,
    header: &'static str,
    extract_header: fn(&str) -> Vec<(String, String)>,
    rust_label: &'static str,
    rust: &'static str,
    rust_ty: &'static str,
    /// `(seed header source, seed rust source)`: extra entries that seed the
    /// evaluation environment but are filtered back out of the final
    /// comparison, for `tx_flags`'s aliasing into `ls_flags`.
    seed: Option<(&'static str, &'static str)>,
}

const CASES: &[Case] = &[
    Case {
        header_label: "tts.h",
        header: TTS_HEADER,
        extract_header: extract_c_defines,
        rust_label: "tts.rs",
        rust: TTS_RUST,
        rust_ty: "u16",
        seed: None,
    },
    Case {
        header_label: "sfcodes.h",
        header: SFCODES_HEADER,
        extract_header: extract_c_defines,
        rust_label: "sfcodes.rs",
        rust: SFCODES_RUST,
        rust_ty: "u32",
        seed: None,
    },
    Case {
        header_label: "error.h",
        header: ERROR_HEADER,
        extract_header: extract_c_defines,
        rust_label: "error.rs",
        rust: ERROR_RUST,
        rust_ty: "i64",
        seed: None,
    },
    Case {
        header_label: "ls_flags.h",
        header: LS_FLAGS_HEADER,
        extract_header: extract_c_enum_members,
        rust_label: "ls_flags.rs",
        rust: LS_FLAGS_RUST,
        rust_ty: "u32",
        seed: None,
    },
    Case {
        header_label: "tx_flags.h",
        header: TX_FLAGS_HEADER,
        extract_header: extract_c_enum_members,
        rust_label: "tx_flags.rs",
        rust: TX_FLAGS_RUST,
        rust_ty: "u32",
        seed: Some((LS_FLAGS_HEADER, LS_FLAGS_RUST)),
    },
];

fn check(case: &Case) {
    let own_header_defs = (case.extract_header)(case.header);
    let filter: Option<BTreeSet<String>> = case
        .seed
        .is_some()
        .then(|| own_header_defs.iter().map(|(n, _)| n.clone()).collect());
    let header_defs = match case.seed {
        Some((seed_header, _)) => {
            let mut combined = (case.extract_header)(seed_header);
            combined.extend(own_header_defs);
            combined
        }
        None => own_header_defs,
    };
    let mut header_env = build_env(&header_defs);
    if let Some(names) = &filter {
        header_env.retain(|k, _| names.contains(k));
    }
    assert!(
        !header_env.is_empty(),
        "{}: no entries extracted",
        case.header_label
    );

    let own_rust_consts = extract_rust_consts(case.rust);
    for (name, ty, _) in &own_rust_consts {
        assert_eq!(
            ty, case.rust_ty,
            "{}: {name}: expected type {}, found {ty}",
            case.rust_label, case.rust_ty
        );
    }
    let rust_defs: Vec<(String, String)> = match case.seed {
        Some((_, seed_rust)) => {
            let mut combined: Vec<(String, String)> = extract_rust_consts(seed_rust)
                .into_iter()
                .map(|(n, _, e)| (n, e))
                .collect();
            combined.extend(own_rust_consts.into_iter().map(|(n, _, e)| (n, e)));
            combined
        }
        None => own_rust_consts
            .into_iter()
            .map(|(n, _, e)| (n, e))
            .collect(),
    };
    let mut rust_env = build_env(&rust_defs);
    if let Some(names) = &filter {
        rust_env.retain(|k, _| names.contains(k));
    }
    assert!(
        !rust_env.is_empty(),
        "{}: no entries extracted",
        case.rust_label
    );

    assert_maps_match(case.header_label, &header_env, case.rust_label, &rust_env);
}

#[test]
fn vendored_header_consts_match_rust_translation() {
    for case in CASES {
        check(case);
    }
}

#[test]
fn error_h_invalid_float_is_the_documented_irregular_value() {
    // The known irregular value, called out explicitly in both the header
    // and the Rust translation's doc comment.
    let env = build_env(&extract_c_defines(ERROR_HEADER));
    assert_eq!(env["INVALID_FLOAT"], -10024);
}
