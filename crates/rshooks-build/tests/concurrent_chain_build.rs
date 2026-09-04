//! Regression test for the shared cargo target-directory race: two
//! `rshooks build` chain builds of the same package, publishing to
//! different `--out` roots, share the same underlying cargo `--target-dir`
//! (`BuildPlan::private_target_dir`). Without a lock keyed on that shared
//! directory, one build's cargo invocation can overwrite the other's
//! artifact between compiling it and reading it back, silently mixing up
//! which entry's wasm ends up under which `--out` root.
//!
//! This drives real `cargo build`/`cargo rustc` invocations against
//! `examples/16_typed-results` (two `#[hook]` entries, so a mix-up between
//! entries is actually observable), so it is slower than the unit tests in
//! `chain_build.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rshooks_build::chain_build::{ChainBuildArgs, run};

fn typed_results_manifest() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/16_typed-results/Cargo.toml");
    manifest
        .canonicalize()
        .expect("examples/16_typed-results/Cargo.toml exists")
}

#[allow(deprecated)]
fn build_args(manifest_path: &Path, out: &Path) -> ChainBuildArgs {
    ChainBuildArgs {
        manifest_path: Some(manifest_path.to_path_buf()),
        out: Some(out.to_path_buf()),
        ..Default::default()
    }
}

/// Reads every `*.wasm` file directly under `<out_root>/current`, keyed by
/// file name, so two builds' outputs can be compared entry-by-entry.
fn read_current_wasms(out_root: &Path) -> BTreeMap<String, Vec<u8>> {
    let current = out_root.join("current");
    let mut wasms = BTreeMap::new();
    for entry in std::fs::read_dir(&current)
        .unwrap_or_else(|error| panic!("reading {}: {error}", current.display()))
    {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("wasm") {
            let name = entry.file_name().to_string_lossy().into_owned();
            let bytes = std::fs::read(&path)
                .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
            wasms.insert(name, bytes);
        }
    }
    assert_eq!(
        wasms.len(),
        2,
        "expected both `16_typed-results` entries (`deposit`, `reset`) under {}",
        current.display()
    );
    wasms
}

#[test]
fn concurrent_builds_with_different_out_roots_do_not_corrupt_each_other() {
    let manifest_path = typed_results_manifest();
    let tmp = tempfile::tempdir().expect("tempdir");

    // Serial reference build: the correct, uncontended output every
    // concurrent build below must match byte-for-byte.
    let out_serial = tmp.path().join("out-serial");
    run(&build_args(&manifest_path, &out_serial)).expect("serial reference build succeeds");
    let reference = read_current_wasms(&out_serial);

    let out_a = tmp.path().join("out-a");
    let out_b = tmp.path().join("out-b");
    let manifest_a = manifest_path.clone();
    let manifest_b = manifest_path.clone();
    let args_a = build_args(&manifest_a, &out_a);
    let args_b = build_args(&manifest_b, &out_b);

    let handle_a = std::thread::spawn(move || run(&args_a));
    let handle_b = std::thread::spawn(move || run(&args_b));

    let result_a = handle_a.join().expect("build a thread did not panic");
    let result_b = handle_b.join().expect("build b thread did not panic");

    result_a.unwrap_or_else(|error| panic!("concurrent build a failed: {error:#}"));
    result_b.unwrap_or_else(|error| panic!("concurrent build b failed: {error:#}"));

    let wasms_a = read_current_wasms(&out_a);
    let wasms_b = read_current_wasms(&out_b);

    assert_eq!(
        wasms_a, reference,
        "concurrent build a must match the serial reference entry-for-entry"
    );
    assert_eq!(
        wasms_b, reference,
        "concurrent build b must match the serial reference entry-for-entry"
    );
}
