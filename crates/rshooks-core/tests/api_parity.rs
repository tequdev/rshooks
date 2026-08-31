//! Parity test: `vendor/xahaud-hook/extern.h` prototypes vs the `pub fn`
//! declarations in `src/api.rs`'s wasm Hook API import block
//! (`docs/DESIGN.md` §4). Checks name sets, order, parameter names/types,
//! and return types — not just "same names."
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
use common::{extract_c_prototypes, extract_rust_fn_signatures, extract_wasm_extern_block};

const HEADER: &str = include_str!("../vendor/xahaud-hook/extern.h");
const RUST: &str = include_str!("../src/api.rs");

#[test]
fn extern_h_matches_api_rs() {
    let c_protos = extract_c_prototypes(HEADER);
    assert_eq!(
        c_protos.len(),
        75,
        "expected 75 prototypes (incl. _g) in extern.h, found {}",
        c_protos.len()
    );

    let block = extract_wasm_extern_block(RUST);
    let rust_fns = extract_rust_fn_signatures(block);
    assert_eq!(
        rust_fns.len(),
        75,
        "expected 75 pub fn declarations (incl. _g) in api.rs's wasm extern block, found {}",
        rust_fns.len()
    );

    // Index-by-index comparison reports both missing entries and ordering
    // drift precisely.
    let c_names: Vec<&str> = c_protos.iter().map(|p| p.name.as_str()).collect();
    let rust_names: Vec<&str> = rust_fns.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        c_names, rust_names,
        "extern.h vs api.rs function name/order mismatch"
    );

    let mut mismatches = Vec::new();
    for (c, r) in c_protos.iter().zip(rust_fns.iter()) {
        if c.ret_ty != r.ret_ty {
            mismatches.push(format!(
                "{}: return type extern.h={} api.rs={}",
                c.name, c.ret_ty, r.ret_ty
            ));
        }
        if c.params.len() != r.params.len() {
            mismatches.push(format!(
                "{}: param count extern.h={} api.rs={}",
                c.name,
                c.params.len(),
                r.params.len()
            ));
            continue;
        }
        for (i, ((c_ty, c_name), (r_ty, r_name))) in
            c.params.iter().zip(r.params.iter()).enumerate()
        {
            if c_name != r_name {
                mismatches.push(format!(
                    "{}: param {i} name extern.h={c_name} api.rs={r_name}",
                    c.name
                ));
            }
            if c_ty != r_ty {
                mismatches.push(format!(
                    "{}: param {i} ({c_name}) type extern.h={c_ty} api.rs={r_ty}",
                    c.name
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "extern.h vs api.rs signature mismatches:\n{}",
        mismatches.join("\n")
    );
}
