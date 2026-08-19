//! Source-scan tests (design §2.1), two of them:
//!
//! 1. [`inventory_matches_a_fresh_grep_of_the_bridged_families`] asserts
//!    set-equality between `crates/rshooks/testenv-call-sites.txt` (bridged +
//!    unbridged sections together) and a fresh grep of every direct
//!    `rshooks_core::<fn>(` call site in the bridged families under
//!    `crates/rshooks/src/api/*.rs`, plus `crates/rshooks/src/xfl.rs`/
//!    `xfl_unchecked.rs` and `api/keylet.rs`'s `util_keylet_buf(` calls (see
//!    [`find_raw_call_in_keylet`]'s doc comment and the inventory file's own
//!    header for why those two need different matching than the rest). This
//!    only keeps the committed *inventory* honest against a fresh grep — it
//!    does not by itself prove any call site is actually intercepted under
//!    `testenv`; a raw call and its own bridging `#[cfg(feature =
//!    "testenv", ...)]` guard could both vanish together (e.g. an entire
//!    intercepted block deleted) without this test noticing, since the grep
//!    and the inventory would still agree.
//! 2. [`every_raw_call_site_has_an_enclosing_testenv_guard`] is the test that
//!    actually catches that case: for every raw call site the grep in (1)
//!    finds, it requires the literal text `feature = "testenv"` to appear
//!    somewhere in the enclosing `fn`'s body (a brace-depth walk from the
//!    preceding `fn ` line — see [`fn_body_end_line`]'s doc comment for the
//!    heuristic's limits). Deleting an entire interception block (the
//!    `#[cfg(...)] if let Some(..) = ... { return ...; }` guard plus its raw
//!    fallback call) makes this test fail even though (1) would stay green,
//!    because the surviving raw call's enclosing fn no longer contains the
//!    marker text at all.

#![allow(
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    missing_docs
)]

use std::collections::BTreeSet;
use std::path::Path;

/// One inventory/grep row: `(file, wrapper_fn, raw_fn)`.
type Row = (String, String, String);

const BRIDGED_FAMILY_FILES: &[&str] = &[
    "state.rs",
    "otxn.rs",
    "hook_ctx.rs",
    "ledger.rs",
    "control.rs",
    "etxn.rs",
    "trace.rs",
    "float.rs",
    "slot.rs",
    "sto.rs",
    "util.rs",
    "keylet.rs",
];

fn rshooks_crate_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../rshooks")
}

fn parse_inventory(path: &Path) -> BTreeSet<Row> {
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let mut set = BTreeSet::new();
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Strip a trailing `# comment` (used on unbridged-section rows).
        let line = match line.split_once('#') {
            Some((code, _)) => code.trim(),
            None => line,
        };
        let Some((lhs, rhs)) = line.split_once("->") else {
            panic!("unexpected inventory line shape: {raw_line:?}");
        };
        let Some((file_part, wrapper)) = lhs.trim().split_once("::") else {
            panic!("unexpected inventory lhs shape: {lhs:?}");
        };
        set.insert((
            file_part.trim().to_string(),
            wrapper.trim().to_string(),
            rhs.trim().to_string(),
        ));
    }
    set
}

/// Finds the identifier right after a `fn ` keyword occurrence starting at
/// byte offset `idx` in `line` (`idx` already known to point at `"fn "`).
fn identifier_after(line: &str, idx: usize) -> Option<String> {
    let rest = line.get(idx + 3..)?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() { None } else { Some(name) }
}

/// Whether `word` occurs in `haystack` as a standalone token (not as a
/// substring of a longer identifier) — distinguishes a real `#[cfg(test)]`/
/// `#[cfg(all(test, ...))]` test-module gate from `#[cfg(feature =
/// "testenv", ...)]`, whose `testenv` would otherwise falsely match a naive
/// `contains("test")` check.
fn contains_word(haystack: &str, word: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut start = 0usize;
    while let Some(rel) = haystack.get(start..).and_then(|s| s.find(word)) {
        let match_start = start + rel;
        let match_end = match_start + word.len();
        let before_ok = match_start == 0
            || bytes
                .get(match_start - 1)
                .is_some_and(|b| !(*b as char).is_alphanumeric() && *b != b'_');
        let after_ok = match_end >= bytes.len()
            || bytes
                .get(match_end)
                .is_some_and(|b| !(*b as char).is_alphanumeric() && *b != b'_');
        if before_ok && after_ok {
            return true;
        }
        start = match_start + 1;
    }
    false
}

