//! Generates the typed `rshooks::LedgerEntryType` model from the ledger
//! entry formats in `protocol_formats.json`.
//!
//! Ledger-entry counterpart of [`super::tx_type`] (typed mirrors are the
//! `rshooks` layer's job per `docs/DESIGN.md` §5). Unlike `tx_type`, no name
//! derivation is needed: `LEDGER_ENTRY(tag, value, name, rpcName, fields)`
//! states the canonical name outright, so variants are quoted from upstream
//! rather than reconstructed.

use std::fmt::Write as _;

use anyhow::{Context, Result};

use super::with_generated_marker_in;
use crate::protocol_ir::LedgerEntryFormat;

const MODULE_DOC: &str = "\
//! Ledger entry type (`LedgerEntryType`) model.
//!
//! [`LedgerEntryType`] is a typed, exhaustive-by-construction mirror of the
//! raw `lt*` ledger-entry-type codes in `rshooks_core::lets` (plus
//! [`LedgerEntryType::Unknown`] for forward-compatibility with codes this
//! crate does not yet know about) — exactly what [`crate::tx_type::TxType`]
//! is for the `tt*` codes, applied to the `u16` type channel of a ledger
//! object's `sfLedgerEntryType`.
";

/// The ledger entry whose code the generated doc example decodes. Chosen
/// for recognizability; its absence is a generation failure rather than a
/// silently broken doctest.
const DOC_EXAMPLE: &str = "AccountRoot";

/// Renders `ledger_entry_type.rs`'s full contents from the parsed ledger
/// entry formats.
pub fn generate(ledger_entries: &[LedgerEntryFormat]) -> Result<String> {
    let mut variants = String::new();
    let mut from_arms = String::new();
    let mut code_arms = String::new();
    let mut known_codes = Vec::with_capacity(ledger_entries.len());

    for le in ledger_entries {
        writeln!(
            variants,
            "    /// `{tag}` (0x{value:04x}) — RPC name `{rpc}`.",
            tag = le.tag,
            value = le.value,
            rpc = le.rpc_name,
        )
        .context("writing variant doc")?;
        writeln!(variants, "    {},", le.name).context("writing variant")?;
        writeln!(
            from_arms,
            "            rshooks_core::{} => LedgerEntryType::{},",
            le.tag, le.name
        )
        .context("writing From arm")?;
        writeln!(
            code_arms,
            "            LedgerEntryType::{} => rshooks_core::{},",
            le.name, le.tag
        )
        .context("writing code() arm")?;
        known_codes.push(format!("0x{:04x}", le.value));
    }
    let known_codes = known_codes.join(", ");

    let example = ledger_entries
        .iter()
        .find(|le| le.name == DOC_EXAMPLE)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no `{DOC_EXAMPLE}` ledger entry to build the doc example from; \
                 pick another in codegen::ledger_entry_type::DOC_EXAMPLE"
            )
        })?;

    let mut body = String::from("\n");
    write!(
        body,
        "/// The type of a ledger object, decoded from the raw `u16` `lt*` code\n\
         /// its `sfLedgerEntryType` field carries.\n\
         ///\n\
         /// # Examples\n\
         ///\n\
         /// ```\n\
         /// use rshooks::ledger_entry_type::LedgerEntryType;\n\
         ///\n\
         /// let ty = LedgerEntryType::from(0x{value:04x});\n\
         /// assert_eq!(ty, LedgerEntryType::{name});\n\
         /// assert_eq!(ty.code(), 0x{value:04x});\n\
         /// ```\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq)]\n\
         pub enum LedgerEntryType {{\n",
        value = example.value,
        name = example.name,
    )
    .context("writing enum header")?;
    body.push_str(&variants);
    body.push('\n');
    body.push_str(
        "    /// A code this version of rshooks does not recognize yet. Carries\n\
         /// the raw code for forward-compatibility.\n\
         Unknown(u16),\n\
         }\n\
         \n\
         impl From<u16> for LedgerEntryType {\n\
         fn from(code: u16) -> Self {\n\
         match code {\n",
    );
    body.push_str(&from_arms);
    body.push_str(
        "            other => LedgerEntryType::Unknown(other),\n\
         }\n\
         }\n\
         }\n\
         \n\
         impl LedgerEntryType {\n\
         /// The raw `u16` code this variant corresponds to. Exact inverse of\n\
         /// [`LedgerEntryType::from`]:\n\
         /// `LedgerEntryType::from(c).code() == c` for every code, known or\n\
         /// unknown.\n\
         #[inline(always)]\n\
         #[must_use]\n\
         pub const fn code(&self) -> u16 {\n\
         match *self {\n",
    );
    body.push_str(&code_arms);
    write!(
        body,
        "            LedgerEntryType::Unknown(code) => code,\n\
         }}\n\
         }}\n\
         }}\n\
         \n\
         #[cfg(test)]\n\
         mod tests {{\n\
         use super::*;\n\
         \n\
         #[test]\n\
         fn round_trips_known_codes() {{\n\
         const _: u16 = LedgerEntryType::Unknown(0).code();\n\
         let known: &[u16] = &[{known_codes}];\n\
         for &code in known {{\n\
         assert_eq!(\n\
         LedgerEntryType::from(code).code(),\n\
         code,\n\
         \"round-trip failed for {{code}}\"\n\
         );\n\
         }}\n\
         assert_eq!(known.len(), {count}, \"generated for {count} ledger entry types\");\n\
         }}\n\
         \n\
         #[test]\n\
         fn unknown_code_round_trips() {{\n\
         let ty = LedgerEntryType::from(9999);\n\
         assert_eq!(ty, LedgerEntryType::Unknown(9999));\n\
         assert_eq!(ty.code(), 9999);\n\
         }}\n\
         }}\n",
        count = ledger_entries.len(),
    )
    .context("writing generated tests")?;

    Ok(with_generated_marker_in("xahaud-protocol", "ledger_entries.macro", MODULE_DOC) + &body)
}
