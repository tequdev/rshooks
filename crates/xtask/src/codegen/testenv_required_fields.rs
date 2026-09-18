//! Generates `crates/rshooks-testenv/src/protocol_formats_generated.rs`
//! from the transaction formats in `protocol_formats.json`
//! (`crates/xtask/src/protocol_ir.rs`), which come from the vendored
//! `transactions.macro`/`TxFormats.cpp`.
//!
//! `rshooks-testenv`'s off-chain emit-blob validation
//! (`crate::protocol_formats::required_top_level_field_codes`) needs, for
//! every known `TransactionType` value, the wire codes of the fields its
//! format marks `presence: required` (`tx_common`'s plus its own). This
//! renders that table once, at build time, from the same [`ProtocolFormats`]
//! artifact `rshooks-core`/`rshooks` are generated from, instead of
//! `rshooks-testenv` parsing `protocol_formats.json` itself at test-run
//! time.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use anyhow::{Context, Result, anyhow};

use super::with_generated_marker_in;
use crate::protocol_ir::{FieldSpec, Presence, ProtocolFormats};

const MODULE_DOC: &str = "\
//! Required top-level field codes per transaction format.
//!
//! Derived from `protocol_formats.json`'s `presence: required` fields;
//! consumed by `crate::protocol_formats`'s off-chain emit-blob validation.
";

/// The wire codes of `fields`' `presence: required` entries, resolved
/// through `sfield_codes`.
fn required_codes(fields: &[FieldSpec], sfield_codes: &BTreeMap<&str, u32>) -> Result<Vec<u32>> {
    fields
        .iter()
        .filter(|f| f.presence == Presence::Required)
        .map(|f| {
            sfield_codes
                .get(f.sfield.as_str())
                .copied()
                .ok_or_else(|| anyhow!("`{}` has no `sfields` entry", f.sfield))
        })
        .collect()
}

/// Renders `protocol_formats_generated.rs`'s full contents from the parsed
/// [`ProtocolFormats`].
pub fn generate(formats: &ProtocolFormats) -> Result<String> {
    let sfield_codes: BTreeMap<&str, u32> = formats
        .sfields
        .iter()
        .map(|s| (s.name.as_str(), s.code))
        .collect();

    let tx_common_required = required_codes(&formats.tx_common, &sfield_codes)?;

    let mut entries: Vec<(u16, Vec<u32>)> = Vec::with_capacity(formats.transactions.len());
    for tx in &formats.transactions {
        entries.push((tx.value, required_codes(&tx.fields, &sfield_codes)?));
    }
    entries.sort_by_key(|(value, _)| *value);

    let mut body = String::from("\n");
    body.push_str(
        "/// `tx_common`'s required field codes.\n\
         pub(crate) const TX_COMMON_REQUIRED: &[u64] = &[",
    );
    for code in &tx_common_required {
        write!(body, "{code}, ").context("writing a tx_common field code")?;
    }
    body.push_str("];\n\n");

    body.push_str(
        "/// Each known `TransactionType` value's own required field codes,\n\
         /// sorted by value.\n\
         pub(crate) const TX_REQUIRED: &[(u16, &[u64])] = &[\n",
    );
    for (value, codes) in &entries {
        write!(body, "    ({value}, &[").context("writing a transaction type entry")?;
        for code in codes {
            write!(body, "{code}, ").context("writing a required field code")?;
        }
        body.push_str("]),\n");
    }
    body.push_str("];\n");

    Ok(with_generated_marker_in("xahaud-protocol", "transactions.macro", MODULE_DOC) + &body)
}
