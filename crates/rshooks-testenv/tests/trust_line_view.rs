//! Off-chain host coverage for `TrustLineView` (`rshooks::trust_line`): a
//! real `RippleState` ledger object is fabricated via
//! `TestEnv::ledger_object` and read back through a `#[hooks]` chain that
//! calls the typed API exactly as a hook would — no wasm build, no node.
//!
//! `crates/rshooks/src/trust_line.rs`'s own unit tests already cover the
//! per-account resolution logic (`side`/`limit_of`/flag reads) against a
//! hand-built `TrustLineView`, with no live host involved. What only a real
//! host can prove is the other half: that `TrustLineView::load` correctly
//! drives `keylet_line` + the typed slot layer against actual serialized
//! ledger-object bytes, that a genuinely missing line and a genuinely
//! malformed (present-but-corrupt) record surface distinct errors, and that
//! the two participants' independently set limits and flags land on the
//! right side regardless of which account the caller happens to pass first.
//!
//! The `RippleState` object's `LowLimit`/`HighLimit`/`Flags` bytes are
//! hand-assembled below using the same wire layout
//! `crates/rshooks-testenv/src/host/float.rs`'s `float_sto` writes (see
//! that module for the authoritative encoding); the keylet itself is an
//! independent `sha512Half(ledgerSpace ++ args)` recomputation, the same
//! technique `examples/13_keylets/tests/keylets.rs` uses.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use rshooks::prelude::*;
use rshooks::*;
use rshooks_core::ls_flags::{lsfHighDeepFreeze, lsfLowAuth, lsfLowFreeze};
use rshooks_testenv::prelude::*;
use sha2::{Digest, Sha512};

// ---------------------------------------------------------------------------
// Independent keylet recomputation (mirrors examples/13_keylets/tests)
// ---------------------------------------------------------------------------

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

fn line_keylet(a: [u8; 20], b: [u8; 20], currency: [u8; 20]) -> [u8; 34] {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let index = index_hash(b'r' as u16, &[&lo, &hi, &currency]);
    let mut out = [0u8; 34];
    out[0..2].copy_from_slice(&0x0072u16.to_be_bytes()); // ltRIPPLE_STATE
    out[2..34].copy_from_slice(&index);
    out
}

// ---------------------------------------------------------------------------
// RippleState object bytes: a bare canonical field sequence (root shape, no
// header/footer — same convention `slot_set` reads, matching
// `crates/rshooks-testenv/src/host/slots.rs`'s `account_root_bytes` test
// helper).
// ---------------------------------------------------------------------------

/// Wire bytes of a 48-byte IOU `Amount` value (no field header): the same
/// mantissa/sign/exponent layout `float_sto` writes. `currency`/`issuer`
/// content is irrelevant to `TrustLineView` (it only reads the 8-byte
/// magnitude via `as_xfl`), so both are filled with an arbitrary fixed
/// pattern.
fn iou_amount_bytes(mantissa: u64, exponent: i32) -> [u8; 48] {
    const EXPONENT_BIAS: i32 = 97;
    let mut out = [0u8; 48];
    if mantissa != 0 {
        let exp_biased = (exponent + EXPONENT_BIAS) as u8;
        out[0] = 0b1100_0000 | (exp_biased >> 2); // positive, IOU
        out[1] = ((exp_biased & 0b11) << 6) | ((mantissa >> 48) & 0x3F) as u8;
        out[2] = ((mantissa >> 40) & 0xFF) as u8;
        out[3] = ((mantissa >> 32) & 0xFF) as u8;
        out[4] = ((mantissa >> 24) & 0xFF) as u8;
        out[5] = ((mantissa >> 16) & 0xFF) as u8;
        out[6] = ((mantissa >> 8) & 0xFF) as u8;
        out[7] = (mantissa & 0xFF) as u8;
    }
    out[8..28].copy_from_slice(&[0xCCu8; 20]);
    out[28..48].copy_from_slice(&[0xDDu8; 20]);
    out
}

