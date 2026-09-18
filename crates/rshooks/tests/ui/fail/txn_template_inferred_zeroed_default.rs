//! A bare `field: sfXxx = default` whose `sfXxx` is an inferable zeroed
//! (no-default) STI (here `sfInvoiceID`, `STI_UINT256`) is rejected: a
//! zeroed kind takes no `= default` at all.

use rshooks::prelude::*;
use rshooks::txn_template;

txn_template! {
    struct ZeroedDefault {
        transaction_type = ttPAYMENT,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        invoice: sfInvoiceID = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        emit_details: emit_details,
    }
}

fn main() {}
