//! Differential tests for the flatten pass (`docs/DESIGN.md` §6.2b): every
//! fixture runs both *before* and *after* flattening in a real wasm
//! interpreter (`wasmi`, dev-only — never linked into `rshooks-build`
//! itself), against the same stubbed `env` import. An inlining bug must fail
//! these tests, not silently change hook semantics.
//!
//! Every fixture imports one `env` function, `obs` (`(i32,i32) -> i32`),
//! which the host stub logs every call to it (as `(a, b)` pairs, in order)
//! and answers with the pure, deterministic
//! `a.wrapping_mul(1000).wrapping_add(b)`.
//! Comparing the recorded call sequence plus both runs' return values is
//! therefore a complete behavioral comparison — the "scripted response"
//! `docs/DESIGN.md` §6.2b asks for, scripted as a formula of the (varying)
//! call-site arguments rather than a hand-authored sequence.
//!
//! Test code is exempt from the workspace's panic-freedom lints (`docs/DESIGN.md`
//! §8): `unwrap`/`expect` on a known-good fixture is idiomatic here.
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
/// named export (`"hook"` or `"cbak"`) with `param`. Returns the call's i64
/// result plus the full `(a, b)` call-observation log recorded during that
/// single call.
fn run(wasm: &[u8], export: &str, param: i32) -> (i64, Vec<(i32, i32)>) {
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
    // Flatten unconditionally ensures `_g` is imported in its output (R1,
    // `docs/DESIGN.md` §6.2b), even though these fixtures never call it —
    // flatten only inserts the import, not guard calls (the separate, later
    // guard pass does that). Defining `_g` here regardless is harmless and
    // lets one `run()` helper instantiate both pre- and post-flatten bytes.
    linker
        .func_wrap(
            "env",
            "_g",
            |_caller: Caller<'_, HostState>, _a: i32, _b: i32| -> i32 { 1 },
        )
        .expect("define env::_g");
    let instance = linker
        .instantiate(&mut store, &module)
        .expect("all imports satisfied")
        .start(&mut store)
        .expect("no start function to run, or it succeeds");
    let entry = instance
        .get_typed_func::<i32, i64>(&store, export)
        .unwrap_or_else(|_| panic!("`{export}` export with signature (i32) -> i64"));
    let result = entry
        .call(&mut store, param)
        .unwrap_or_else(|e| panic!("`{export}({param})` should not trap: {e}"));
    let calls = log.borrow().clone();
    (result, calls)
}

/// Flattens `wat` source and asserts that every named export behaves
/// identically before and after flattening, for each `(export, param)` in
/// `cases`: same return value, same `env::obs` call sequence. Returns the
/// flattened bytes and the flatten report, for fixture-specific follow-up
/// assertions (e.g. duplication notes, surviving function count).
fn assert_differential(
    wat: &str,
    cases: &[(&str, i32)],
) -> (Vec<u8>, rshooks_build::FlattenReport) {
    let pre = wat::parse_str(wat).expect("fixture is valid wat");
    let (post, report) = rshooks_build::flatten(&pre).expect("flatten succeeds");

    for &(export, param) in cases {
        let (pre_result, pre_calls) = run(&pre, export, param);
        let (post_result, post_calls) = run(&post, export, param);
        assert_eq!(
            pre_result, post_result,
            "`{export}({param})` return value diverged after flattening"
        );
        assert_eq!(
            pre_calls, post_calls,
            "`{export}({param})` host-call sequence diverged after flattening"
        );
    }
    (post, report)
}

/// Number of defined functions in `wasm` (function-section entries, not
/// counting imports).
fn defined_func_count(wasm: &[u8]) -> u32 {
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        if let wasmparser::Payload::FunctionSection(r) = payload.expect("valid wasm") {
            return r.count();
        }
    }
    0
}

/// Whether any defined function body in `wasm` contains a `block` or `br`
/// instruction. Confirms `docs/DESIGN.md` §6.2b: a callee with no `return`
/// (or whose only `return` is the trailing instruction) is spliced bare, no
/// wrapper `block`/`br` — since the fixtures below contain no `block`/`br`
/// of their own, any survivor must come from `emit_inlined`'s wrapper path.
fn contains_block_or_br(wasm: &[u8]) -> bool {
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        if let wasmparser::Payload::CodeSectionEntry(body) = payload.expect("valid wasm") {
            let mut reader = body.get_operators_reader().expect("operators");
            while !reader.eof() {
                if matches!(
                    reader.read().expect("operator"),
                    wasmparser::Operator::Block { .. } | wasmparser::Operator::Br { .. }
                ) {
                    return true;
                }
            }
        }
    }
    false
}

