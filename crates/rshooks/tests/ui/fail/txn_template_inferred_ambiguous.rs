//! A bare `field: sfXxx = default` whose `sfXxx` is an ambiguous STI
//! (`STI_AMOUNT`, here `sfFee`) is rejected: the kind cannot be inferred
//! from the serialized type alone, so the caller must spell out
//! `native_amount`/`amount` explicitly.

use rshooks::prelude::*;
use rshooks::txn_template;

txn_template! {
    struct BadFee {
        transaction_type = ttPAYMENT,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        fee: sfFee = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        emit_details: emit_details,
    }
}

fn main() {}