/// `LOW_LIMIT` = 5, `HIGH_LIMIT` = 25 — distinct canonical XFL encodings
/// (16-significant-digit mantissa), so a swapped side reads back a visibly
/// wrong value rather than an accidentally-matching one.
fn low_limit_bytes() -> [u8; 48] {
    iou_amount_bytes(5_000_000_000_000_000, -15) // 5
}
fn high_limit_bytes() -> [u8; 48] {
    iou_amount_bytes(2_500_000_000_000_000, -14) // 25
}

/// `Flags`: only the low account froze, only the high account deep-froze,
/// only the low account authorized — one bit per category set on a
/// different side, so a direction bug (reading the other side's bit) is
/// visible in every category independently.
const LINE_FLAGS: u32 = lsfLowFreeze | lsfHighDeepFreeze | lsfLowAuth;

/// A well-formed `RippleState`: `Flags`, `LowLimit`, `HighLimit`.
fn ripple_state_bytes(flags: u32, low_limit: &[u8], high_limit: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x22); // sfFlags (type 2, field 2)
    out.extend_from_slice(&flags.to_be_bytes());
    out.push(0x66); // sfLowLimit (type 6, field 6)
    out.extend_from_slice(low_limit);
    out.push(0x67); // sfHighLimit (type 6, field 7)
    out.extend_from_slice(high_limit);
    out
}

const ACCOUNT_LOW: [u8; 20] = [0x11u8; 20];
const ACCOUNT_HIGH: [u8; 20] = [0x22u8; 20];
const STRANGER: [u8; 20] = [0x33u8; 20];
const TEST_CURRENCY: CurrencyCode = CurrencyCode([
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'U', b'S', b'D', 0, 0, 0, 0, 0,
]);
const TEST_CURRENCY_RAW: [u8; 20] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'U', b'S', b'D', 0, 0, 0, 0, 0,
];

// ---------------------------------------------------------------------------
// The hook chain under test
// ---------------------------------------------------------------------------

#[hooks]
pub struct TrustLineViewUser {
    /// `limit_of(owner)`'s raw XFL bits.
    #[state(key = b"lim_o")]
    limit_owner: State<i64>,
    /// `limit_of(destination)`'s raw XFL bits.
    #[state(key = b"lim_d")]
    limit_dest: State<i64>,
    /// Packed `is_frozen_by`/`is_deep_frozen_by`/`is_authorized_by(owner)`:
    /// bit 0 / 1 / 2.
    #[state(key = b"flg_o")]
    flags_owner: State<u32>,
    /// Same packing as `flags_owner`, for `destination`.
    #[state(key = b"flg_d")]
    flags_dest: State<u32>,
    /// `side(owner)`: 0 = Low, 1 = High.
    #[state(key = b"sid_o")]
    side_owner: State<u8>,
    /// `side(destination)`: 0 = Low, 1 = High.
    #[state(key = b"sid_d")]
    side_dest: State<u8>,
}

