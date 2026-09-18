//! Two inline `optional object(sfX) { .. }` containers at the top level,
//! each a 2-byte header + one required `hash256` field (1-byte header +
//! 32-byte value) + a 1-byte `0xE1` end marker = 36 bytes, together
//! needing 72 NOPs if both are absent at once -- exceeding the
//! 63-per-container budget xahaud enforces. Pins that the whole slot span
//! a named `optional object`/`optional array` container closes with is
//! charged to its *parent's* budget, not just whatever `optional`/`vl`/
//! `any_amount` fields happen to live directly inside it (neither
//! container has any of those, so each one's own internal budget is 0).

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
        first_container: optional object(sfSignerEntry) {
            hash: hash256(sfInvoiceID),
        },
        second_container: optional object(sfHookGrant) {
            hash: hash256(sfInvoiceID),
        },
        emit_details: emit_details,
    }
}

fn main() {}
