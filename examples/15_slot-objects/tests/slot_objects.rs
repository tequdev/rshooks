//! Off-chain unit tests for the `slot-objects` example, driven through
//! `TestEnv::invoke` against the real `SlotObjects` hook — no wasm build,
//! no node. Every check group `src/lib.rs` implements is driven through
//! its real entry, seeding the ledger objects/trust line it navigates.
//!
//! Keylets are computed independently here (not through `rshooks-testenv`'s
//! or `rshooks::api::keylet`'s helpers, which need a live backend — not yet
//! installed when this seed data is built, before `TestEnv::invoke` runs)
//! via `sha512Half(ledgerSpace ++ args)`,
//! cross-checked against `crates/rshooks-testenv/src/host/keylet.rs`'s own
//! vectors — same pattern `examples/13_keylets/tests/keylets.rs` uses.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use rshooks_testenv::prelude::*;
use sha2::{Digest, Sha512};
use slot_objects::SlotObjects;

// -- Independent keylet computation (see module doc comment) --

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

fn keylet(ty: u16, key: [u8; 32]) -> [u8; 34] {
    let mut out = [0u8; 34];
    out[0..2].copy_from_slice(&ty.to_be_bytes());
    out[2..34].copy_from_slice(&key);
    out
}

const LT_ACCOUNT_ROOT: u16 = 0x0061;
const LT_SIGNER_LIST: u16 = 0x0053;
const LT_RIPPLE_STATE: u16 = 0x0072;
const SPACE_ACCOUNT: u16 = b'a' as u16;
const SPACE_SIGNER_LIST: u16 = b'S' as u16;
const SPACE_TRUST_LINE: u16 = b'r' as u16;

fn keylet_account(account: [u8; 20]) -> [u8; 34] {
    keylet(LT_ACCOUNT_ROOT, index_hash(SPACE_ACCOUNT, &[&account]))
}

fn keylet_signers(account: [u8; 20]) -> [u8; 34] {
    keylet(
        LT_SIGNER_LIST,
        index_hash(SPACE_SIGNER_LIST, &[&account, &0u32.to_be_bytes()]),
    )
}

fn keylet_line(a: [u8; 20], b: [u8; 20], currency: [u8; 20]) -> [u8; 34] {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    keylet(
        LT_RIPPLE_STATE,
        index_hash(SPACE_TRUST_LINE, &[&lo, &hi, &currency]),
    )
}

// -- Wire-format builders --

fn native_amount(drops: u64) -> [u8; 8] {
    let mut amt = drops.to_be_bytes();
    amt[0] |= 0x40; // positive
    amt
}

/// A minimal IOU `Amount` (48 bytes, no header): sign/native flags, biased
/// exponent, and 54-bit mantissa laid out per the STO Amount format —
/// currency/issuer bytes are left zeroed since `SlotObject<Amount>` never
/// reads them.
fn iou_amount(mantissa: u64, exponent: i32, negative: bool) -> [u8; 48] {
    let mut out = [0u8; 48];
    let exp_biased = (exponent + 97) as u8;
    out[0] = (if negative { 0b1000_0000 } else { 0b1100_0000 }) | (exp_biased >> 2);
    out[1] = ((exp_biased & 0b11) << 6) | ((mantissa >> 48) & 0x3F) as u8;
    out[2] = ((mantissa >> 40) & 0xFF) as u8;
    out[3] = ((mantissa >> 32) & 0xFF) as u8;
    out[4] = ((mantissa >> 24) & 0xFF) as u8;
    out[5] = ((mantissa >> 16) & 0xFF) as u8;
    out[6] = ((mantissa >> 8) & 0xFF) as u8;
    out[7] = (mantissa & 0xFF) as u8;
    out
}

/// `sfSequence`(2,4) + `sfBalance`(6,2, native) + `sfAccount`(8,1) — a bare
/// field sequence (root shape, no header/footer), matching what
/// `slot_set` expects a seeded ledger object's bytes to look like.
fn account_root_bytes(seq: u32, drops: u64, account: [u8; 20]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x24);
    out.extend_from_slice(&seq.to_be_bytes());
    out.push(0x62);
    out.extend_from_slice(&native_amount(drops));
    out.push(0x81);
    out.push(20);
    out.extend_from_slice(&account);
    out
}

/// A `SignerList`-shaped object: `sfSignerEntries`(15,4), one element
/// holding only `sfAccount`(8,1) — deliberately **no** `sfBalance`, so
/// `check_midhop_loop`'s `[sfSignerEntries][0][sfBalance]` walk fails as
/// the hook expects.
fn signer_list_bytes(entry_account: [u8; 20]) -> Vec<u8> {
    let mut inner = vec![0x81, 20];
    inner.extend_from_slice(&entry_account);
    let mut element = vec![0xEA]; // (type 14, field 10) — a SignerEntry-shaped element header
    element.extend_from_slice(&inner);
    element.push(0xE1);
    let mut root = vec![0xF4]; // sfSignerEntries header
    root.extend_from_slice(&element);
    root.push(0xF1);
    root
}

