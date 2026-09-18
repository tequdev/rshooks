//! Table-driven parity tests: vendored xahaud-hook headers' `#define`s /
//! enum members vs the corresponding `pub const`s in `rshooks-core`
//! (`docs/DESIGN.md` §4). One row per header/translation pair; the shared
//! [`check`] runner does extraction, type checking, and name-set/value
//! comparison via `common::assert_maps_match`.
//!
//! `tx_flags.h` is the one irregular row: some of its members
//! (`MPTokenIssuanceCreateFlags`) alias `ls_flags.h` values (`tfMPTCanLock =
//! lsfMPTCanLock`), so its row seeds the evaluation environment with
//! `ls_flags` first, then filters the comparison back down to just the
//! `tx_flags` names.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

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

#[derive(Clone, Copy)]
enum HeaderKind {
    Defines,
    EnumMembers,
}

fn extract_header(kind: HeaderKind, src: &str) -> Vec<(String, String)> {
    match kind {
        HeaderKind::Defines => extract_c_defines(src),
        HeaderKind::EnumMembers => extract_c_enum_members(src),
    }
}

struct Case {
    header_label: &'static str,
    header_kind: HeaderKind,
    header: &'static str,
    seed_header: Option<(HeaderKind, &'static str)>,
    rust_label: &'static str,
    rust: &'static str,
    rust_seed: Option<&'static str>,
    rust_ty: &'static str,
    extra: Option<fn(&BTreeMap<String, i64>)>,
}

const CASES: &[Case] = &[
    Case {
        header_label: "tts.h",
        header_kind: HeaderKind::Defines,
        header: TTS_HEADER,
        seed_header: None,
        rust_label: "tts.rs",
        rust: TTS_RUST,
        rust_seed: None,
        rust_ty: "u16",
        extra: None,
    },
    Case {
        header_label: "sfcodes.h",
        header_kind: HeaderKind::Defines,
        header: SFCODES_HEADER,
        seed_header: None,
        rust_label: "sfcodes.rs",
        rust: SFCODES_RUST,
        rust_seed: None,
        rust_ty: "u32",
        extra: None,
    },
    Case {
        header_label: "error.h",
        header_kind: HeaderKind::Defines,
        header: ERROR_HEADER,
        seed_header: None,
        rust_label: "error.rs",
        rust: ERROR_RUST,
        rust_seed: None,
        rust_ty: "i64",
        // The known irregular value, called out explicitly in both the
        // header and the Rust translation's doc comment.
        extra: Some(|env| assert_eq!(env["INVALID_FLOAT"], -10024)),
    },
    Case {
        header_label: "ls_flags.h",
        header_kind: HeaderKind::EnumMembers,
        header: LS_FLAGS_HEADER,
        seed_header: None,
        rust_label: "ls_flags.rs",
        rust: LS_FLAGS_RUST,
        rust_seed: None,
        rust_ty: "u32",
        extra: None,
    },
    Case {
        header_label: "tx_flags.h",
        header_kind: HeaderKind::EnumMembers,
        header: TX_FLAGS_HEADER,
        seed_header: Some((HeaderKind::EnumMembers, LS_FLAGS_HEADER)),
        rust_label: "tx_flags.rs",
        rust: TX_FLAGS_RUST,
        rust_seed: Some(LS_FLAGS_RUST),
        rust_ty: "u32",
        extra: None,
    },
];

fn check(case: &Case) {
    let own_header_defs = extract_header(case.header_kind, case.header);
    let filter: Option<BTreeSet<String>> = case
        .seed_header
        .is_some()
        .then(|| own_header_defs.iter().map(|(n, _)| n.clone()).collect());
    let header_defs = match case.seed_header {
        Some((seed_kind, seed_src)) => {
            let mut combined = extract_header(seed_kind, seed_src);
            combined.extend(own_header_defs);
            combined
        }
        None => own_header_defs,
    };
    let mut header_env = build_env(&header_defs);
    if let Some(names) = &filter {
        header_env.retain(|k, _| names.contains(k));
    }

    let own_rust_consts = extract_rust_consts(case.rust);
    for (name, ty, _) in &own_rust_consts {
        assert_eq!(
            ty, case.rust_ty,
            "{}: {name}: expected type {}, found {ty}",
            case.rust_label, case.rust_ty
        );
    }
    let rust_defs: Vec<(String, String)> = match case.rust_seed {
        Some(seed_src) => {
            let mut combined: Vec<(String, String)> = extract_rust_consts(seed_src)
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

    assert_maps_match(case.header_label, &header_env, case.rust_label, &rust_env);
    if let Some(extra) = case.extra {
        extra(&header_env);
    }
}

#[test]
fn vendored_header_consts_match_rust_translation() {
    for case in CASES {
        check(case);
    }
}
