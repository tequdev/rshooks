//! Differential + structural tests for the unnest pass (`docs/DESIGN.md`
//! §6.2c): every fixture is executed both *before* and *after* unnesting, in
//! a real wasm interpreter (`wasmi`, a dev-dependency), with the same
//! stubbed `env` import. Fixtures whose diverging tails trap (by design —
//! that's what makes them "diverging") are driven down both their success
//! path (no trap: same return value, same host-call sequence) and their
//! error path(s) (both pre- and post-unnest must trap, with an identical
//! host-call sequence recorded *before* the trap).
//!
//! Test code is exempt from the workspace's panic-freedom lints (per
//! `docs/DESIGN.md` §8): `unwrap`/`expect` on a known-good fixture is the
//! normal, idiomatic way to assert behavior in a test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::cell::RefCell;
use std::rc::Rc;

use wasmi::{Caller, Engine, Linker, Module, Store};

/// Host state: the shared call-observation log.
struct HostState {
    log: Rc<RefCell<Vec<(i32, i32)>>>,
}

/// Instantiates `wasm` with the standard `env::obs` stub and calls the
/// named export with `param`. Returns `Ok(result)` if the call completed
/// normally, or `Err(())` if it trapped (the fixtures below all trap via
/// `unreachable`, which is the entire point of a "diverging tail" — we only
/// care *that* both sides trap, not the exact trap-message text), plus the
/// full `(a, b)` call-observation log recorded up to that point.
fn run(wasm: &[u8], export: &str, param: i32) -> (Result<i64, ()>, Vec<(i32, i32)>) {
    let engine = Engine::default();
    let module = Module::new(&engine, wasm).expect("fixture is valid wasm");
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut store = Store::new(&engine, HostState { log: log.clone() });
    let mut linker = <Linker<HostState>>::new(&engine);
    linker
        .func_wrap(
            "env",
            "obs",
            |caller: Caller<'_, HostState>, a: i32, b: i32| -> i32 {
                caller.data().log.borrow_mut().push((a, b));
                a.wrapping_mul(1000).wrapping_add(b)
            },
        )
        .expect("define env::obs");
    let instance = linker
        .instantiate(&mut store, &module)
        .expect("all imports satisfied")
        .start(&mut store)
        .expect("no start function to run, or it succeeds");
    let entry = instance
        .get_typed_func::<i32, i64>(&store, export)
        .unwrap_or_else(|_| panic!("`{export}` export with signature (i32) -> i64"));
    let result = entry.call(&mut store, param).map_err(|_| ());
    let calls = log.borrow().clone();
    (result, calls)
}

/// Unnests `wat` source and asserts that every named export behaves
/// identically before and after, for each `(export, param)` in `cases`:
/// same ok/trap outcome, same return value when `Ok`, same `env::obs` call
/// sequence. Returns the unnested bytes and the unnest report, for
/// fixture-specific structural follow-up assertions.
fn assert_differential(wat: &str, cases: &[(&str, i32)]) -> (Vec<u8>, rshooks_build::UnnestReport) {
    let pre = wat::parse_str(wat).expect("fixture is valid wat");
    let (post, report) = rshooks_build::unnest(&pre).expect("unnest succeeds");

    for &(export, param) in cases {
        let (pre_result, pre_calls) = run(&pre, export, param);
        let (post_result, post_calls) = run(&post, export, param);
        assert_eq!(
            pre_result.is_ok(),
            post_result.is_ok(),
            "`{export}({param})` ok/trap outcome diverged after unnesting"
        );
        if let (Ok(a), Ok(b)) = (&pre_result, &post_result) {
            assert_eq!(
                a, b,
                "`{export}({param})` return value diverged after unnesting"
            );
        }
        assert_eq!(
            pre_calls, post_calls,
            "`{export}({param})` host-call sequence diverged after unnesting"
        );
    }
    (post, report)
}

