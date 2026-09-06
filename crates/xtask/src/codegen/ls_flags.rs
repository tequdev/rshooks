//! Generates `crates/rshooks-core/src/ls_flags.rs` from `ls_flags.h`'s parsed
//! [`ConstGroup`]s (`crates/xtask/src/ir.rs`, `hook_api.json`).

use anyhow::Result;

use super::{push_const, with_generated_marker};
use crate::ir::ConstGroup;
use crate::render::render_literal;

const MODULE_DOC: &str = "\
//! Ledger entry flags (`lsfXxx`).
//!
//! Upstream: `Xahau/xahaud`, branch `release`, `hook/ls_flags.h`, vendored at
//! `crates/rshooks-core/vendor/xahaud-hook/ls_flags.h`.
//!
//! The header groups these into several unnamed-in-Rust C enums, one per
//! ledger entry type (`ltACCOUNT_ROOT`, `ltOFFER`, ...). This translation
//! flattens every enum into a single list of `pub const`s, in header order;
//! no name collides across enums (verified against the header), so every
//! name is kept verbatim with no disambiguation prefix.
";

/// Renders `ls_flags.rs`'s full contents from `ls_flags.h`'s parsed
/// [`ConstGroup`]s.
pub fn generate(groups: &[ConstGroup]) -> Result<String> {
    let mut body = String::new();
    for group in groups {
        body.push('\n');
        body.push_str("// enum ");
        body.push_str(&group.name);
        body.push('\n');
        for member in &group.members {
            let value = render_literal(&member.value)?;
            let doc = vec![format!(
                "C: `{}` (ls_flags.h, `enum {}`)",
                member.name, group.name
            )];
            push_const(&mut body, &doc, &member.name, "u32", &value);
        }
    }
    Ok(with_generated_marker("ls_flags.h", MODULE_DOC) + &body)
}
