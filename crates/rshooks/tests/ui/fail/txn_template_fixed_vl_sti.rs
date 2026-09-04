//! `fixed_vl` requires an STI_VL `sfXxx` code -- `sfAmount` is STI_AMOUNT,
//! so declaring it as `fixed_vl` is rejected at compile time by the
//! per-field STI-agreement check.

use rshooks::prelude::*;
use rshooks::txn_template;

txn_template! {
    struct Payment {
        transaction_type = ttPAYMENT,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        amount: fixed_vl(sfAmount, 8), // WRONG: sfAmount is STI_AMOUNT, not STI_VL
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        emit_details: emit_details,
    }
}

fn main() {}