/// Computes the maximum simultaneous `block`/`loop`/`if` nesting depth
/// across every defined function body in `wasm` (matches
/// `ir::max_nesting_depth`'s definition, duplicated here since that helper
/// is private to the crate).
fn max_nesting_depth(wasm: &[u8]) -> u32 {
    let mut overall = 0u32;
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        if let wasmparser::Payload::CodeSectionEntry(body) = payload.expect("valid wasm") {
            let mut depth = 0u32;
            let mut max_depth = 0u32;
            let mut reader = body.get_operators_reader().expect("operators");
            while !reader.eof() {
                match reader.read().expect("operator") {
                    wasmparser::Operator::Block { .. }
                    | wasmparser::Operator::Loop { .. }
                    | wasmparser::Operator::If { .. } => {
                        depth = depth.saturating_add(1);
                        max_depth = max_depth.max(depth);
                    }
                    wasmparser::Operator::End => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
            overall = overall.max(max_depth);
        }
    }
    overall
}

/// Whether any defined function body in `wasm` contains a `block`
/// instruction at all.
fn contains_block(wasm: &[u8]) -> bool {
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        if let wasmparser::Payload::CodeSectionEntry(body) = payload.expect("valid wasm") {
            let mut reader = body.get_operators_reader().expect("operators");
            while !reader.eof() {
                if matches!(
                    reader.read().expect("operator"),
                    wasmparser::Operator::Block { .. }
                ) {
                    return true;
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------
// Fixture (a): a classic 3-level error ladder. Three nested empty blocks,
// each with a `br_if` in the innermost body targeting one of them, and a
// self-contained diverging tail (an `obs` call + `unreachable`) right after
// each block's own `end`. The success path falls all the way through to a
// `return`.
// ---------------------------------------------------------------------

const CLASSIC_LADDER: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $hook (param $x i32) (result i64)
    (block $L2
      (block $L1
        (block $L0
          (br_if $L0 (i32.eqz (local.get $x)))
          (br_if $L1 (i32.eq (local.get $x) (i32.const 1)))
          (br_if $L2 (i32.eq (local.get $x) (i32.const 2)))
          (return (i64.extend_i32_s (call $obs (local.get $x) (i32.const 999)))))
        (drop (call $obs (i32.const 100) (local.get $x)))
        (unreachable))
      (drop (call $obs (i32.const 200) (local.get $x)))
      (unreachable))
    (drop (call $obs (i32.const 300) (local.get $x)))
    (unreachable))
  (export "hook" (func $hook)))
"#;

#[test]
fn classic_three_level_ladder_collapses_and_matches() {
    let pre = wat::parse_str(CLASSIC_LADDER).expect("fixture is valid wat");
    let pre_depth = max_nesting_depth(&pre);

    // x=99: success path, no trap. x=0,1,2: each takes a distinct rollback
    // tail and traps.
    let (post, report) = assert_differential(
        CLASSIC_LADDER,
        &[("hook", 99), ("hook", 0), ("hook", 1), ("hook", 2)],
    );

    assert_eq!(
        report.blocks_removed, 3,
        "all 3 ladder blocks should have been unwrapped"
    );
    assert_eq!(
        report.tails_duplicated, 3,
        "all 3 `br_if`s should have been rewritten into spliced tails"
    );

    let post_depth = max_nesting_depth(&post);
    assert!(
        post_depth < pre_depth,
        "max nesting depth should have decreased: {pre_depth} -> {post_depth}"
    );
    assert_eq!(pre_depth, 3, "sanity check on the fixture's own nesting");
    assert_eq!(
        post_depth, 1,
        "the 3 nested ladder blocks collapse to 3 sibling `if`s, each 1 level deep"
    );

    assert!(
        !contains_block(&post),
        "no `block` should remain at all: the fixture defines no block other than the 3 ladder \
         levels, all of which should have been fully unwrapped"
    );
}

// ---------------------------------------------------------------------
// Fixture (b): an unconditional `br` to a diverging tail, alongside a
// `br_if` targeting the *same* block (so this fixture also cross-checks
// that mixing conditional and unconditional branches to one qualifying
// block is handled correctly).
// ---------------------------------------------------------------------

const UNCONDITIONAL_BR_TAIL: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $hook (param $x i32) (result i64)
    (block $L0
      (br_if $L0 (i32.eqz (local.get $x)))
      (drop (call $obs (i32.const 7) (local.get $x)))
      (br $L0))
    (drop (call $obs (i32.const 42) (local.get $x)))
    (unreachable))
  (export "hook" (func $hook)))
"#;

#[test]
fn unconditional_br_to_diverging_tail_matches() {
    // x=0: `br_if` fires immediately, skipping the `obs(7, x)` call.
    // x=5: `br_if` doesn't fire, `obs(7, x)` runs, then the unconditional
    // `br` exits unconditionally. Both paths converge on the same tail.
    let (post, report) = assert_differential(UNCONDITIONAL_BR_TAIL, &[("hook", 0), ("hook", 5)]);
    assert_eq!(report.blocks_removed, 1);
    assert_eq!(
        report.tails_duplicated, 2,
        "both the `br_if` and the plain `br` targeting $L0 should have been rewritten"
    );
    assert!(!contains_block(&post));
}

// ---------------------------------------------------------------------
// Fixture (c): a `br_table` targeting a block whose continuation would
// otherwise qualify as a diverging tail. Per `docs/DESIGN.md` §6.2c, this
// must be left entirely unhandled: the block must survive untouched.
// ---------------------------------------------------------------------

const BR_TABLE_TARGETS_LADDER_BLOCK: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $hook (param $x i32) (result i64)
    (block $L0
      (br_table $L0 $L0 (local.get $x)))
    (drop (call $obs (i32.const 1) (local.get $x)))
    (unreachable))
  (export "hook" (func $hook)))
