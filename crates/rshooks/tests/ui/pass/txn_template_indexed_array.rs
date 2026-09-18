//! A homogeneous `array(sfX) [ Elem: object(sfY) { .. } ; N ]` field
//! compiles, and its runtime-indexed accessor is reachable and callable in
//! a loop over every valid index.

use rshooks::prelude::*;
use rshooks::txn_template;

txn_template! {
    struct Remit {
        transaction_type = ttREMIT,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        amounts: array(sfAmounts) [
            AmountEntry: object(sfAmountEntry) {
                amount: native_amount(sfAmount) = 1,
            }; 3
        ],
        emit_details: emit_details,
    }
}

fn fill(e: &mut AmountEntry<'_>) {
    e.set_amount(7).expect("7 drops is in range");
}

fn main() {
    let mut txn = Remit::new();
    for i in 0..3 {
        let mut entry = txn.amounts(i).expect("index in range");
        fill(&mut entry);
    }
    assert!(txn.amounts(3).is_none());
}