#[hooks]
impl TrustLineViewUser {
    /// Loads the trust line between the originating transaction's
    /// `Account` and `Destination`, then records every `TrustLineView`
    /// accessor's result in state — rolling back with the propagated
    /// `HookError`'s own raw code (`e.code()`) if `load` itself fails, so a
    /// test can assert on the exact underlying error rather than a
    /// hook-chosen stand-in.
    #[hook(0, on = [Invoke])]
    fn check(&self) -> HookResult {
        let Ok(owner) = otxn_field_typed(sfAccount) else {
            rollback!(b"trust_line_view: sfAccount missing", 1);
        };
        let Ok(dest) = otxn_field_typed(sfDestination) else {
            rollback!(b"trust_line_view: sfDestination missing", 2);
        };

        let view = match TrustLineView::load(&owner, &dest, &TEST_CURRENCY) {
            Ok(v) => v,
            Err(e) => rollback!(b"trust_line_view: load failed", e.code()),
        };

        let Ok(limit_owner) = view.limit_of(&owner) else {
            rollback!(b"trust_line_view: limit_of(owner) failed", 3);
        };
        let Ok(limit_dest) = view.limit_of(&dest) else {
            rollback!(b"trust_line_view: limit_of(dest) failed", 4);
        };
        let _ = self.limit_owner.set(&limit_owner.raw_bits());
        let _ = self.limit_dest.set(&limit_dest.raw_bits());

        let Ok(pack) = pack_flags(&view, &owner) else {
            rollback!(b"trust_line_view: flag read(owner) failed", 5);
        };
        let _ = self.flags_owner.set(&pack);
        let Ok(pack) = pack_flags(&view, &dest) else {
            rollback!(b"trust_line_view: flag read(dest) failed", 6);
        };
        let _ = self.flags_dest.set(&pack);

        let Ok(side_owner) = view.side(&owner) else {
            rollback!(b"trust_line_view: side(owner) failed", 7);
        };
        let Ok(side_dest) = view.side(&dest) else {
            rollback!(b"trust_line_view: side(dest) failed", 8);
        };
        let _ = self.side_owner.set(&side_code(side_owner));
        let _ = self.side_dest.set(&side_code(side_dest));

        accept!(b"trust_line_view: ok", 0)
    }

    /// Queries the line with an account that is not one of its two
    /// participants — expected to roll back with
    /// `HookError::DoesNotMatch`'s own code.
    #[hook(1, on = [Invoke])]
    fn check_non_participant(&self) -> HookResult {
        let Ok(owner) = otxn_field_typed(sfAccount) else {
            rollback!(b"trust_line_view: sfAccount missing", 1);
        };
        let Ok(dest) = otxn_field_typed(sfDestination) else {
            rollback!(b"trust_line_view: sfDestination missing", 2);
        };
        let view = match TrustLineView::load(&owner, &dest, &TEST_CURRENCY) {
            Ok(v) => v,
            Err(e) => rollback!(b"trust_line_view: load failed", e.code()),
        };
        let stranger = AccountId(STRANGER);
        match view.limit_of(&stranger) {
            Ok(_) => rollback!(b"trust_line_view: stranger unexpectedly accepted", 9),
            Err(e) => rollback!(b"trust_line_view: stranger rejected", e.code()),
        }
    }
}

/// Packs `is_frozen_by`/`is_deep_frozen_by`/`is_authorized_by(account)` into
/// bits 0/1/2 of one `u32`.
fn pack_flags(view: &TrustLineView, account: &AccountId) -> Result<u32> {
    let frozen = view.is_frozen_by(account)?;
    let deep_frozen = view.is_deep_frozen_by(account)?;
    let authorized = view.is_authorized_by(account)?;
    Ok(u32::from(frozen) | (u32::from(deep_frozen) << 1) | (u32::from(authorized) << 2))
}

