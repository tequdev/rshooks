//! Off-chain unit tests for the `xfl-math` example, driven through
//! `TestEnv::invoke` against the real `XflMath` chain — no wasm build, no
//! node.
//!
//! `XflMath::main` reaches its `Amount` via `SlotObject::from_otxn()` (the
//! numbered-slot host API) and runs `mulratio`/`XFL::new`/comparisons
//! through `rshooks-testenv`'s real host implementation, including the
//! compounding step's [`rshooks::xfl_unchecked::XFLUnchecked`] operators —
//! so a reasonably sized amount reaches this hook's real `accept!()` path
//! end-to-end, verified below.
//!
//! `as_xfl()` on a native `Amount` yields **XAH units, not drops** — every
//! drops value below is chosen with that conversion (`XAH = drops /
//! 1_000_000`) in mind.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, missing_docs)]

use rshooks_testenv::prelude::*;
use xfl_math::{XflMath, XflMathError};

fn env_with_drops(drops: u64) -> TestEnv {
    TestEnv::new().hook_account([1u8; 20]).otxn(
        Otxn::new(TxType::Payment)
            .account([2u8; 20])
            .destination([1u8; 20])
            .amount_drops(drops),
    )
}

#[test]
fn accepts_for_a_reasonably_sized_amount() {
    // 1000 XAH: 1% share = 10, well above the 1e-6 minimum, so the hook
    // runs all the way through `mulratio`/comparisons/`remaining` and the
    // `XFLUnchecked`-based compounding step, reaching its real `accept!()`
    // path: compounding a 10 XAH share by 1% three times (~10.303) stays
    // far below the ~990 XAH `remaining`, so neither `CompoundNotIncreasing`
    // nor `CompoundExceedsRemaining` fires.
    let exit = env_with_drops(1_000_000_000).invoke::<XflMath>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    assert!(exit.is_success());
}

#[test]
fn accept_leaves_no_trace_side_effects() {
    let e = env_with_drops(1_000_000_000);
    let exit = e.invoke::<XflMath>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    assert!(e.traces().is_empty());
}

#[test]
fn rolls_back_when_the_computed_share_is_below_the_minimum() {
    // 1 drop = 0.000001 XAH; 1% of that (1e-8) is below the hook's
    // hardcoded 1e-6 minimum share.
    let exit = env_with_drops(1).invoke::<XflMath>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, XflMathError::BelowMinimum.code());
    assert_eq!(exit.msg, b"xfl-math: computed share below minimum");
}

#[test]
fn rolls_back_when_the_otxn_has_no_amount_field() {
    let e = TestEnv::new().hook_account([1u8; 20]).otxn(
        Otxn::new(TxType::Payment)
            .account([2u8; 20])
            .destination([1u8; 20]),
    );
    let exit = e.invoke::<XflMath>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, XflMathError::NoAmountField.code());
    assert_eq!(exit.msg, b"xfl-math: no Amount field on otxn");
}

#[test]
fn behavior_is_deterministic_given_the_same_seeded_amount() {
    let a = env_with_drops(1_000_000_000).invoke::<XflMath>(0);
    let b = env_with_drops(1_000_000_000).invoke::<XflMath>(0);
    assert_eq!(a.exit, b.exit);
    assert_eq!(a.code, b.code);
}
