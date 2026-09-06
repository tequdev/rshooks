//! Generates `crates/rshooks-core/src/consts.rs` from `hookapi.h`'s and
//! `macro.h`'s parsed [`ConstSpec`] families (`crates/xtask/src/ir.rs`,
//! `hook_api.json`).

use anyhow::Result;

use super::{push_const, with_generated_marker};
use crate::ir::ConstSpec;
use crate::render::{expect_decimal, render_literal};

const MODULE_DOC: &str = "\
//! Keylet, compare, and field-offset constants.
//!
//! Upstream: `Xahau/xahaud`, branch `release`, `hook/hookapi.h` (`KEYLET_*`,
//! `COMPARE_*`) and `hook/macro.h` (`tfCANONICAL`, the `atACCOUNT` family,
//! the `amAMOUNT` family), vendored at
//! `crates/rshooks-core/vendor/xahaud-hook/hookapi.h` and
//! `crates/rshooks-core/vendor/xahaud-hook/macro.h`. Function-like macros from
//! `macro.h` (`SBUF`, `BUFFER_EQUAL`, `GUARD`, ...) are C conveniences, not
//! constants, and are NOT ported here — rshooks covers their roles with
//! real functions/macros.
";

fn push_section(
    body: &mut String,
    comment: &str,
    doc_file: &str,
    defines: &[ConstSpec],
    value_of: impl Fn(&str, &str) -> Result<String>,
) -> Result<()> {
    body.push('\n');
    body.push_str(comment);
    body.push_str("\n\n");
    for d in defines {
        let value = value_of(&d.name, &d.value)?;
        let doc = vec![format!("C: `{}` ({doc_file})", d.name)];
        push_const(body, &doc, &d.name, "u32", &value);
    }
    Ok(())
}

/// Renders `consts.rs`'s full contents from `hookapi.h`'s and `macro.h`'s
/// parsed [`ConstSpec`] families.
#[allow(clippy::too_many_arguments)]
pub fn generate(
    keylet: &[ConstSpec],
    compare: &[ConstSpec],
    canonical: &[ConstSpec],
    at_family: &[ConstSpec],
    am_family: &[ConstSpec],
) -> Result<String> {
    let mut body = String::new();
    push_section(
        &mut body,
        "// --- hookapi.h: KEYLET_* ---",
        "hookapi.h",
        keylet,
        expect_decimal,
    )?;
    push_section(
        &mut body,
        "// --- hookapi.h: COMPARE_* ---",
        "hookapi.h",
        compare,
        expect_decimal,
    )?;
    push_section(
        &mut body,
        "// --- macro.h: tfCANONICAL ---",
        "macro.h",
        canonical,
        |_, v| render_literal(v),
    )?;
    push_section(
        &mut body,
        "// --- macro.h: atACCOUNT family (amount/account slot-field offsets) ---",
        "macro.h",
        at_family,
        expect_decimal,
    )?;
    push_section(
        &mut body,
        "// --- macro.h: amAMOUNT family ---",
        "macro.h",
        am_family,
        expect_decimal,
    )?;

    Ok(with_generated_marker(
        "hookapi.h, crates/rshooks-core/vendor/xahaud-hook/macro.h",
        MODULE_DOC,
    ) + &body)
}
