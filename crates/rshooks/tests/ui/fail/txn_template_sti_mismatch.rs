//! `u32_field` requires an STI_UINT32 `sfXxx` code. `sfFee` is STI_AMOUNT,
//! so declaring it as `u32_field` is rejected at compile time by the
//! per-field STI-agreement check, not silently accepted with the wrong
//! wire encoding.

use rshooks::prelude::*;
use rshooks::txn_template;

txn_template! {
    struct Payment {
        transaction_type = ttPAYMENT,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        fee: u32_field(sfFee) = 0, // WRONG: sfFee is STI_AMOUNT, not STI_UINT32
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        emit_details: emit_details,
    }
}

fn main() {}
