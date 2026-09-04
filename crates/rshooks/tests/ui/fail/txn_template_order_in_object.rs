//! Canonical `(type, field)` order is checked per container, not just at
//! the top level: `sfDestination` (8,3) declared before `sfAmount` (6,1)
//! inside a nested `object` violates that container's own strictly
//! increasing order, even though the top-level order is fine.

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
            destination: account_id(sfDestination),
            amount: native_amount(sfAmount) = 1,
        },
        emit_details: emit_details,
    }
}

fn main() {}
