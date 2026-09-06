//! Generates `crates/rshooks-core/src/sfcodes.rs` from `sfcodes.h`'s parsed
//! [`ConstSpec`]s (`crates/xtask/src/ir.rs`, `hook_api.json`).

use anyhow::Result;

use super::const_table;
use crate::ir::ConstSpec;
use crate::render::render_shift_add;

const MODULE_DOC: &str = "\
//! Serialized field (`sfXxx`) codes.
//!
//! Upstream: `Xahau/xahaud`, branch `release`, `hook/sfcodes.h`, vendored at
//! `crates/rshooks-core/vendor/xahaud-hook/sfcodes.h`.
//!
//! Each code packs a type code and a field index: `(type << 16) + index`,
//! mirrored verbatim from the header.
";

/// Renders `sfcodes.rs`'s full contents from `sfcodes.h`'s parsed
/// [`ConstSpec`]s.
pub fn generate(sfcodes: &[ConstSpec]) -> Result<String> {
    const_table(
        "sfcodes.h",
        MODULE_DOC,
        "u32",
        sfcodes,
        |_, value| render_shift_add(value),
        |name| vec![format!("C: `{name}` (sfcodes.h)")],
    )
}