fn find_fn_name(line: &str) -> Option<String> {
    let idx = line.find("fn ")?;
    if idx > 0 {
        let prev = line.as_bytes().get(idx - 1).copied()?;
        if (prev as char).is_alphanumeric() || prev == b'_' {
            return None;
        }
    }
    identifier_after(line, idx)
}

fn find_raw_call(line: &str) -> Option<String> {
    const NEEDLE: &str = "rshooks_core::";
    let idx = line.find(NEEDLE)?;
    let rest = line.get(idx + NEEDLE.len()..)?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() || name == "backend" {
        return None;
    }
    let after = rest.get(name.len()..)?;
    if after.starts_with('(') {
        Some(name)
    } else {
        None
    }
}

/// `api/keylet.rs`'s 26 typed helpers each intercept the backend with their
/// own real slices *before* falling through to `util_keylet_buf` (a
/// crate-local composing call, not a bare `rshooks_core::<fn>(` one — see
/// `crates/rshooks/testenv-call-sites.txt`'s header and
/// [`rshooks_core::backend::KeyletArg`]'s doc comment for the full
/// rationale). [`find_raw_call`]'s `"rshooks_core::"` needle never matches
/// those calls, so this file's own raw-call marker is
/// `util_keylet_buf(` instead.
fn find_raw_call_in_keylet(line: &str) -> Option<String> {
    const NEEDLE: &str = "util_keylet_buf(";
    if line.contains(NEEDLE) {
        Some("util_keylet_buf".to_string())
    } else {
        None
    }
}

fn grep_raw_call_sites(api_dir: &Path) -> BTreeSet<Row> {
    let mut set = BTreeSet::new();
    for file_name in BRIDGED_FAMILY_FILES {
        let path = api_dir.join(file_name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let mut current_fn: Option<String> = None;
        for line in content.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("#[cfg(") && contains_word(trimmed, "test") {
                break; // stop at the first test module in this file
            }
            if let Some(name) = find_fn_name(trimmed) {
                current_fn = Some(name);
            }
            let raw = if *file_name == "keylet.rs" {
                find_raw_call_in_keylet(line)
            } else {
                find_raw_call(line)
            };
            if let Some(raw) = raw {
                if let Some(f) = &current_fn {
                    set.insert((format!("api/{file_name}"), f.clone(), raw));
                }
            }
        }
    }
    // `xfl.rs`/`xfl_unchecked.rs` (crate root, not under `api/`) each have
    // their own raw call sites — see the inventory file's own trailing
    // sections for why both are scanned as honorary members of the "float"
    // bridged family.
    for file_name in ["xfl.rs", "xfl_unchecked.rs"] {
        let path = api_dir.parent().unwrap_or(api_dir).join(file_name);
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let mut current_fn: Option<String> = None;
        for line in content.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("#[cfg(") && contains_word(trimmed, "test") {
                break;
            }
            if let Some(name) = find_fn_name(trimmed) {
                current_fn = Some(name);
            }
            if let Some(raw) = find_raw_call(line) {
                if let Some(f) = &current_fn {
                    set.insert((file_name.to_string(), f.clone(), raw));
                }
            }
        }
    }
    set
}

