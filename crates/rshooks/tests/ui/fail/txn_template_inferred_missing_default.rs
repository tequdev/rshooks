//! A bare `field: sfXxx` (no `= default`) whose `sfXxx` is an inferable
//! integer STI (here `sfFlags`, `STI_UINT32`) is rejected: an integer kind
//! needs a declared default, unlike a zeroed kind.

use rshooks::prelude::*;
use rshooks::txn_template;

txn_template! {
    struct MissingDefault {
        transaction_type = ttPAYMENT,
        flags: sfFlags,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        emit_details: emit_details,
    }
}

fn main() {}
