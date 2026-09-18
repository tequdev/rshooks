//! The six emit-plumbing fields are recognized only at the top level: an
//! `sfAccount` declared only inside a nested `object` must not satisfy the
//! top-level required-field presence check, so this template still fails
//! to compile with a "missing required `sfAccount` field" error.

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
        // sfAccount only declared inside a nested object below -- this
        // must NOT satisfy the top-level presence check.
        entry: object(sfAmountEntry) {
            account: account_id(sfAccount),
        },
        emit_details: emit_details,
    }
}

fn main() {}