#[test]
fn inventory_matches_a_fresh_grep_of_the_bridged_families() {
    let crate_dir = rshooks_crate_dir();
    let inventory = parse_inventory(&crate_dir.join("testenv-call-sites.txt"));
    let grep = grep_raw_call_sites(&crate_dir.join("src/api"));

    let missing_from_inventory: Vec<_> = grep.difference(&inventory).collect();
    let stale_in_inventory: Vec<_> = inventory.difference(&grep).collect();

    assert!(
        missing_from_inventory.is_empty() && stale_in_inventory.is_empty(),
        "testenv-call-sites.txt is out of sync with crates/rshooks/src/api/*.rs:\n\
         call sites present in source but missing from the inventory: {missing_from_inventory:#?}\n\
         inventory rows with no matching call site in source: {stale_in_inventory:#?}"
    );
}

/// Brace-depth walk from `start_line` (already known to contain a `fn `
/// declaration, per [`find_fn_name`]) to the line where that function's body
/// closes. A single lightweight heuristic — raw `{`/`}` counting across
/// every char of each line, no string/comment awareness — deliberately not
/// a real parser: good enough to bound "the text of this one function" for
/// [`find_unbridged_call_sites`]'s marker search, not to be relied on for
/// anything more precise. Stops at the first line where the depth returns to
/// (or below) zero after having gone positive at least once; falls back to
/// the file's last line if the brace is never closed (defensive — every
/// real Rust `fn` closes; this only guards against a heuristic miscount).
fn fn_body_end_line(lines: &[&str], start_line: usize) -> usize {
    let mut depth: i64 = 0;
    let mut opened = false;
    for (offset, line) in lines.get(start_line..).unwrap_or(&[]).iter().enumerate() {
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if opened && depth <= 0 {
            return start_line + offset;
        }
    }
    lines.len().saturating_sub(1)
}

/// One raw call site whose enclosing `fn` body carries no `feature =
/// "testenv"` cfg marker anywhere in it — the actual interception-loss
/// signal [`every_raw_call_site_has_an_enclosing_testenv_guard`] checks for.
/// `(label, fn_name)`, `label` matching [`grep_raw_call_sites`]'s own row
/// shape (`"api/<file>.rs"` or the bare `xfl.rs`/`xfl_unchecked.rs` name).
fn find_unbridged_call_sites(api_dir: &Path) -> Vec<(String, String)> {
    const MARKER: &str = "feature = \"testenv\"";
    let mut offenders = Vec::new();

    let mut scan_file = |path: &Path, label: &str, is_keylet: bool| {
        let content = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let lines: Vec<&str> = content.lines().collect();
        let mut current_fn: Option<String> = None;
        let mut current_fn_start = 0usize;
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("#[cfg(") && contains_word(trimmed, "test") {
                break; // stop at the first test module in this file
            }
            if let Some(name) = find_fn_name(trimmed) {
                current_fn = Some(name);
                current_fn_start = i;
            }
            let raw = if is_keylet {
                find_raw_call_in_keylet(line)
            } else {
                find_raw_call(line)
            };
            if raw.is_some() {
                if let Some(f) = &current_fn {
                    let end = fn_body_end_line(&lines, current_fn_start);
                    let body_has_marker = lines
                        .get(current_fn_start..=end)
                        .is_some_and(|body| body.iter().any(|l| l.contains(MARKER)));
                    if !body_has_marker {
                        offenders.push((label.to_string(), f.clone()));
                    }
                }
            }
        }
    };

    for file_name in BRIDGED_FAMILY_FILES {
        scan_file(
            &api_dir.join(file_name),
            &format!("api/{file_name}"),
            *file_name == "keylet.rs",
        );
    }
    for file_name in ["xfl.rs", "xfl_unchecked.rs"] {
        scan_file(
            &api_dir.parent().unwrap_or(api_dir).join(file_name),
            file_name,
            false,
        );
    }

    offenders
}

#[test]
fn every_raw_call_site_has_an_enclosing_testenv_guard() {
    let crate_dir = rshooks_crate_dir();
    let offenders = find_unbridged_call_sites(&crate_dir.join("src/api"));
    assert!(
        offenders.is_empty(),
        "raw rshooks_core call site(s) whose enclosing fn has no `feature = \"testenv\"` cfg \
         guard anywhere in its body (an interception block may have been deleted): {offenders:#?}"
    );
}
