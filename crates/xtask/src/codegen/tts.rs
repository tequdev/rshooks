//! Generates `crates/rshooks-core/src/tts.rs` from `tts.h`'s parsed
//! [`ConstSpec`]s (`crates/xtask/src/ir.rs`, `hook_api.json`).

use anyhow::Result;

use super::const_table;
use crate::ir::ConstSpec;
use crate::render::expect_decimal;

const MODULE_DOC: &str = "\
//! Transaction type (`ttXXX`) codes.
//!
//! Upstream: `Xahau/xahaud`, branch `release`, `hook/tts.h`, vendored at
//! `crates/rshooks-core/vendor/xahaud-hook/tts.h`.
";

/// Renders `tts.rs`'s full contents from `tts.h`'s parsed [`ConstSpec`]s.
pub fn generate(tts: &[ConstSpec]) -> Result<String> {
    const_table("tts.h", MODULE_DOC, "u16", tts, expect_decimal, |name| {
        vec![format!("C: `{name}` (tts.h)")]
    })
}
