//! Off-chain unit test for the `keylets` example, driven through
//! `TestEnv::invoke` against the real `Keylets` chain — no wasm build, no
//! node.
//!
//! The hook itself (`src/lib.rs`) does no independent verification of its
//! own — it calls each `keylet_xxx` typed helper and stores the raw
//! 34-byte result, rolling back with code `100 + KEYLET_*` only if a call
//! itself fails. So the primary fidelity assertion here is that the hook
//! completes with `accept!`/code `0` at all: every one of the 25 exercised
//! `keylet_xxx` calls succeeded through `rshooks-testenv`'s host
//! implementation with the exact argument shapes `rshooks::api::keylet`'s
//! typed wrappers construct.
//!
//! On top of that baseline, several of the 25 stored keylets are also
//! independently recomputed here — the same `sha512Half(ledgerSpace ++
//! args)` formula `e2e/test/keylets.test.ts` uses — and compared
//! byte-for-byte against what the hook actually wrote to state.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use keylets::Keylets;
use rshooks_testenv::prelude::*;
use sha2::{Digest, Sha512};

const OWNER: [u8; 20] = [0x30u8; 20];
const DEST: [u8; 20] = [0x40u8; 20];
const TEST_HASH: [u8; 32] = [0xABu8; 32];
const TEST_CURRENCY: [u8; 20] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'U', b'S', b'D', 0, 0, 0, 0, 0,
];
const OFFER_SEQ: u32 = 1;
const ESCROW_SEQ: u32 = 2;
const CHECK_SEQ: u32 = 3;
const PAYCHAN_SEQ: u32 = 5;
const CRON_START_TIME: u32 = 1_700_000_000;

/// The 32-byte-index-only half of `rshooks-testenv`'s own `index_hash`,
/// reimplemented independently here (not imported: that module is
/// crate-private) so this test is a genuine second computation, not a
/// tautology against the code under test.
fn index_hash(space: u16, parts: &[&[u8]]) -> [u8; 32] {
    let mut buf = Vec::new();
    buf.extend_from_slice(&space.to_be_bytes());
    for p in parts {
        buf.extend_from_slice(p);
    }
    let mut hasher = Sha512::new();
    hasher.update(&buf);
    let full = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&full[..32]);
    out
}

fn keylet(ty: u16, key: [u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(34);
    out.extend_from_slice(&ty.to_be_bytes());
    out.extend_from_slice(&key);
    out
}

fn env() -> TestEnv {
    TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Invoke).account(OWNER).destination(DEST))
}

/// `KeyletKey` variant discriminants (`src/lib.rs`'s `state_keys!` order,
/// 0-indexed) — matching `e2e/test/keylets.test.ts`'s own numbering.
mod disc {
    pub(super) const ACCOUNT: u8 = 2;
    pub(super) const AMENDMENTS: u8 = 3;
    pub(super) const FEES: u8 = 6;
    pub(super) const LINE: u8 = 8;
    pub(super) const OFFER: u8 = 9;
    pub(super) const TICKET: u8 = 12;
    pub(super) const SIGNERS: u8 = 13;
    pub(super) const CHECK: u8 = 14;
    pub(super) const DEPOSIT_PREAUTH: u8 = 15;
    pub(super) const UNCHECKED: u8 = 16;
    pub(super) const OWNER_DIR: u8 = 17;
    pub(super) const ESCROW: u8 = 19;
    pub(super) const PAYCHAN: u8 = 20;
    pub(super) const CRON: u8 = 25;
}

#[test]
fn accepts_and_computes_all_25_keylets() {
    let env = env();
    let exit = env.invoke::<Keylets>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(exit.code, 0);
    assert_eq!(exit.msg, b"keylets: ok");
}

#[test]
fn ticket_is_never_computed_or_stored() {
    let env = env();
    env.invoke::<Keylets>(0);
    assert_eq!(env.state(&[disc::TICKET]), None);
}

#[test]
fn account_matches_independent_recomputation() {
    let env = env();
    env.invoke::<Keylets>(0);
    let expected = keylet(0x0061, index_hash(b'a' as u16, &[&OWNER]));
    assert_eq!(env.state(&[disc::ACCOUNT]), Some(expected));
}