"#;

#[test]
fn br_table_targeting_ladder_block_is_left_untouched() {
    let (post, report) =
        assert_differential(BR_TABLE_TARGETS_LADDER_BLOCK, &[("hook", 0), ("hook", 5)]);
    assert_eq!(
        report.blocks_removed, 0,
        "a block targeted by a br_table must never be rewritten or removed"
    );
    assert_eq!(report.tails_duplicated, 0);
    assert!(
        contains_block(&post),
        "the block targeted by `br_table` must survive"
    );
}

// ---------------------------------------------------------------------
// Fixture (d): a continuation that is NOT self-contained (it uses
// `local.set`, which is outside the allowed op set for a diverging tail).
// No rewrite may happen.
// ---------------------------------------------------------------------

const NON_SELF_CONTAINED_TAIL: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $hook (param $x i32) (result i64)
    (local $tmp i32)
    (block $L0
      (br_if $L0 (i32.eqz (local.get $x)))
      (drop (call $obs (i32.const 1) (local.get $x))))
    (local.set $tmp (i32.const 5))
    (drop (call $obs (local.get $tmp) (local.get $x)))
    (unreachable))
  (export "hook" (func $hook)))
"#;

#[test]
fn non_self_contained_tail_is_not_rewritten() {
    // x=0: `br_if` fires, skipping `obs(1, x)`.
    // x=7: `br_if` doesn't fire, `obs(1, x)` runs, falls through to the
    // same continuation.
    let (post, report) = assert_differential(NON_SELF_CONTAINED_TAIL, &[("hook", 0), ("hook", 7)]);
    assert_eq!(
        report.blocks_removed, 0,
        "a continuation containing `local.set` must never qualify"
    );
    assert_eq!(report.tails_duplicated, 0);
    assert!(contains_block(&post));
}

// ---------------------------------------------------------------------
// Fixture (e): a block with a non-empty result type, whose continuation
// (dropping the block's own result value, then `unreachable`) would
// otherwise be exactly the shape of a qualifying diverging tail. Only
// empty-blocktype blocks qualify, so this must be left untouched.
// ---------------------------------------------------------------------

const NON_EMPTY_RESULT_BLOCK: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $hook (param $x i32) (result i64)
    (block $L0 (result i32)
      (call $obs (i32.const 1) (local.get $x)))
    (drop)
    (unreachable))
  (export "hook" (func $hook)))
"#;

#[test]
fn non_empty_result_block_is_not_rewritten() {
    // The block's own content doesn't matter for candidacy (only its own
    // blockty and what follows its `end` do) — two different `x` values are
    // enough to confirm behavior is preserved.
    let (post, report) = assert_differential(NON_EMPTY_RESULT_BLOCK, &[("hook", 0), ("hook", 5)]);
    assert_eq!(
        report.blocks_removed, 0,
        "a block with a non-empty result type must never qualify, even if its continuation \
         would otherwise be a valid diverging tail"
    );
    assert_eq!(report.tails_duplicated, 0);
    assert!(contains_block(&post));
}

// ---------------------------------------------------------------------
// Counts the number of `Operator::Call` instructions targeting import index
// `target`, and the number of `Operator::I32Const` instructions carrying
// `value`, across every defined function body in `wasm` — used below to
// confirm specific (live vs. dead) instruction occurrences, not just that
// *some* instruction count changed. `contains_loop` mirrors `contains_block`
// above but for `Operator::Loop`.
// ---------------------------------------------------------------------

fn count_calls(wasm: &[u8], target: u32) -> usize {
    let mut n: usize = 0;
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        if let wasmparser::Payload::CodeSectionEntry(body) = payload.expect("valid wasm") {
            let mut reader = body.get_operators_reader().expect("operators");
            while !reader.eof() {
                if let wasmparser::Operator::Call { function_index } =
                    reader.read().expect("operator")
                    && function_index == target
                {
                    n = n.saturating_add(1);
                }
            }
        }
    }
    n
}

fn count_i32_const(wasm: &[u8], value: i32) -> usize {
    let mut n: usize = 0;
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        if let wasmparser::Payload::CodeSectionEntry(body) = payload.expect("valid wasm") {
            let mut reader = body.get_operators_reader().expect("operators");
            while !reader.eof() {
                if let wasmparser::Operator::I32Const { value: v } =
                    reader.read().expect("operator")
                    && v == value
                {
                    n = n.saturating_add(1);
                }
            }
        }
    }
    n
}

