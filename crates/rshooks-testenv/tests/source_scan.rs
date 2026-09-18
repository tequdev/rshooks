//! Source-scan test (design §2.1): for each bridged `rshooks::api::*` file,
//! counts raw `rshooks_core::<fn>(` call sites (source before the file's
//! own `#[cfg(test)]` module) against `feature = "testenv"` cfg-marker
//! occurrences, and requires at least as many markers as call sites. A
//! deleted interception block drops a marker without dropping its call
//! site, so the count goes negative and this test catches it.
//! `tests/spy_backend_audit.rs` separately proves the runtime property
//! (every backend method is actually reached).

#![allow(
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    missing_docs
)]

use std::path::Path;

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

/// Everything before this file's `#[cfg(test)]` module. Test code calls
/// `rshooks_core::` directly with no testenv guard, so it's excluded from
/// both counts rather than forcing a marker that would only ever guard a
/// test.
fn source_before_test_module(content: &str) -> &str {
    match content.find("\n#[cfg(test)]") {
        Some(i) => &content[..i],
        None => content,
    }
}

/// Whether `line` contains a raw `rshooks_core::<ident>(` call site — a
/// direct function call, not a module-qualified path
/// (`rshooks_core::backend::with_backend(`, whose next token after
/// `rshooks_core::` is `backend` followed by `::`, not `(`) or a doc/comment
/// reference (`` [`rshooks_core::DOESNT_EXIST`] ``, followed by `]`/`` ` ``,
/// not `(`).
fn find_raw_call(line: &str) -> bool {
    const NEEDLE: &str = "rshooks_core::";
    let Some(idx) = line.find(NEEDLE) else {
        return false;
    };
    let rest = &line[idx + NEEDLE.len()..];
    let name_len = rest
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(rest.len());
    name_len > 0 && rest[name_len..].starts_with('(')
}

/// `api/keylet.rs`'s 26 typed wrappers go through `util_keylet_buf(`/a bare
/// `util_keylet(` rather than `rshooks_core::<fn>(` directly (see the
/// module doc comment's "`_into` twins" section) — excluding `.util_keylet(`,
/// the one method-dispatch call inside `testenv_keylet` itself, not a raw
/// host call site.
fn find_raw_call_in_keylet(line: &str) -> bool {
    line.contains("util_keylet_buf(")
        || line
            .find("util_keylet(")
            .is_some_and(|idx| !line[..idx].ends_with('.'))
}

fn every_bridged_file_has_enough_markers() -> Vec<(String, usize, usize)> {
    const MARKER: &str = "feature = \"testenv\"";
    let api_dir = rshooks_crate_dir().join("src/api");
    let mut offenders = Vec::new();

    let mut check = |path: &Path, label: &str, is_keylet: bool| {
        let full = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let content = source_before_test_module(&full);
        let find = if is_keylet {
            find_raw_call_in_keylet
        } else {
            find_raw_call
        };
        let raw_count = content.lines().filter(|l| find(l)).count();
        let marker_count = content.matches(MARKER).count();
        if marker_count < raw_count {
            offenders.push((label.to_string(), raw_count, marker_count));
        }
    };

    for file_name in BRIDGED_FAMILY_FILES {
        check(
            &api_dir.join(file_name),
            &format!("api/{file_name}"),
            *file_name == "keylet.rs",
        );
    }
    for file_name in ["xfl.rs", "xfl_unchecked.rs"] {
        check(
            &api_dir.parent().unwrap_or(&api_dir).join(file_name),
            file_name,
            false,
        );
    }

    offenders
}

#[test]
fn every_raw_call_site_has_an_enclosing_testenv_guard() {
    let offenders = every_bridged_file_has_enough_markers();
    assert!(
        offenders.is_empty(),
        "file(s) with fewer `feature = \"testenv\"` markers than raw call sites, as \
         (label, raw_call_count, marker_count) — an interception block may have been \
         deleted: {offenders:#?}"
    );
}
