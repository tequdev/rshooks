//! Two `optional hash256` fields (33 bytes each -- 1-byte header + 32-byte
//! value) at the top level together need 66 NOPs if both are absent at
//! once, exceeding the 63-per-container budget xahaud enforces.

use rshooks::prelude::*;
use rshooks::txn_template;

txn_template! {
    struct Remit {
        transaction_type = ttREMIT,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        ledger_hash: optional hash256(sfLedgerHash),
        parent_hash: optional hash256(sfParentHash),
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        emit_details: emit_details,
    }
}

fn main() {}
