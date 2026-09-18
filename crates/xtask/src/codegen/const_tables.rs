//! Generates `crates/rshooks-core/src/{tts,sfcodes,error,ls_flags}.rs` from
//! their respective headers' parsed [`ConstSpec`]/[`ConstGroup`]s
//! (`crates/xtask/src/ir.rs`). One file: each of the four outputs is a
//! module doc plus one const-rendering call, nothing else.

use anyhow::Result;

use super::{const_table, push_const, with_generated_marker};
use crate::ir::{ConstGroup, ConstSpec};
use crate::render::{expect_decimal, render_literal, render_shift_add};

const TTS_MODULE_DOC: &str = "\
//! Transaction type (`ttXXX`) codes.
//!
//! Upstream: `Xahau/xahaud`, branch `release`, `hook/tts.h`, vendored at
//! `crates/rshooks-core/vendor/xahaud-hook/tts.h`.
";

/// Renders `tts.rs`'s full contents from `tts.h`'s parsed [`ConstSpec`]s.
pub fn tts(tts: &[ConstSpec]) -> Result<String> {
    const_table(
        "tts.h",
        TTS_MODULE_DOC,
        "u16",
        tts,
        expect_decimal,
        |name| vec![format!("C: `{name}` (tts.h)")],
    )
}

const SFCODES_MODULE_DOC: &str = "\
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
pub fn sfcodes(sfcodes: &[ConstSpec]) -> Result<String> {
    const_table(
        "sfcodes.h",
        SFCODES_MODULE_DOC,
        "u32",
        sfcodes,
        |_, value| render_shift_add(value),
        |name| vec![format!("C: `{name}` (sfcodes.h)")],
    )
}

const ERROR_MODULE_DOC: &str = "\
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
fn error_doc_lines(name: &str) -> Vec<String> {
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
pub fn error(error_codes: &[ConstSpec]) -> Result<String> {
    const_table(
        "error.h",
        ERROR_MODULE_DOC,
        "i64",
        error_codes,
        expect_decimal,
        error_doc_lines,
    )
}

const LS_FLAGS_MODULE_DOC: &str = "\
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
pub fn ls_flags(groups: &[ConstGroup]) -> Result<String> {
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
    Ok(with_generated_marker("ls_flags.h", LS_FLAGS_MODULE_DOC) + &body)
}