/// A `RippleState`-shaped object: just `sfBalance`(6,2, IOU).
fn trust_line_bytes(iou: [u8; 48]) -> Vec<u8> {
    let mut out = vec![0x62];
    out.extend_from_slice(&iou);
    out
}

const SENDER: [u8; 20] = [5u8; 20];
const ISSUER: [u8; 20] = [6u8; 20];
/// The exponent/mantissa `check_iou_amount` (`src/lib.rs`) expects to
/// round-trip to `IOU_AMOUNT = 100` via `to_int(0, true)` (absolute value):
/// `10^15 * 10^-13 = 100`.
const IOU_MANTISSA: u64 = 1_000_000_000_000_000;
const IOU_EXPONENT: i32 = -13;

/// Seeds the sender's account root, SignerList, and (for the IOU group)
/// trust line, plus an otxn carrying `CHK` and — for `CHK_IOU` — `ISS`.
fn env(chk: u8, with_iss: bool) -> TestEnv {
    let mut otxn = Otxn::new(rshooks::tx_type::TxType::Invoke)
        .account(SENDER)
        .param(b"CHK", &[chk]);
    if with_iss {
        otxn = otxn.param(b"ISS", &ISSUER);
    }
    let mut e = TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(otxn)
        .ledger_object(
            keylet_account(SENDER),
            &account_root_bytes(9, 1_000_000, SENDER),
        )
        .ledger_object(keylet_signers(SENDER), &signer_list_bytes(SENDER));
    if with_iss {
        // Negative sign, checked via `to_int(0, true)`'s absolute value.
        let iou = iou_amount(IOU_MANTISSA, IOU_EXPONENT, true);
        e = e.ledger_object(
            keylet_line(SENDER, ISSUER, currency_usd()),
            &trust_line_bytes(iou),
        );
    }
    e
}

fn currency_usd() -> [u8; 20] {
    let mut currency = [0u8; 20];
    currency[12..15].copy_from_slice(b"USD");
    currency
}

// Bit/group constants, mirrored from `src/lib.rs` (not exported by the
// hook crate).
const BIT_ACCOUNT_WALK: i64 = 1;
const BIT_DROPS_ROUNDTRIP: i64 = 2;
const BIT_PARENT_CLEAR: i64 = 4;
const BIT_TAKE_LOOP: i64 = 8;
const BIT_MIDHOP_LOOP: i64 = 16;
const BIT_DEEP_LOOP: i64 = 32;
const BIT_TAKE_FAILURE: i64 = 64;
const BIT_CAST_CLEANUP: i64 = 128;
const BIT_ROOT_CAST: i64 = 256;
const BIT_U64_WIRE: i64 = 512;
const BIT_IOU_XFL: i64 = 1024;

const CHK_CHEAP: u8 = 0;
const CHK_DEEP: u8 = 1;
const CHK_TAKE_FAILURE: u8 = 2;
const CHK_CAST: u8 = 3;
const CHK_MIDHOP: u8 = 4;
const CHK_IOU: u8 = 5;

#[test]
fn cheap_group_earns_every_bit_it_declares() {
    let exit = env(CHK_CHEAP, false).invoke::<SlotObjects>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    let expected =
        BIT_ACCOUNT_WALK | BIT_DROPS_ROUNDTRIP | BIT_U64_WIRE | BIT_PARENT_CLEAR | BIT_ROOT_CAST;
    assert_eq!(exit.code, expected);
}

#[test]
fn deep_group_earns_take_and_deep_loop_bits() {
    let exit = env(CHK_DEEP, false).invoke::<SlotObjects>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(exit.code, BIT_TAKE_LOOP | BIT_DEEP_LOOP);
}

#[test]
fn take_failure_group_earns_its_bit() {
    let exit = env(CHK_TAKE_FAILURE, false).invoke::<SlotObjects>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(exit.code, BIT_TAKE_FAILURE);
}

#[test]
fn cast_cleanup_group_earns_its_bit() {
    let exit = env(CHK_CAST, false).invoke::<SlotObjects>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(exit.code, BIT_CAST_CLEANUP);
}

#[test]
fn midhop_group_earns_its_bit() {
    let exit = env(CHK_MIDHOP, false).invoke::<SlotObjects>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(exit.code, BIT_MIDHOP_LOOP);
}

#[test]
fn iou_group_earns_its_bit() {
    let exit = env(CHK_IOU, true).invoke::<SlotObjects>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(exit.code, BIT_IOU_XFL);
}

#[test]
fn missing_account_root_rolls_back() {
    let e = TestEnv::new().hook_account([1u8; 20]).otxn(
        Otxn::new(rshooks::tx_type::TxType::Invoke)
            .account(SENDER)
            .param(b"CHK", &[CHK_CHEAP]),
    );
    let exit = e.invoke::<SlotObjects>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
}