fn contains_loop(wasm: &[u8]) -> bool {
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        if let wasmparser::Payload::CodeSectionEntry(body) = payload.expect("valid wasm") {
            let mut reader = body.get_operators_reader().expect("operators");
            while !reader.eof() {
                if matches!(
                    reader.read().expect("operator"),
                    wasmparser::Operator::Loop { .. }
                ) {
                    return true;
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------
// Fixture (f): the motivating case from `docs/DESIGN.md` §6.2c — step 3
// unwraps ladder block `$L0`, but its *original* continuation (`obs(100,
// x)` then `unreachable`, right after where its `end` used to be) is left
// in place rather than removed, so it now sits directly after the
// unwrapped block body's own `unreachable` — dead code duplicating what
// step 2 already spliced into the rewritten `br_if`. A second, wholly
// unrelated dead group (`obs(999, x)` then `unreachable`) follows it,
// mirroring the multi-group leftover seen in `03_hook-params`'s real
// output. The post-pass must drop both dead groups in full, leaving only
// the one live copy of each call.
// ---------------------------------------------------------------------

const DEAD_TAIL_AFTER_LADDER_BLOCK: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $hook (param $x i32) (result i64)
    (block $L0
      (br_if $L0 (i32.eqz (local.get $x)))
      (drop (call $obs (i32.const 1) (local.get $x)))
      (unreachable))
    (drop (call $obs (i32.const 100) (local.get $x)))
    (unreachable)
    (drop (call $obs (i32.const 999) (local.get $x)))
    (unreachable))
  (export "hook" (func $hook)))
"#;

#[test]
fn dead_tail_after_ladder_block_is_eliminated() {
    // x=0: `br_if` fires, takes the spliced `obs(100, x)` tail (traps).
    // x=5: `br_if` doesn't fire, falls through to `obs(1, x)` (traps too —
    // this fixture has no non-trapping path, which is fine: both branches
    // are exercised and must match pre/post regardless).
    let (post, report) =
        assert_differential(DEAD_TAIL_AFTER_LADDER_BLOCK, &[("hook", 0), ("hook", 5)]);

    assert_eq!(report.blocks_removed, 1);
    assert_eq!(report.tails_duplicated, 1);
    assert_eq!(
        report.dead_ops_removed, 10,
        "both dead 5-op groups (the leftover duplicate `obs(100, x)` tail and the unrelated \
         `obs(999, x)` tail) should be reported removed"
    );
    assert_eq!(
        count_i32_const(&post, 999),
        0,
        "the `obs(999, x)` group is pure dead code and must be gone entirely"
    );
    assert_eq!(
        count_i32_const(&post, 100),
        1,
        "only the spliced copy of the `obs(100, x)` tail should survive; the leftover original \
         copy step 3 left in place must be eliminated"
    );
    assert_eq!(
        count_calls(&post, 0),
        2,
        "exactly the spliced `obs(100, x)` call and the live `obs(1, x)` call should remain"
    );
}

// ---------------------------------------------------------------------
// Fixture (g): an entire nested `loop` — unnest's own steps never touch
// `loop` frames at all, so this isolates the dead-code pass's "drop a whole
// nested span" behavior from any interaction with steps 1–4 — sits after an
// unconditional `return` at the top level, i.e. the whole loop is dead
// code, not just a flat instruction run. It must be dropped as one unit
// (and the trailing top-level `unreachable` after it, itself dead too, must
// go as well).
// ---------------------------------------------------------------------

const DEAD_NESTED_LOOP: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $hook (param $x i32) (result i64)
    (return (i64.extend_i32_s (call $obs (local.get $x) (i32.const 1))))
    (loop $L0
      (drop (call $obs (i32.const 2) (local.get $x)))
      (br_if $L0 (i32.eqz (local.get $x))))
    (unreachable))
  (export "hook" (func $hook)))
"#;

#[test]
fn dead_nested_loop_is_dropped_as_a_whole_unit() {
    // Every path takes the unconditional `return` immediately; the dead
    // loop is never reachable for any `x`.
    let (post, report) = assert_differential(DEAD_NESTED_LOOP, &[("hook", 0), ("hook", 9)]);

    assert_eq!(
        report.blocks_removed, 0,
        "a `loop` is never a candidate for steps 1-3, which only ever touch `block` frames"
    );
    assert_eq!(report.tails_duplicated, 0);
    assert!(
        !contains_loop(&post),
        "the entire dead `$L0` loop must be dropped, not merely left in place"
    );
    assert_eq!(
        count_calls(&post, 0),
        1,
        "only the live `obs(1, x)` call on the `return` path should remain"
    );
    assert!(
        report.dead_ops_removed > 0,
        "the dead-code pass should have reported at least the loop's own dropped instructions"
    );
}
