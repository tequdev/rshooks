//! Off-chain unit tests for the `xfl-math` example, driven through
//! `TestEnv::invoke` against the real `XflMath` chain — no wasm build, no
//! node.
//!
//! # What P2-D changed, and what it did not
//!
//! `XflMath::main` (`src/lib.rs`) reaches its `Amount` via
//! `SlotObject::from_otxn()`, i.e. the numbered-slot host API
//! (`otxn_slot`/`slot_subfield`/`slot_float`). That family landed in
//! `.claude/design/TESTENV_PHASE2_DESIGN.md`'s stage P2-D, so — unlike when
//! this file was first written in P2-B (float-only) and pinned a
//! deterministic `otxn_slot`-not-implemented rollback — the hook now
//! genuinely runs `mulratio`/`XFL::new`/comparisons through
//! `rshooks-testenv`'s real host implementation, all the way up through
//! computing `share`, checking it against the minimum, and computing
//! `remaining`.
//!
//! It does **not**, however, reach `accept!()` for any input, and this is
//! not a P2-D gap: `src/lib.rs`'s compounding step —
//! `share.unchecked() * growth.unchecked() * growth.unchecked() *
//! growth.unchecked()`, then `.validate()` — goes through
//! [`rshooks::xfl_unchecked::XFLUnchecked`]'s operators, and *every one of
//! those* (`Mul`/`Add`/`Sub`/`Neg`/`validate`) calls `rshooks_core::float_*`
//! **directly** (`unsafe { rshooks_core::float_multiply(..) }`, etc. —
//! confirmed by reading `crates/rshooks/src/xfl_unchecked.rs`), bypassing
//! `rshooks::api::float`'s `#[cfg(testenv)]` bridging block entirely. This
//! is exactly the pre-existing, already-documented "raw `rshooks_core` call
//! escape hatch" `rshooks-testenv`'s own `lib.rs` module doc comment calls
//! out ("a hook that calls `rshooks_core` directly keeps hitting the real
//! `NOT_IMPLEMENTED` host stubs under `testenv`") — it just wasn't
//! previously *reachable* in this example, because `otxn_slot` used to fail
//! first. Under `testenv`'s native (non-wasm) build, every raw
//! `rshooks_core::float_*` call returns `NOT_IMPLEMENTED` (`-14`)
//! regardless of its arguments, so `compounded_raw.validate()` always fails
//! and the hook always rolls back at `CompoundValidationFailed` — for
//! *every* amount that survives the earlier, properly-bridged checks. No
//! seeded input reaches `accept!()` through this harness; that remains
//! e2e-only territory for this specific hook (confirmed by manual tracing:
//! a throwaway diagnostic entry recorded `compounded_raw`'s raw bits as
//! `NOT_IMPLEMENTED`'s bit pattern immediately after the first `unchecked()`
//! multiply).
//!
//! `as_xfl()` on a native `Amount` yields **XAH units, not drops** (see
//! `~/.claude/skills/hook-api/references/slot.md`'s documented native/XFL
//! wire-format warning) — every drops value below is chosen with that
//! conversion (`XAH = drops / 1_000_000`) in mind.

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
fn rolls_back_at_compound_validation_for_a_reasonably_sized_amount() {
    // 1000 XAH: 1% share = 10, well above the 1e-6 minimum, so the hook
    // runs all the way through `mulratio`/comparisons/`remaining` — all
    // real, bridged host calls — before hitting the `XFLUnchecked`
    // escape hatch documented in this file's module doc comment.
    let exit = env_with_drops(1_000_000_000).invoke::<XflMath>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, XflMathError::CompoundValidationFailed.code());
    assert_eq!(exit.msg, b"xfl-math: compounded share failed to validate");
}

#[test]
fn rollback_leaves_no_trace_or_state_side_effects() {
    let e = env_with_drops(1_000_000_000);
    let _ = e.invoke::<XflMath>(0);
    assert!(e.traces().is_empty());
    assert_eq!(e.state_typed::<u64>(b"counter"), None);
}

#[test]
fn rolls_back_when_the_computed_share_is_below_the_minimum() {
    // 1 drop = 0.000001 XAH; 1% of that (1e-8) is below the hook's
    // hardcoded 1e-6 minimum share — reached and rejected *before* the
    // `XFLUnchecked` escape hatch, so this rollback reason is genuinely
    // exercised by the real `mulratio`/comparison host calls.
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