// Fixture 1: a callee with params + a result, used at 2 call sites.
const TWO_CALL_SITES: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $helper (param $a i32) (param $b i32) (result i32)
    (i32.add (call $obs (local.get $a) (local.get $b)) (local.get $a)))
  (func $hook (param i32) (result i64)
    (local $r1 i32) (local $r2 i32)
    (local.set $r1 (call $helper (i32.const 1) (i32.const 2)))
    (local.set $r2 (call $helper (i32.const 10) (i32.const 20)))
    (drop (call $obs (local.get $r1) (local.get $r2)))
    (i64.extend_i32_s (i32.add (local.get $r1) (local.get $r2))))
  (export "hook" (func $hook)))
"#;

#[test]
fn two_call_sites_duplicate_and_match() {
    let (post, report) = assert_differential(TWO_CALL_SITES, &[("hook", 0)]);
    assert_eq!(
        defined_func_count(&post),
        1,
        "only `hook` should survive flattening"
    );
    assert!(
        report.notes.iter().any(|n| n.contains("2 call sites")),
        "expected a duplication note for the 2-call-site helper: {:?}",
        report.notes
    );
    // `$helper` has no `return`, so both inlined copies splice bare
    // (`docs/DESIGN.md` §6.2b) — no wrapper `block`/`br` despite duplication.
    assert!(
        !contains_block_or_br(&post),
        "return-free callee should splice bare, even when duplicated across call sites"
    );
}

// Fixture 2: a callee with its own locals and an internal block/br, no
// `return` — isolates "internal branch depths are left alone" from the
// `return`-rewriting fixture below.
const INTERNAL_BLOCK_BR: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $helper (param $x i32) (result i32)
    (local $tmp i32)
    (local.set $tmp (i32.const 0))
    (block $b
      (local.set $tmp (call $obs (local.get $x) (i32.const 1)))
      (br_if $b (i32.eqz (local.get $x)))
      (local.set $tmp (i32.add (local.get $tmp) (call $obs (local.get $x) (i32.const 2)))))
    (local.get $tmp))
  (func $hook (param i32) (result i64)
    (i64.extend_i32_s (call $helper (local.get 0))))
  (export "hook" (func $hook)))
"#;

#[test]
fn internal_block_br_both_branches() {
    // param 0 takes the `br_if`, skipping the second `obs` call; any other
    // param falls through to it. Both must match pre/post-flatten.
    assert_differential(INTERNAL_BLOCK_BR, &[("hook", 0), ("hook", 5)]);
}

// Fixture 3: a callee with `return` from inside nested blocks — the critical
// depth-rewrite case. `return` sits 2 blocks deep; the correct rewrite is
// `br 2` (skip both original blocks, then the wrapper).
const RETURN_FROM_NESTED_BLOCKS: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $helper (param $x i32) (result i32)
    (block $outer
      (block $inner
        (br_if $inner (i32.eqz (local.get $x)))
        (return (call $obs (local.get $x) (i32.const 100))))
      (drop (call $obs (i32.const 0) (i32.const 200))))
    (call $obs (i32.const 999) (i32.const 999)))
  (func $hook (param i32) (result i64)
    (i64.extend_i32_s (call $helper (local.get 0))))
  (export "hook" (func $hook)))
"#;

#[test]
fn return_from_nested_blocks_rewrites_depth_correctly() {
    // x == 0: `br_if` fires, `return` is skipped -> both later `obs` calls
    // happen, falling through to the last one's result. x != 0: `return`
    // fires from 2 blocks deep -> only the first `obs` call happens.
    assert_differential(
        RETURN_FROM_NESTED_BLOCKS,
        &[("hook", 0), ("hook", 7), ("hook", -3)],
    );
}

// Fixture 4: a callee containing a loop.
const CALLEE_WITH_LOOP: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $helper (param $n i32) (result i32)
    (local $i i32) (local $acc i32)
    (local.set $i (i32.const 0))
    (local.set $acc (i32.const 0))
    (block $exit
      (loop $l
        (br_if $exit (i32.ge_u (local.get $i) (local.get $n)))
        (local.set $acc (i32.add (local.get $acc) (call $obs (local.get $i) (i32.const 7))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $l)))
    (local.get $acc))
  (func $hook (param i32) (result i64)
    (i64.extend_i32_s (call $helper (local.get 0))))
  (export "hook" (func $hook)))
"#;

#[test]
fn callee_with_loop_matches() {
    assert_differential(CALLEE_WITH_LOOP, &[("hook", 0), ("hook", 1), ("hook", 4)]);
}

