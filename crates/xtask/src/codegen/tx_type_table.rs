//! Generates `crates/rshooks-build/src/tx_type_table.rs` from `tts.h`.
//!
//! `rshooks-build`'s `#[hooks]` metadata pipeline validates entries'
//! `on`/`can_emit` transaction-type lists and encodes the HookOn /
//! HookCanEmit bitmasks against a name-to-code table. That table used to be
//! hand-written, independent of [`super::tx_type`]'s typed `TxType` enum, so
//! an upstream `tts.h` change could make a type usable in the typed API
//! while the build side silently rejected it or produced a wrong mask. This
//! generator renders the build-side table from the same [`ConstSpec`]s and
//! the same [`super::tx_type::variant_name`] spelling, so the two can never
//! drift apart.

use std::fmt::Write as _;

use anyhow::{Context, Result};

use super::tx_type::variant_name;
use super::with_generated_marker;
use crate::ir::ConstSpec;
use crate::render::expect_decimal;

const MODULE_DOC: &str = "\
//! Build-side transaction-type name/code table.\n\
//!\n\
//! Consumed by `rshooks_build::metadata` to validate `#[hooks]` entries'\n\
//! `on`/`can_emit` transaction-type lists and to encode the HookOn /\n\
//! HookCanEmit bitmasks — the build-side counterpart of\n\
//! `rshooks::tx_type::TxType`, generated from the same `tts.h` constants so\n\
//! the two can never drift apart.\n";

/// Renders `tx_type_table.rs`'s full contents from `tts.h`'s parsed
/// [`ConstSpec`]s.
pub fn generate(tts: &[ConstSpec]) -> Result<String> {
    let mut names = String::new();
    let mut codes = Vec::with_capacity(tts.len());

    for d in tts {
        let value = expect_decimal(&d.name, &d.c_expr)?;
        let variant = variant_name(&d.name)?;
        writeln!(names, "    \"{variant}\",").context("writing transaction type name")?;
        codes.push(value);
    }
    let codes = codes.join(", ");

    let mut body = String::from("\n");
    body.push_str(
        "/// Canonical Xahau JSON spellings for every known `TxType` variant\n\
         /// (excluding the data-carrying `Unknown`), used to validate\n\
         /// `#[hooks]` entries' `on`/`can_emit` transaction-type lists.\n\
         /// Position-for-position with [`TRANSACTION_TYPE_CODES`], and using\n\
         /// the exact same spellings as `rshooks::tx_type::TxType`.\n\
         pub(crate) const TRANSACTION_TYPES: &[&str] = &[\n",
    );
    body.push_str(&names);
    body.push_str("];\n\n");
    body.push_str(
        "/// `tt*` codes corresponding position-for-position to\n\
         /// [`TRANSACTION_TYPES`].\n\
         pub(crate) const TRANSACTION_TYPE_CODES: &[u8] = &[",
    );
    body.push_str(&codes);
    body.push_str("];\n");

    Ok(with_generated_marker("tts.h", MODULE_DOC) + &body)
}
