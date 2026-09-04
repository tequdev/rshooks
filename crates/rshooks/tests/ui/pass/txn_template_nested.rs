//! A `Remit`-shaped template exercising nested `object`/`array` fields end
//! to end: it compiles, and its nested setter (named by its full
//! `_`-joined declaration path) is reachable and callable.

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
            entry: object(sfAmountEntry) {
                amount: native_amount(sfAmount) = 1,
            },
        ],
        emit_details: emit_details,
    }
}

fn main() {
    let mut txn = Remit::new();
    txn.set_amounts_entry_amount(5).expect("5 drops is in range");
    let _ = &txn;
}
