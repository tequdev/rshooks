//! A bare `field: sfXxx = <named const>` whose `sfXxx` is `STI_VL` (here
//! `sfMemoType`) does not match any of the default-shape desugar arms
//! (`[]`/`[ .. ]`/`*b".."` are literal shapes; a named const is none of
//! them), so it falls through to the plain inferred-scalar-with-default
//! arm and is rejected the same way any other ambiguous STI is: `VL`
//! still needs the explicit `fixed_vl(sfMemoType, N)` form so the length
//! is spelled out, not recovered from a literal.

use rshooks::prelude::*;
use rshooks::txn_template;

const SOME_CONST: [u8; 4] = *b"note";

txn_template! {
    struct BadMemoType {
        transaction_type = ttPAYMENT,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        memo: sfMemoType = SOME_CONST,
        account: account_id(sfAccount),
        emit_details: emit_details,
    }
}

fn main() {}
