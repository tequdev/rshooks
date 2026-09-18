//! `emit_details` is a structural, top-level-only marker -- declaring it
//! inside a nested `object` is rejected as an unrecognized declaration
//! rather than silently treated as the template's real `EmitDetails`
//! region.

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
            bad: emit_details
        },
        emit_details: emit_details,
    }
}

fn main() {}
