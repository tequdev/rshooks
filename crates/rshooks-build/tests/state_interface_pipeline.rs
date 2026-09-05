//! The one CI-runnable (no live node) link from a real
//! `#[state_interface(..)]` attribute all the way through to its declared
//! `HookParameterName`/`HookParameterValue` hex: `hooks_struct.rs`'s own
//! unit tests hand-build `SiFieldSpec`s (bypassing `parse_si_field_list`/
//! `classify_si_type` — `proc_macro::TokenStream`/`Span` cannot be
//! constructed outside an active macro invocation, so a token-stream-level
//! unit test isn't possible there), and `rshooks-testenv`'s integration
//! test only checks the resulting *runtime* state bytes, never the
//! *declaration* hex (which the hook binary itself never materializes —
//! see `docs/STATE_INTERFACE_DESIGN.md` §4). This test instead runs the
//! real `rshooks build` pipeline against `examples/20_state-interface`
//! (real `#[state_interface(..)]` attribute -> real macro parse -> real
//! wasm carrier -> real `sethook.template.json`) and asserts the result
//! against `docs/STATE_INTERFACE_DESIGN.md` §7's pinned spec vector.
//!
//! Test code is exempt from the workspace's panic-freedom lints (`docs/DESIGN.md`
//! §8): `unwrap`/`expect` on a known-good fixture is idiomatic here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use rshooks_build::chain_build::{ChainBuildArgs, run};

fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `<repo>/crates/rshooks-build`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root resolves")
}

#[test]
fn example_20_declares_the_design_docs_spec_vector_hex() {
    let root = repo_root();
    let manifest = root.join("examples/20_state-interface/Cargo.toml");
    let out = root.join("examples/20_state-interface/out");

    let args = ChainBuildArgs {
        manifest_path: Some(manifest),
        out: Some(out.clone()),
        ..ChainBuildArgs::default()
    };
    run(&args).expect("examples/20_state-interface builds through the real rshooks pipeline");

    let template_bytes = std::fs::read(out.join("current").join("sethook.template.json"))
        .expect("sethook.template.json was written");
    let template: serde_json::Value = serde_json::from_slice(&template_bytes).expect("valid json");
    let params = template["Hooks"][0]["Hook"]["HookParameters"]
        .as_array()
        .expect("HookParameters array");
    assert_eq!(params.len(), 2);

    // `balances(id=0): key(account: AccountId, token: u32), value(amount: u64, updated: u32)`.
    assert_eq!(
        params[0]["HookParameter"]["HookParameterName"],
        "5F534900000208076163636F756E740205746F6B656E"
    );
    assert_eq!(
        params[0]["HookParameter"]["HookParameterValue"],
        "020306616D6F756E74020775706461746564"
    );

    // `config(id=1): value(paused: u8)` — a singleton (no key fields).
    assert_eq!(
        params[1]["HookParameter"]["HookParameterName"],
        "5F5349000100"
    );
    assert_eq!(
        params[1]["HookParameter"]["HookParameterValue"],
        "011006706175736564"
    );
}