fn side_code(side: TrustLineSide) -> u8 {
    match side {
        TrustLineSide::Low => 0,
        TrustLineSide::High => 1,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn seeded_env(owner: [u8; 20], dest: [u8; 20]) -> TestEnv {
    let kl = line_keylet(ACCOUNT_LOW, ACCOUNT_HIGH, TEST_CURRENCY_RAW);
    TestEnv::new()
        .otxn(Otxn::new(TxType::Invoke).account(owner).destination(dest))
        .ledger_object(
            kl,
            &ripple_state_bytes(LINE_FLAGS, &low_limit_bytes(), &high_limit_bytes()),
        )
}

#[test]
fn owner_low_destination_high_reads_the_correct_side() {
    let env = seeded_env(ACCOUNT_LOW, ACCOUNT_HIGH);
    let exit = env.invoke::<TrustLineViewUser>(0);
    assert_eq!(exit.exit, ExitType::Accept, "code {}", exit.code);

    assert_eq!(env.state_typed::<i64>(b"lim_o"), Some(XFL!(5).raw_bits()));
    assert_eq!(env.state_typed::<i64>(b"lim_d"), Some(XFL!(25).raw_bits()));
    // owner (low): frozen + authorized, not deep-frozen -> 0b101 = 5
    assert_eq!(env.state_typed::<u32>(b"flg_o"), Some(0b101));
    // dest (high): deep-frozen only -> 0b010 = 2
    assert_eq!(env.state_typed::<u32>(b"flg_d"), Some(0b010));
    assert_eq!(env.state_typed::<u8>(b"sid_o"), Some(0)); // Low
    assert_eq!(env.state_typed::<u8>(b"sid_d"), Some(1)); // High
}

/// Same seeded line, `Account`/`Destination` swapped — `TrustLineView`
/// resolves sides from the accounts themselves, not from argument order or
/// transaction role, so every recorded value must swap right along with
/// them (proving "no caller-selected Low/High field").
#[test]
fn owner_high_destination_low_still_reads_the_correct_side() {
    let env = seeded_env(ACCOUNT_HIGH, ACCOUNT_LOW);
    let exit = env.invoke::<TrustLineViewUser>(0);
    assert_eq!(exit.exit, ExitType::Accept, "code {}", exit.code);

    assert_eq!(env.state_typed::<i64>(b"lim_o"), Some(XFL!(25).raw_bits()));
    assert_eq!(env.state_typed::<i64>(b"lim_d"), Some(XFL!(5).raw_bits()));
    assert_eq!(env.state_typed::<u32>(b"flg_o"), Some(0b010));
    assert_eq!(env.state_typed::<u32>(b"flg_d"), Some(0b101));
    assert_eq!(env.state_typed::<u8>(b"sid_o"), Some(1)); // High
    assert_eq!(env.state_typed::<u8>(b"sid_d"), Some(0)); // Low
}

#[test]
fn missing_line_rolls_back_with_doesnt_exist() {
    // No `.ledger_object(..)` seeded for this keylet at all.
    let env = TestEnv::new().otxn(
        Otxn::new(TxType::Invoke)
            .account(ACCOUNT_LOW)
            .destination(ACCOUNT_HIGH),
    );
    let exit = env.invoke::<TrustLineViewUser>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, HookError::DoesntExist.code());
}

/// The line exists, but `HighLimit`'s header claims the 48-byte IOU shape
/// while only 5 bytes of value actually follow it — a truncated,
/// structurally corrupt object, not merely an absent field. Reading it
/// surfaces a parse failure (`HookError::NotAnObject`, from the field walk
/// that cannot skip past the truncated value), never `DoesntExist` — proving
/// a missing line and a malformed record are distinguishable outcomes, not
/// the same error in disguise.
#[test]
fn malformed_limit_field_rolls_back_with_a_different_error_than_a_missing_line() {
    let kl = line_keylet(ACCOUNT_LOW, ACCOUNT_HIGH, TEST_CURRENCY_RAW);
    let bad_high_limit = [0xC1u8, 0x00, 0x00, 0x00, 0x00]; // claims 48 bytes, has 5
    let env = TestEnv::new()
        .otxn(
            Otxn::new(TxType::Invoke)
                .account(ACCOUNT_LOW)
                .destination(ACCOUNT_HIGH),
        )
        .ledger_object(
            kl,
            &ripple_state_bytes(LINE_FLAGS, &low_limit_bytes(), &bad_high_limit),
        );
    let exit = env.invoke::<TrustLineViewUser>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_ne!(exit.code, HookError::DoesntExist.code());
    assert_eq!(exit.code, HookError::NotAnObject.code());
}

#[test]
fn an_account_that_is_neither_participant_is_rejected() {
    let env = seeded_env(ACCOUNT_LOW, ACCOUNT_HIGH);
    let exit = env.invoke::<TrustLineViewUser>(1);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, HookError::DoesNotMatch.code());
}
