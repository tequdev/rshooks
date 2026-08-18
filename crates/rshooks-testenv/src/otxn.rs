//! [`Otxn`]: builds the originating transaction a [`crate::TestEnv`] seeds
//! its invocations with.

use std::collections::HashMap;
use std::vec::Vec;

use rshooks::tx_type::TxType;
use rshooks::txn::codec::encode_native_amount_const;

/// The originating transaction a [`crate::TestEnv`] seeds its invocations
/// with — backs `otxn_field`/`otxn_type`/`otxn_id`/`otxn_param`. Every field
/// is stored as its **raw value bytes** (what a real `otxn_field` call
/// would write into a caller buffer — no STObject header, no VL length
/// prefix), keyed by the field's `sfXxx` code.
#[derive(Debug, Clone)]
pub struct Otxn {
    pub(crate) tx_type: TxType,
    pub(crate) fields: HashMap<u32, Vec<u8>>,
    pub(crate) params: HashMap<Vec<u8>, Vec<u8>>,
    pub(crate) id: [u8; 32],
}

impl Otxn {
    /// Starts a new originating transaction of type `tx_type`. Every field
    /// is absent (`otxn_field` returns `DOESNT_EXIST`) until set.
    #[must_use]
    pub fn new(tx_type: TxType) -> Self {
        Self {
            tx_type,
            fields: HashMap::new(),
            params: HashMap::new(),
            id: [0u8; 32],
        }
    }

    /// Sets `sfAccount` (the transaction's sender).
    #[must_use]
    pub fn account(mut self, acc: [u8; 20]) -> Self {
        self.fields
            .insert(rshooks::sfield::sfAccount.code(), acc.to_vec());
        self
    }

    /// Sets `sfDestination`.
    #[must_use]
    pub fn destination(mut self, acc: [u8; 20]) -> Self {
        self.fields
            .insert(rshooks::sfield::sfDestination.code(), acc.to_vec());
        self
    }

    /// Sets `sfAmount` to a native (XRP/XAH) amount of `drops`.
    ///
    /// # Panics
    ///
    /// Panics if `drops >=`[`rshooks::txn::codec::MAX_NATIVE_DROPS`] (via
    /// [`encode_native_amount_const`]) — a test-author bug to fix, not
    /// something worth threading a `Result` through every chainable `Otxn`
    /// method for.
    #[must_use]
    pub fn amount_drops(mut self, drops: u64) -> Self {
        self.fields.insert(
            rshooks::sfield::sfAmount.code(),
            encode_native_amount_const(drops).to_vec(),
        );
        self
    }

    /// Escape hatch: sets an arbitrary field's raw value bytes directly, by
    /// its `sfXxx` code (`rshooks::raw::sfcodes::sfXxx`, or
    /// `rshooks::sfield::sfXxx.code()`).
    #[must_use]
    pub fn field_raw(mut self, sfield: u32, bytes: &[u8]) -> Self {
        self.fields.insert(sfield, bytes.to_vec());
        self
    }

    /// Sets a Hook API parameter attached to this originating transaction
    /// (read back via `otxn_param`).
    #[must_use]
    pub fn param(mut self, name: &[u8], value: &[u8]) -> Self {
        self.params.insert(name.to_vec(), value.to_vec());
        self
    }

    /// Sets this transaction's ID (hash), returned by `otxn_id`.
    #[must_use]
    pub fn id(mut self, hash: [u8; 32]) -> Self {
        self.id = hash;
        self
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    use super::*;

    #[test]
    fn account_sets_raw_field_bytes() {
        let otxn = Otxn::new(TxType::Payment).account([7u8; 20]);
        assert_eq!(
            otxn.fields.get(&rshooks::sfield::sfAccount.code()),
            Some(&[7u8; 20].to_vec())
        );
    }

    #[test]
    fn amount_drops_sets_native_amount_encoding() {
        let otxn = Otxn::new(TxType::Payment).amount_drops(1);
        let bytes = otxn.fields.get(&rshooks::sfield::sfAmount.code()).unwrap();
        assert_eq!(bytes, &vec![0x40, 0, 0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn field_raw_is_a_general_escape_hatch() {
        let otxn =
            Otxn::new(TxType::Payment).field_raw(rshooks::sfield::sfSequence.code(), &[0, 0, 0, 5]);
        assert_eq!(
            otxn.fields.get(&rshooks::sfield::sfSequence.code()),
            Some(&vec![0, 0, 0, 5])
        );
    }

    #[test]
    fn param_and_id_are_stored() {
        let otxn = Otxn::new(TxType::Payment).param(b"K", b"V").id([9u8; 32]);
        assert_eq!(otxn.params.get(b"K".as_slice()), Some(&b"V".to_vec()));
        assert_eq!(otxn.id, [9u8; 32]);
    }

    #[test]
    #[should_panic(expected = "native_amount default does not fit in 62 bits")]
    fn amount_drops_panics_at_or_above_max_native_drops() {
        let _ = Otxn::new(TxType::Payment).amount_drops(1u64 << 62);
    }
}
