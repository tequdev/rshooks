//! Generates `crates/rshooks-core/src/error.rs` from `error.h`'s parsed
//! [`ConstSpec`]s (`crates/xtask/src/ir.rs`, `hook_api.json`).

use anyhow::Result;

use super::const_table;
use crate::ir::ConstSpec;
use crate::render::expect_decimal;

const MODULE_DOC: &str = "\
//! Hook API error codes.
//!
//! Upstream: `Xahau/xahaud`, branch `release`, `hook/error.h`, vendored at
//! `crates/rshooks-core/vendor/xahaud-hook/error.h`.
//!
//! Every Hook API function returns an `i64`; non-negative values are
//! success payloads (often \"bytes written\"), negative values are one of
//! the error codes below. Kept verbatim (name and value) from the C header.
";

/// `INVALID_FLOAT` has the exceptional value `-10024`.
fn doc_lines(name: &str) -> Vec<String> {
    if name == "INVALID_FLOAT" {
        vec![
            "C: `INVALID_FLOAT` (error.h) — note the upstream value is `-10024`, not".to_string(),
            "`-24` (kept verbatim; this is not a typo in this translation).".to_string(),
        ]
    } else {
        vec![format!("C: `{name}` (error.h)")]
    }
}

/// Renders `error.rs`'s full contents from `error.h`'s parsed [`ConstSpec`]s.
pub fn generate(error_codes: &[ConstSpec]) -> Result<String> {
    const_table(
        "error.h",
        MODULE_DOC,
        "i64",
        error_codes,
        expect_decimal,
        doc_lines,
    )
}
