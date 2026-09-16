//! A bare `field: sfXxx = [ .. ]` default desugars to `fixed_vl`
//! regardless of `sfXxx`'s actual serialized type, so an array default on
//! a non-VL field (here `sfEmailHash`, `STI_UINT128`) is rejected by
//! `fixed_vl`'s own STI-agreement check, not by a message that blames a
//! kind the author never wrote (`hash128`).

use rshooks::prelude::*;
use rshooks::txn_template;

txn_template! {
    struct BadEmailHash {
        transaction_type = ttPAYMENT,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        email_hash: sfEmailHash = [0; 16],
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        emit_details: emit_details,
    }
}

fn main() {}
