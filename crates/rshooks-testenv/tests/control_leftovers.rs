//! Integration tests for `hook_again`/`hook_skip`/`hook_param_set` semantics
//! (P2-E — `.claude/design/TESTENV_PHASE2_DESIGN.md` §4 "control
//! leftovers"). Hand-rolled `NativeEntry` tables (not `#[hooks]`-declared)
//! drive `rshooks::api::control`/`rshooks::api::hook_ctx` directly, matching
//! `crates/rshooks-testenv/src/env.rs`'s own in-crate test-module pattern
//! (`OneEntry`/`accepting_hook`, etc.) — this file exercises the same
//! machinery from outside the crate, through the public `TestEnv` API only.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    missing_docs
)]

use rshooks::decl::{HookChainEntries, NativeEntry};
use rshooks::error::HookError;
use rshooks_testenv::prelude::*;

const HASH_A: [u8; 32] = [0xA0; 32];
const HASH_B: [u8; 32] = [0xB0; 32];

// -- hook_again --

fn hook_again_once_then_accept(_r: u32) -> i64 {
    rshooks::api::control::hook_again().expect("first hook_again call should succeed");
    rshooks::api::control::accept(b"ok", 0);
}

fn hook_again_twice(_r: u32) -> i64 {
    let first = rshooks::api::control::hook_again();
    let second = rshooks::api::control::hook_again();
    if first.is_ok() && second == Err(HookError::AlreadySet) {
        rshooks::api::control::accept(b"ok", 0);
    }
    rshooks::api::control::rollback(b"unexpected hook_again semantics", 99);
}

fn hook_again_then_rollback(_r: u32) -> i64 {
    rshooks::api::control::hook_again().expect("hook_again call should succeed");
    rshooks::api::control::rollback(b"deliberate rollback", 1);
}

// -- hook_skip --

fn hook_skip_add(_r: u32) -> i64 {
    let r = rshooks::api::control::hook_skip(&HASH_A, 0);
    assert_eq!(r, Ok(1));
    rshooks::api::control::accept(b"ok", 0);
}

fn hook_skip_add_remove_remove_again(_r: u32) -> i64 {
    let add = rshooks::api::control::hook_skip(&HASH_A, 0);
    let remove = rshooks::api::control::hook_skip(&HASH_A, 1);
    let remove_again = rshooks::api::control::hook_skip(&HASH_A, 1);
    if add == Ok(1) && remove == Ok(1) && remove_again == Err(HookError::DoesntExist) {
        rshooks::api::control::accept(b"ok", 0);
    }
    rshooks::api::control::rollback(b"unexpected skip semantics", 77);
}

fn hook_skip_then_rollback(_r: u32) -> i64 {
    let r = rshooks::api::control::hook_skip(&HASH_B, 0);
    assert_eq!(r, Ok(1));
    rshooks::api::control::rollback(b"deliberate rollback", 1);
}

// -- hook_param_set (own-hash override precedence) --

const OVERRIDE_HASH: [u8; 32] = [0x77; 32];
const PARAM_NAME: &[u8] = b"K";

/// Calls `hook_param_set(value, PARAM_NAME, OVERRIDE_HASH)`, then reads
/// `hook_param(PARAM_NAME)` (same invocation) and records what it saw into
/// state (`b"seen"`) as either the raw value or a sentinel — proving the
/// documented "not visible within the same invocation" limitation (design
/// §4: overrides commit only on `accept!`, so a same-invocation read
/// consults only the world's already-committed value from a *previous*
/// invocation, if any).
fn set_override_and_record_same_invocation_read(_r: u32) -> i64 {
    let _ = rshooks::api::hook_ctx::hook_param_set(b"OVERRIDDEN", PARAM_NAME, &OVERRIDE_HASH);
    let mut buf = [0u8; 32];
    match rshooks::api::hook_ctx::hook_param(&mut buf, PARAM_NAME) {
        Ok(n) => {
            let _ = rshooks::api::state::state_set(&buf[..n], b"seen");
        }
        Err(_) => {
            let _ = rshooks::api::state::state_set(b"NONE", b"seen");
        }
    }
    rshooks::api::control::accept(b"ok", 0);
}

fn set_override_then_rollback(_r: u32) -> i64 {
    let _ =
        rshooks::api::hook_ctx::hook_param_set(b"SHOULD_NOT_COMMIT", PARAM_NAME, &OVERRIDE_HASH);
    rshooks::api::control::rollback(b"deliberate rollback", 1);
}

