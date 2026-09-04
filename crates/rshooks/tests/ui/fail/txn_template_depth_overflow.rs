//! Nesting depth is bounded at compile time by `STO_WRITER_MAX_DEPTH` (10),
//! the same limit `StoWriter::begin_object`/`begin_array` enforce at
//! runtime. Ten `object(sfAmountEntry) { .. }` levels nested inside one
//! another (reusing the one STObject sfield at every level -- legal, since
//! canonical order is checked per container, not across containers) push
//! the innermost field one level past that bound.

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
        level1: object(sfAmountEntry) {
            level2: object(sfAmountEntry) {
                level3: object(sfAmountEntry) {
                    level4: object(sfAmountEntry) {
                        level5: object(sfAmountEntry) {
                            level6: object(sfAmountEntry) {
                                level7: object(sfAmountEntry) {
                                    level8: object(sfAmountEntry) {
                                        level9: object(sfAmountEntry) {
                                            level10: object(sfAmountEntry) {
                                                flags: u32_field(sfFlags) = 0,
                                            },
                                        },
                                    },
                                },
                            },
                        },
                    },
                },
            },
        },
        emit_details: emit_details,
    }
}

fn main() {}