#[test]
fn signers_matches_independent_recomputation() {
    let env = env();
    env.invoke::<Keylets>(0);
    let expected = keylet(
        0x0053,
        index_hash(b'S' as u16, &[&OWNER, &0u32.to_be_bytes()]),
    );
    assert_eq!(env.state(&[disc::SIGNERS]), Some(expected));
}

#[test]
fn line_matches_independent_recomputation_with_canonical_account_order() {
    let env = env();
    env.invoke::<Keylets>(0);
    let (lo, hi) = if OWNER <= DEST {
        (OWNER, DEST)
    } else {
        (DEST, OWNER)
    };
    let expected = keylet(0x0072, index_hash(b'r' as u16, &[&lo, &hi, &TEST_CURRENCY]));
    assert_eq!(env.state(&[disc::LINE]), Some(expected));
}

#[test]
fn offer_escrow_check_match_independent_recomputation() {
    let env = env();
    env.invoke::<Keylets>(0);

    let offer = keylet(
        0x006f,
        index_hash(b'o' as u16, &[&OWNER, &OFFER_SEQ.to_be_bytes()]),
    );
    assert_eq!(env.state(&[disc::OFFER]), Some(offer));

    let escrow = keylet(
        0x0075,
        index_hash(b'u' as u16, &[&OWNER, &ESCROW_SEQ.to_be_bytes()]),
    );
    assert_eq!(env.state(&[disc::ESCROW]), Some(escrow));

    let check = keylet(
        0x0043,
        index_hash(b'C' as u16, &[&OWNER, &CHECK_SEQ.to_be_bytes()]),
    );
    assert_eq!(env.state(&[disc::CHECK]), Some(check));
}

#[test]
fn paychan_matches_independent_recomputation() {
    let env = env();
    env.invoke::<Keylets>(0);
    let expected = keylet(
        0x0078,
        index_hash(b'x' as u16, &[&OWNER, &DEST, &PAYCHAN_SEQ.to_be_bytes()]),
    );
    assert_eq!(env.state(&[disc::PAYCHAN]), Some(expected));
}

#[test]
fn deposit_preauth_matches_independent_recomputation() {
    let env = env();
    env.invoke::<Keylets>(0);
    let expected = keylet(0x0070, index_hash(b'p' as u16, &[&OWNER, &DEST]));
    assert_eq!(env.state(&[disc::DEPOSIT_PREAUTH]), Some(expected));
}

#[test]
fn owner_dir_type_code_differs_from_hash_space() {
    let env = env();
    env.invoke::<Keylets>(0);
    // ltDIR_NODE (0x0064), not the 'O' hash-space char itself.
    let expected = keylet(0x0064, index_hash(b'O' as u16, &[&OWNER]));
    assert_eq!(env.state(&[disc::OWNER_DIR]), Some(expected));
}

#[test]
fn amendments_and_fees_match_independent_recomputation() {
    let env = env();
    env.invoke::<Keylets>(0);
    let amendments = keylet(0x0066, index_hash(b'f' as u16, &[]));
    assert_eq!(env.state(&[disc::AMENDMENTS]), Some(amendments));
    // Fees' type code (0x0073) differs from its hash-space char ('e').
    let fees = keylet(0x0073, index_hash(b'e' as u16, &[]));
    assert_eq!(env.state(&[disc::FEES]), Some(fees));
}

#[test]
fn unchecked_is_the_raw_hash_verbatim() {
    let env = env();
    env.invoke::<Keylets>(0);
    let expected = keylet(0x0000, TEST_HASH);
    assert_eq!(env.state(&[disc::UNCHECKED]), Some(expected));
}

#[test]
fn cron_type_code_differs_from_hash_space_and_uses_the_bespoke_layout() {
    let env = env();
    env.invoke::<Keylets>(0);
    let got = env.state(&[disc::CRON]).unwrap();
    assert_eq!(&got[0..2], &[0x00, 0x41]); // ltCRON, not hash-space 'L'

    let ns = index_hash(b'L' as u16, &[]);
    assert_eq!(&got[2..10], &ns[0..8]);
    assert_eq!(&got[10..14], &CRON_START_TIME.to_be_bytes());
    let acc_hash = index_hash(b'L' as u16, &[&CRON_START_TIME.to_be_bytes(), &OWNER]);
    assert_eq!(&got[14..34], &acc_hash[0..20]);
}
