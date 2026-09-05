//! Deterministic per-transaction-type required-field lookup for
//! [`crate::emit_walk::validate_emit_blob`], sourced directly from the
//! vendored `protocol_formats.json` artifact (`crates/rshooks-core/protocol_formats.json`
//! — the same file `xtask` parses when regenerating `rshooks-core`/`rshooks`'s
//! generated sources, kept in sync with the vendored xahaud transaction
//! formats by `crates/rshooks-core/tests/protocol_formats_parity.rs`).
//!
//! This is the one piece of xahaud's real `ripple::preflight` a
//! byte-level harness can reproduce deterministically: whether every field
//! a transaction format marks `presence: "required"` (`tx_common`'s common
//! fields plus the emitted type's own) is present in the blob. It is a
//! **subset** of preflight — field *values* (amount signs, flag
//! combinations, currency/issuer validity, and so on) are not checked; see
//! `crate::emit_walk::validate_emit_blob`'s own doc comment for the full
//! list of `HookAPI::emit` rules this harness does and does not reproduce.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::vec::Vec;

const PROTOCOL_FORMATS_JSON: &str = include_str!("../../rshooks-core/protocol_formats.json");

/// Parsed once, on first use: `tx_common`'s required field codes, and each
/// known `TransactionType` value's own required field codes.
struct Formats {
    tx_common_required: Vec<u64>,
    tx_required: HashMap<u16, Vec<u64>>,
}

/// Parses [`PROTOCOL_FORMATS_JSON`] into a [`Formats`] table.
///
/// # Panics
///
/// Only if the vendored artifact's shape stops matching what this function
/// expects — an invariant `crates/rshooks-core/tests/protocol_formats_parity.rs`
/// guards at the source (`xtask`-generated) level, so this is a build-
/// config bug, never a runtime condition a hook author's `cargo test` run
/// can trigger.
#[allow(clippy::expect_used, clippy::panic)]
fn parse() -> Formats {
    /// `value.get(key)`, panicking with a message naming `key` if absent —
    /// `Value`'s own `Index` impl is what `clippy::indexing_slicing` flags,
    /// so every field access below goes through this instead.
    fn field<'a>(value: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
        value
            .get(key)
            .unwrap_or_else(|| panic!("protocol_formats.json: missing `{key}`"))
    }

    let root: serde_json::Value = serde_json::from_str(PROTOCOL_FORMATS_JSON)
        .expect("crates/rshooks-core/protocol_formats.json is valid JSON");

    let mut sfield_codes: HashMap<&str, u64> = HashMap::new();
    for sfield in field(&root, "sfields")
        .as_array()
        .expect("protocol_formats.json: `sfields` is an array")
    {
        let name = field(sfield, "name")
            .as_str()
            .expect("protocol_formats.json: sfield `name` is a string");
        let code = field(sfield, "code")
            .as_u64()
            .expect("protocol_formats.json: sfield `code` is a u64");
        sfield_codes.insert(name, code);
    }

    let required_codes = |fields: &serde_json::Value| -> Vec<u64> {
        fields
            .as_array()
            .expect("protocol_formats.json: a format's `fields` is an array")
            .iter()
            .filter(|f| field(f, "presence").as_str() == Some("required"))
            .map(|f| {
                let name = field(f, "sfield")
                    .as_str()
                    .expect("protocol_formats.json: field `sfield` is a string");
                *sfield_codes
                    .get(name)
                    .expect("protocol_formats.json: every `sfield` name has a `sfields` entry")
            })
            .collect()
    };

    let tx_common_required = required_codes(field(&root, "tx_common"));

    let mut tx_required = HashMap::new();
    for tx in field(&root, "transactions")
        .as_array()
        .expect("protocol_formats.json: `transactions` is an array")
    {
        let value = field(tx, "value")
            .as_u64()
            .expect("protocol_formats.json: transaction `value` is a u64");
        let value =
            u16::try_from(value).expect("protocol_formats.json: transaction `value` fits in u16");
        tx_required.insert(value, required_codes(field(tx, "fields")));
    }

    Formats {
        tx_common_required,
        tx_required,
    }
}

fn formats() -> &'static Formats {
    static FORMATS: OnceLock<Formats> = OnceLock::new();
    FORMATS.get_or_init(parse)
}

/// Every top-level field code a blob whose `TransactionType` value is
/// `tx_type_value` must contain: `tx_common`'s required fields plus that
/// type's own. `None` if `tx_type_value` names no transaction format in
/// `protocol_formats.json` — [`crate::emit_walk::validate_emit_blob`] only
/// reaches this after rejecting unknown and pseudo transaction types, so in
/// practice this is always `Some` there.
pub(crate) fn required_top_level_field_codes(tx_type_value: u16) -> Option<Vec<u64>> {
    let f = formats();
    let mut codes = f.tx_common_required.clone();
    codes.extend(f.tx_required.get(&tx_type_value)?.iter().copied());
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
