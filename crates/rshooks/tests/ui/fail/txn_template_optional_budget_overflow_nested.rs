//! Same overflow as the top-level case, but inside a nested `object`: the
//! 63-NOP budget is checked per container, not just at the top level.

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
        entry: object(sfAmountEntry) {
            ledger_hash: optional hash256(sfLedgerHash),
            parent_hash: optional hash256(sfParentHash),
        },
        emit_details: emit_details,
    }
}

fn main() {}
