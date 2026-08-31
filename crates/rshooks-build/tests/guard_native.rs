//! Tests for the vendored upstream guard checker (`docs/DESIGN.md` §6.5):
//! integrity of the vendored files, and behavior of
//! [`rshooks_build::validate_guards_native`] against small hand-authored
//! fixtures covering accept/reject/exception.
//!
//! Test code is exempt from the workspace's panic-freedom lints (`docs/DESIGN.md`
//! §8): `unwrap`/`expect` on a known-good fixture is idiomatic here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use rshooks_build::{NativeGuardError, Options, clean, validate_guards_native};

/// Drift tripwire against `vendor/xahaud/SHA256SUMS` (the source of truth for
/// the vendored hashes, regenerated only by `scripts/sync-vendor.sh`): an
/// accidental edit to the vendored upstream headers, or a corrupted
/// re-download, fails loudly instead of silently diverging from a real
/// xahaud node.
#[test]
fn vendored_files_match_recorded_sha256() {
    use sha2::{Digest, Sha256};

    fn sha256_hex(path: &str) -> String {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    let sums = std::fs::read_to_string("vendor/xahaud/SHA256SUMS")
        .expect("reading vendor/xahaud/SHA256SUMS");
    let mut checked = 0;
    for line in sums.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (want, name) = line
            .split_once("  ")
            .unwrap_or_else(|| panic!("malformed SHA256SUMS line: {line:?}"));
        let path = format!("vendor/xahaud/{name}");
        let got = sha256_hex(&path);
        assert_eq!(
            got, want,
            "{path} sha256 mismatch — the vendored file has drifted from \
             vendor/xahaud/SHA256SUMS; never hand-edit vendored files, re-sync \
             with scripts/sync-vendor.sh (see VENDOR.md)"
        );
        checked += 1;
    }
    assert_eq!(
        checked, 3,
        "expected exactly 3 entries in vendor/xahaud/SHA256SUMS"
    );
}

/// A minimal but structurally realistic Guard-type hook: it imports `_g` and
/// actually calls it in a correctly-guarded loop, so the import survives the
/// cleaner's reachability GC (a bare `import "env" "_g"` that is never called
/// gets garbage-collected). Padded with a data segment past the checker's
/// 63-byte minimum.
const VALID_GUARDED_HOOK: &str = r#"
(module
  (import "env" "_g" (func $g (param i32 i32) (result i32)))
  (import "env" "accept" (func $accept (param i32 i32 i64) (result i64)))
  (memory 1)
  (func $hook (param i32) (result i64)
    (local $i i32)
    (loop $l
      (call $g (i32.const 1) (i32.const 10))
      drop
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br_if $l (i32.lt_u (local.get $i) (i32.const 10))))
    (call $accept (i32.const 0) (i32.const 0) (i64.const 0)))
  (export "hook" (func $hook))
  (data (i32.const 0) "0123456789012345678901234567890123456789012345678901234567890123456789"))
"#;

fn cleaned(src: &str) -> Vec<u8> {
    let raw = wat::parse_str(src).expect("fixture is valid wat");
    clean(&raw, &Options::default()).expect("clean succeeds")
}

#[test]
fn native_checker_accepts_valid_guarded_hook() {
    let w = cleaned(VALID_GUARDED_HOOK);
    assert!(w.len() >= 63, "fixture must clear the checker's size floor");
    let verdict = validate_guards_native(&w).expect("checker should accept a valid guarded hook");
    // Guard prologue `i32.const 1; i32.const 10; call $_g` means the checker's
    // worst-case count must be nonzero and finite.
    assert!(
        verdict.hook_cost > 0,
        "hook cost should be nonzero: {verdict:?}"
    );
    assert_eq!(
        verdict.cbak_cost, 0,
        "no cbak export exists in this fixture"
    );
}

#[test]
fn native_checker_rejects_unguarded_loop() {
    // An unguarded loop alongside a correctly guarded one (so `_g` survives
    // cleaning) isolates "unguarded loop" rejection from "no `_g` import".
    let src = r#"
    (module
      (import "env" "_g" (func $g (param i32 i32) (result i32)))
      (import "env" "accept" (func $accept (param i32 i32 i64) (result i64)))
      (memory 1)
      (func $hook (param i32) (result i64)
        (local $i i32)
        (loop $guarded
          (call $g (i32.const 1) (i32.const 10))
          drop
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br_if $guarded (i32.lt_u (local.get $i) (i32.const 10))))
        (local.set $i (i32.const 0))
        (loop $unguarded
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br_if $unguarded (i32.lt_u (local.get $i) (i32.const 1000000))))
        (call $accept (i32.const 0) (i32.const 0) (i64.const 0)))
      (export "hook" (func $hook))
      (data (i32.const 0) "0123456789012345678901234567890123456789012345678901234567890123456789"))
    "#;
    let w = cleaned(src);
    match validate_guards_native(&w) {
        Err(NativeGuardError::Invalid { log, .. }) => {
            assert!(
                log.contains("GuardCheck") || log.contains("Missing"),
                "expected a guard-shape rejection, got: {log}"
            );
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn native_checker_rejects_non_whitelisted_import() {
    let src = r#"
    (module
      (import "env" "_g" (func $g (param i32 i32) (result i32)))
      (import "env" "not_a_real_hook_api_fn" (func $bogus (param i32 i32 i64) (result i64)))
      (memory 1)
      (func $hook (param i32) (result i64)
        (call $bogus (i32.const 0) (i32.const 0) (i64.const 0)))
      (export "hook" (func $hook))
      (data (i32.const 0) "0123456789012345678901234567890123456789012345678901234567890123456789"))
    "#;
    let w = cleaned(src);
    match validate_guards_native(&w) {
        Err(NativeGuardError::Invalid { log, .. }) => {
            assert!(
                log.contains("not_a_real_hook_api_fn"),
                "expected the log to name the offending import: {log}"
            );
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// `validateGuards` is documented upstream as "may throw overflow_error,
/// length_error" on malformed LEB128 input. This exercises the shim's
/// exception path (status 2), not the ordinary invalid path (status 1): a
/// type-section length field with enough continuation bytes to overflow the
/// 64-bit accumulator in `parseLeb128`.
#[test]
fn native_checker_reports_exception_not_ub_on_malformed_leb128() {
    let mut w: Vec<u8> = vec![0x00, 0x61, 0x73, 0x6D, 0x01, 0x00, 0x00, 0x00]; // magic + version
    w.push(1); // type section id
    w.extend(std::iter::repeat_n(0xFFu8, 12)); // continuation bytes, all payload bits set
    w.push(0x7F); // terminate the LEB128 sequence
    while w.len() < 70 {
        w.push(0); // clear the checker's 63-byte minimum
    }

    match validate_guards_native(&w) {
        Err(NativeGuardError::Exception { log, .. }) => {
            assert!(
                log.to_lowercase().contains("overflow"),
                "expected an overflow_error message, got: {log}"
            );
        }
        other => panic!("expected Exception, got {other:?}"),
    }
}