// Fixture 5: chained calls A -> B -> C, exercising reverse-topological
// processing order: C must be fully flattened before it is inlined into B,
// and B before it is inlined into A.
const CHAINED_CALLS: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $c (param $x i32) (result i32) (call $obs (local.get $x) (i32.const 3)))
  (func $b (param $x i32) (result i32)
    (i32.add (call $c (local.get $x)) (call $obs (local.get $x) (i32.const 2))))
  (func $a (param $x i32) (result i32)
    (i32.add (call $b (local.get $x)) (call $obs (local.get $x) (i32.const 1))))
  (func $hook (param i32) (result i64)
    (i64.extend_i32_s (call $a (local.get 0))))
  (export "hook" (func $hook)))
"#;

#[test]
fn chained_calls_flatten_in_topological_order() {
    let (post, _report) = assert_differential(CHAINED_CALLS, &[("hook", 6)]);
    assert_eq!(
        defined_func_count(&post),
        1,
        "a, b, c should all have been inlined away, leaving only hook"
    );
}

// Fixture 6: a void-result callee.
const VOID_RESULT_CALLEE: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $sideeffect (param $x i32)
    (drop (call $obs (local.get $x) (i32.const 42))))
  (func $hook (param i32) (result i64)
    (call $sideeffect (local.get 0))
    (call $sideeffect (i32.const 999))
    (i64.const 7))
  (export "hook" (func $hook)))
"#;

#[test]
fn void_result_callee_matches() {
    let (post, _report) = assert_differential(VOID_RESULT_CALLEE, &[("hook", 5)]);
    assert!(
        !contains_block_or_br(&post),
        "return-free void callee should splice bare"
    );
}

// Fixture 6b: a callee whose *only* `return` is the trailing instruction at
// depth 0 — `docs/DESIGN.md` §6.2b says this should also splice bare (drop
// the `return`, let the value fall through), same as a return-free body.
const TRAILING_RETURN_ONLY: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $helper (param $x i32) (result i32)
    (return (call $obs (local.get $x) (i32.const 9))))
  (func $hook (param i32) (result i64)
    (i64.extend_i32_s (call $helper (local.get 0))))
  (export "hook" (func $hook)))
"#;

#[test]
fn trailing_return_only_splices_bare() {
    let (post, _report) = assert_differential(TRAILING_RETURN_ONLY, &[("hook", 3), ("hook", -1)]);
    assert!(
        !contains_block_or_br(&post),
        "a callee whose only `return` is the trailing instruction should splice bare, with no \
         wrapper `block`/`br` introduced"
    );
}

// Fixture 7: an entry that keeps working when another defined function is
// dropped, and two entries (hook, cbak) sharing one helper (each call site
// gets its own duplicated copy).
const SHARED_HELPER_TWO_ENTRIES: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $shared (param $x i32) (result i32) (call $obs (local.get $x) (i32.const 5)))
  (func $hook (param i32) (result i64) (i64.extend_i32_s (call $shared (local.get 0))))
  (func $cbak (param i32) (result i64) (i64.extend_i32_s (call $shared (i32.const 77))))
  (export "hook" (func $hook))
  (export "cbak" (func $cbak)))
"#;

#[test]
fn shared_helper_across_two_entries() {
    let (post, report) = assert_differential(
        SHARED_HELPER_TWO_ENTRIES,
        &[("hook", 3), ("hook", -1), ("cbak", 0)],
    );
    assert_eq!(
        defined_func_count(&post),
        2,
        "only hook and cbak should survive; `shared` should have been dropped"
    );
    assert!(
        report.notes.iter().any(|n| n.contains("2 call sites")),
        "expected a duplication note for `shared` (inlined into both hook and cbak): {:?}",
        report.notes
    );
}

// Fixture 8: a callee that exits through a `br` targeting its implicit
// function-level label (`return` semantics) instead of `return`. A bare
// splice would leave the branch's depth unchanged and retarget it at the
// caller's enclosing block — skipping the addition below while still
// producing valid wasm — so this callee must be spliced under a wrapper
// block.
const FUNCTION_LEVEL_BRANCH_CALLEE: &str = r#"
(module
  (import "env" "obs" (func $obs (param i32 i32) (result i32)))
  (func $one (result i64)
    (br 0 (i64.const 1)))
  (func $hook (param i32) (result i64)
    (drop (call $obs (local.get 0) (i32.const 9)))
    (block (result i64)
      (i64.add (call $one) (i64.const 2))))
  (export "hook" (func $hook)))
"#;

#[test]
fn function_level_branch_in_callee_keeps_return_semantics() {
    assert_differential(FUNCTION_LEVEL_BRANCH_CALLEE, &[("hook", 5), ("hook", 0)]);
}
