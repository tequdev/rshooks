//! Drift tripwire for the vendored Hook API headers (`docs/DESIGN.md` §4),
//! identical in spirit to
//! `crates/rshooks-build/tests/guard_native.rs::vendored_files_match_recorded_sha256`.
//!
//! Test code is exempt from the workspace's panic-freedom lints (per
//! `docs/DESIGN.md` §8): `unwrap`/`expect` on a known-good fixture is the
//! normal, idiomatic way to assert behavior in a test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

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

/// Drift tripwire against one group's `SHA256SUMS` (the single source of
/// truth for that group's vendored hashes, regenerated only by
/// `scripts/sync-vendor.sh`): an accidental local edit to a vendored,
/// supposedly byte-identical upstream file (or a corrupted re-download)
/// fails a test loudly, instead of silently diverging from what a real
/// xahaud node runs — and, transitively, from what the parity tests in this
/// same directory assume they're checking. The recorded entry count is
/// asserted too, so a *removed* file is caught alongside an edited one.
fn assert_vendor_group(dir: &str, expected_entries: usize) {
    let sums_path = format!("{dir}/SHA256SUMS");
    let sums =
        std::fs::read_to_string(&sums_path).unwrap_or_else(|e| panic!("reading {sums_path}: {e}"));
    let mut checked = 0;
    for line in sums.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (want, name) = line
            .split_once("  ")
            .unwrap_or_else(|| panic!("malformed SHA256SUMS line: {line:?}"));
        let path = format!("{dir}/{name}");
        let got = sha256_hex(&path);
        assert_eq!(
            got, want,
            "{path} sha256 mismatch — the vendored file has drifted from \
             {sums_path}; never hand-edit vendored files, re-sync with \
             scripts/sync-vendor.sh (see VENDOR.md)"
        );
        checked += 1;
    }
    assert_eq!(
        checked, expected_entries,
        "expected exactly {expected_entries} entries in {sums_path}"
    );
}

#[test]
fn vendored_files_match_recorded_sha256() {
    assert_vendor_group("vendor/xahaud-hook", 8);
}

/// The same tripwire for the second vendor group, the protocol format
/// definitions `protocol_formats.json` is generated from.
#[test]
fn vendored_protocol_files_match_recorded_sha256() {
    assert_vendor_group("vendor/xahaud-protocol", 7);
}
