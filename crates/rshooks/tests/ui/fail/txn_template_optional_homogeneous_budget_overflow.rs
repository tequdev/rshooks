//! A homogeneous array of `optional object` elements whose per-element
//! size times its count would need more than 63 NOPs if every element
//! were absent at once -- checked against the *array's own* budget (these
//! NOPs sit in the array's own `STArray` field loop, not any parent
//! object's), inline in this arm since there is no `@end_array` for it.

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
        grants: array(sfHookGrants) [
            Grant: optional object(sfHookGrant) {
                ledger_hash: hash256(sfLedgerHash),
            }; 3
        ],
        emit_details: emit_details,
    }
}

fn main() {}
