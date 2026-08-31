//! Generates `crates/rshooks-core/src/lets.rs` from the ledger entry
//! formats in `protocol_formats.json` (`crates/xtask/src/protocol_ir.rs`),
//! which come from the vendored `ledger_entries.macro`.
//!
//! Raw-`u16` counterpart of [`super::tts`]: mirrors the `lt*` ledger entry
//! type codes, declared only inside `LEDGER_ENTRY` macro invocations
//! upstream rather than in a header of their own. [`super::ledger_entry_type`]
//! renders the typed mirror one layer up, as [`super::tx_type`] does for
//! `tts.rs`.

use anyhow::Result;

use super::{push_const, with_generated_marker_in};
use crate::protocol_ir::LedgerEntryFormat;

const MODULE_DOC: &str = "\
//! Ledger entry type (`ltXXX`) codes.
//!
//! The values [`sfLedgerEntryType`](crate::sfcodes::sfLedgerEntryType)
//! carries, and what
//! [`slot_type`](crate::api::slot_type)-style checks compare against.
//!
//! Upstream: `Xahau/xahaud`, branch `release`,
//! `include/xrpl/protocol/detail/ledger_entries.macro`, vendored at
//! `crates/rshooks-core/vendor/xahaud-protocol/ledger_entries.macro`.
//!
//! # Rendering
//!
//! Upstream writes most of these values as 4-digit hex and three of them
//! (`ltHOOK_DEFINITION`, `ltEMITTED_TXN`, `ltHOOK`) as character literals
//! (`'D'`, `'E'`, `'H'`). They are all one number on the wire, so they are
//! rendered uniformly as 4-digit hex here — `ltHOOK` is `0x0048`, the code
//! of `'H'`.
";

/// Renders `lets.rs`'s full contents from the parsed ledger entry formats.
pub fn generate(ledger_entries: &[LedgerEntryFormat]) -> Result<String> {
    let mut body = String::from("\n");
    for le in ledger_entries {
        let doc = vec![format!(
            "C: `{}` (ledger_entries.macro) — the `{}` ledger entry.",
            le.tag, le.name
        )];
        push_const(
            &mut body,
            &doc,
            &le.tag,
            "u16",
            &format!("0x{:04x}", le.value),
        );
    }
    Ok(with_generated_marker_in("xahaud-protocol", "ledger_entries.macro", MODULE_DOC) + &body)
}
