//! Generates `crates/rshooks-core/src/tx_flags.rs` from `tx_flags.h`'s parsed
//! [`ConstGroup`]s (`crates/xtask/src/ir.rs`, `hook_api.json`).

use anyhow::Result;

use super::{push_const, with_generated_marker};
use crate::ir::ConstGroup;
use crate::render::{is_bare_identifier, render_flag_value};

const MODULE_DOC: &str = "\
//! Transaction flags (`tfXxx`) and account flags (`asfXxx`).
//!
//! Upstream: `Xahau/xahaud`, branch `release`, `hook/tx_flags.h`, vendored at
//! `crates/rshooks-core/vendor/xahaud-hook/tx_flags.h`.
//!
//! The header groups these into several C enums, one per transaction type
//! (or `AccountFlags` for the `asfXxx` `AccountSet` sub-flags). This
//! translation flattens every enum into a single list of `pub const`s, in
//! header order; no name collides across enums. A few `MPTokenIssuanceCreateFlags`
//! members alias `ls_flags.h` values in the C header (`tfMPTCanLock = lsfMPTCanLock`,
//! etc.) — those are kept as references to the [`crate::ls_flags`] consts
//! rather than re-typed literals, so the two stay in sync by construction.
";

/// `enum` group header comments are `// enum NAME`, except two groups whose
/// comments carry extra explanatory suffixes: `AccountFlags` (the `asfXxx`
/// `AccountSet` sub-flags aren't obviously flags from the name alone) and
/// `MPTokenIssuanceCreateFlags` (whose members alias `ls_flags.h` constants).
fn group_comment(enum_name: &str) -> String {
    match enum_name {
        "AccountFlags" => {
            "// enum AccountFlags (asfXxx: AccountSet SetFlag/ClearFlag sub-flags)".to_string()
        }
        "MPTokenIssuanceCreateFlags" => {
            "// enum MPTokenIssuanceCreateFlags (aliases of ls_flags.h values in the C header)"
                .to_string()
        }
        _ => format!("// enum {enum_name}"),
    }
}

/// Renders `tx_flags.rs`'s full contents from `tx_flags.h`'s parsed
/// [`ConstGroup`]s.
pub fn generate(groups: &[ConstGroup]) -> Result<String> {
    let mut body = String::from("\nuse crate::ls_flags;\n");
    for group in groups {
        body.push('\n');
        body.push_str(&group_comment(&group.name));
        body.push('\n');
        for member in &group.members {
            let value = render_flag_value(&member.value, "ls_flags")?;
            let doc = if is_bare_identifier(&member.value) {
                format!(
                    "C: `{}` (tx_flags.h, `enum {}`) = `{}`",
                    member.name, group.name, member.value
                )
            } else {
                format!("C: `{}` (tx_flags.h, `enum {}`)", member.name, group.name)
            };
            push_const(&mut body, &[doc], &member.name, "u32", &value);
        }
    }
    Ok(with_generated_marker("tx_flags.h", MODULE_DOC) + &body)
}