struct Chain;
impl HookChainEntries for Chain {
    const ENTRIES: &'static [NativeEntry] = &[
        NativeEntry {
            index: 0,
            name: "hook_again_once_then_accept",
            hook: hook_again_once_then_accept,
            cbak: None,
            can_emit: None,
        },
        NativeEntry {
            index: 1,
            name: "hook_again_twice",
            hook: hook_again_twice,
            cbak: None,
            can_emit: None,
        },
        NativeEntry {
            index: 2,
            name: "hook_again_then_rollback",
            hook: hook_again_then_rollback,
            cbak: None,
            can_emit: None,
        },
        NativeEntry {
            index: 3,
            name: "hook_skip_add",
            hook: hook_skip_add,
            cbak: None,
            can_emit: None,
        },
        NativeEntry {
            index: 4,
            name: "hook_skip_add_remove_remove_again",
            hook: hook_skip_add_remove_remove_again,
            cbak: None,
            can_emit: None,
        },
        NativeEntry {
            index: 5,
            name: "hook_skip_then_rollback",
            hook: hook_skip_then_rollback,
            cbak: None,
            can_emit: None,
        },
        NativeEntry {
            index: 6,
            name: "set_override_and_record_same_invocation_read",
            hook: set_override_and_record_same_invocation_read,
            cbak: None,
            can_emit: None,
        },
        NativeEntry {
            index: 7,
            name: "set_override_then_rollback",
            hook: set_override_then_rollback,
            cbak: None,
            can_emit: None,
        },
    ];
}

// -- hook_again --

#[test]
fn hook_again_second_call_in_the_same_invocation_is_already_set() {
    let env = TestEnv::new();
    let exit = env.invoke::<Chain>(1);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
}

#[test]
fn hook_again_requested_reflects_the_last_accepted_invocation() {
    let env = TestEnv::new();
    assert!(!env.hook_again_requested());

    let exit = env.invoke::<Chain>(0);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    assert!(env.hook_again_requested());
}

#[test]
fn hook_again_is_not_committed_on_rollback() {
    let env = TestEnv::new();
    let exit = env.invoke::<Chain>(2);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert!(
        !env.hook_again_requested(),
        "a rolled-back invocation's hook_again call must not commit"
    );
}

// -- hook_skip --

#[test]
fn hook_skip_add_remove_remove_again_matches_upstream_semantics() {
    let env = TestEnv::new();
    let exit = env.invoke::<Chain>(4);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
}

#[test]
fn skip_directives_are_committed_on_accept_and_recorded_verbatim() {
    let env = TestEnv::new();
    let exit = env.invoke::<Chain>(3);
    assert_eq!(exit.exit, ExitType::Accept, "{exit:?}");
    assert_eq!(env.skip_directives(), vec![(HASH_A, 0)]);
}

#[test]
fn skip_directives_are_not_committed_on_rollback() {
    let env = TestEnv::new();
    let exit = env.invoke::<Chain>(5);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");
    assert!(
        env.skip_directives().is_empty(),
        "a rolled-back invocation's hook_skip call must not commit"
    );
}

// -- hook_param_set: override precedence, accept-commit, rollback-discard --

#[test]
fn override_is_not_visible_within_the_same_invocation_but_is_after_accept() {
    let env = TestEnv::new().hook_pos(0).hook_hash(0, OVERRIDE_HASH);

    // Invoke #1: sets the override; the same-invocation read sees nothing
    // seeded yet (no `hook_param` builder call was made) -> "NONE".
    let exit1 = env.invoke::<Chain>(6);
    assert_eq!(exit1.exit, ExitType::Accept, "{exit1:?}");
    assert_eq!(env.state(b"seen"), Some(b"NONE".to_vec()));

    // Invoke #2 (same env, same hook_pos/hook_hash seeded): the override
    // committed by invoke #1's accept is now visible.
    let exit2 = env.invoke::<Chain>(6);
    assert_eq!(exit2.exit, ExitType::Accept, "{exit2:?}");
    assert_eq!(env.state(b"seen"), Some(b"OVERRIDDEN".to_vec()));
}

#[test]
fn override_precedence_beats_a_seeded_hook_param() {
    let env = TestEnv::new()
        .hook_pos(0)
        .hook_hash(0, OVERRIDE_HASH)
        .hook_param(PARAM_NAME, b"SEEDED");

    // Before any hook_param_set: the seeded env-level param is visible.
    let exit1 = env.invoke::<Chain>(6);
    assert_eq!(exit1.exit, ExitType::Accept, "{exit1:?}");
    assert_eq!(env.state(b"seen"), Some(b"SEEDED".to_vec()));

    // After invoke #1 commits its override on accept, invoke #2 sees the
    // override instead of the seeded value.
    let exit2 = env.invoke::<Chain>(6);
    assert_eq!(exit2.exit, ExitType::Accept, "{exit2:?}");
    assert_eq!(env.state(b"seen"), Some(b"OVERRIDDEN".to_vec()));
}

#[test]
fn param_override_is_not_committed_on_rollback() {
    let env = TestEnv::new()
        .hook_pos(0)
        .hook_hash(0, OVERRIDE_HASH)
        .hook_param(PARAM_NAME, b"SEEDED");

    let exit = env.invoke::<Chain>(7);
    assert_eq!(exit.exit, ExitType::Rollback, "{exit:?}");

    // A later invoke still sees the original seeded value, not the
    // rolled-back override attempt.
    let exit2 = env.invoke::<Chain>(6);
    assert_eq!(exit2.exit, ExitType::Accept, "{exit2:?}");
    assert_eq!(env.state(b"seen"), Some(b"SEEDED".to_vec()));
}
