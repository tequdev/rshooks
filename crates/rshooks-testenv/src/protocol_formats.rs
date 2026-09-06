//! Deterministic per-transaction-type required-field lookup for
//! [`crate::emit_walk::validate_emit_blob`], sourced from
//! [`protocol_formats_generated`] — a build-time table
//! `cargo xtask gen-core` derives from the vendored `protocol_formats.json`
//! artifact (`crates/rshooks-core/protocol_formats.json`, the same file
//! `xtask` parses when regenerating `rshooks-core`/`rshooks`'s generated
//! sources, kept in sync with the vendored xahaud transaction formats by
//! `crates/rshooks-core/tests/protocol_formats_parity.rs`).
//!
//! This is the one piece of xahaud's real `ripple::preflight` a
//! byte-level harness can reproduce deterministically: whether every field
//! a transaction format marks `presence: "required"` (`tx_common`'s common
//! fields plus the emitted type's own) is present in the blob. It is a
//! **subset** of preflight — field *values* (amount signs, flag
//! combinations, currency/issuer validity, and so on) are not checked; see
//! `crate::emit_walk::validate_emit_blob`'s own doc comment for the full
//! list of `HookAPI::emit` rules this harness does and does not reproduce.

use std::vec::Vec;

use crate::protocol_formats_generated::{TX_COMMON_REQUIRED, TX_REQUIRED};

/// Every top-level field code a blob whose `TransactionType` value is
/// `tx_type_value` must contain: `tx_common`'s required fields plus that
/// type's own. `None` if `tx_type_value` names no transaction format in
/// `protocol_formats.json` — [`crate::emit_walk::validate_emit_blob`] only
/// reaches this after rejecting unknown and pseudo transaction types, so in
/// practice this is always `Some` there.
pub(crate) fn required_top_level_field_codes(tx_type_value: u16) -> Option<Vec<u64>> {
    let specific = TX_REQUIRED
        .iter()
        .find(|(value, _)| *value == tx_type_value)?
        .1;
    let mut codes = TX_COMMON_REQUIRED.to_vec();
    codes.extend_from_slice(specific);
    Some(codes)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    use super::*;

    #[test]
    fn payment_requires_destination_and_amount_plus_the_common_fields() {
        let codes = required_top_level_field_codes(0).unwrap(); // ttPAYMENT
        let sf_destination = (8u64 << 16) | 3; // sfDestination: STI_ACCOUNT(8), field 3
        let sf_amount = (6u64 << 16) | 1; // sfAmount: STI_AMOUNT(6), field 1
        let sf_transaction_type = (1u64 << 16) | 2; // sfTransactionType (tx_common)
        assert!(codes.contains(&sf_destination));
        assert!(codes.contains(&sf_amount));
        assert!(codes.contains(&sf_transaction_type));
    }

    #[test]
    fn unknown_transaction_type_value_is_none() {
        assert!(required_top_level_field_codes(u16::MAX).is_none());
    }
}
