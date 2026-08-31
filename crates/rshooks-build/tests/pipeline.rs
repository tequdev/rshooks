//! Integration tests for the rshooks-build pipeline (`docs/DESIGN.md` §8),
//! using `wat`-authored fixtures.
//!
//! Test code is exempt from the workspace's panic-freedom lints (`docs/DESIGN.md`
//! §8): `unwrap`/`expect`/`panic!`/indexing on a known-good fixture is
//! idiomatic here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use rshooks_build::Options;

fn wasm(src: &str) -> Vec<u8> {
    wat::parse_str(src).expect("fixture is valid wat")
}

fn opts() -> Options {
    Options::default()
}

/// Appends a raw custom section (id 0) to an existing wasm binary.
fn append_custom_section(wasm: &[u8], name: &str, payload: &[u8]) -> Vec<u8> {
    fn write_leb128(mut n: u64, out: &mut Vec<u8>) {
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
    }
    let mut content = Vec::new();
    write_leb128(name.len() as u64, &mut content);
    content.extend_from_slice(name.as_bytes());
    content.extend_from_slice(payload);

    let mut out = wasm.to_vec();
    out.push(0x00);
    write_leb128(content.len() as u64, &mut out);
    out.extend_from_slice(&content);
    out
}

/// Returns every `wasmparser::Payload` variant name found in `wasm`, for
/// simple structural assertions.
fn payload_kinds(wasm: &[u8]) -> Vec<&'static str> {
    let mut kinds = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        let payload = payload.expect("valid wasm");
        kinds.push(match payload {
            wasmparser::Payload::CustomSection(_) => "custom",
            wasmparser::Payload::ExportSection(_) => "export",
            _ => "other",
        });
    }
    kinds
}

fn export_names(wasm: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        if let wasmparser::Payload::ExportSection(reader) = payload.expect("valid wasm") {
            for e in reader {
                names.push(e.expect("valid export").name.to_string());
            }
        }
    }
    names
}

/// Returns every *active* data segment in `wasm` as `(i32 offset, bytes)`
/// pairs, in section order. Panics on a passive segment or a non-`i32.const`
/// offset expression — neither occurs in these fixtures.
fn data_segments(wasm: &[u8]) -> Vec<(i32, Vec<u8>)> {
    let mut out = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        if let wasmparser::Payload::DataSection(reader) = payload.expect("valid wasm") {
            for d in reader {
                let d = d.expect("valid data segment");
                match d.kind {
                    wasmparser::DataKind::Active { offset_expr, .. } => {
                        let mut ops = offset_expr.get_operators_reader();
                        let op = ops.read().expect("offset expr operator");
                        let wasmparser::Operator::I32Const { value } = op else {
                            panic!("expected an `i32.const` offset expression, got {op:?}")
                        };
                        out.push((value, d.data.to_vec()));
                    }
                    wasmparser::DataKind::Passive => panic!("unexpected passive data segment"),
                }
            }
        }
    }
    out
}

// Cleaner

const MINIMAL_HOOK: &str = r#"
(module
  (import "env" "accept" (func $accept (param i32 i32 i64) (result i64)))
  (memory 1)
  (export "memory" (memory 0))
  (func $hook (param i32) (result i64)
    (call $accept (i32.const 0) (i32.const 0) (i64.const 0)))
  (export "hook" (func $hook)))
"#;

#[test]
fn cleaner_strips_memory_export_and_custom_sections() {
    let input = wasm(MINIMAL_HOOK);
    let input = append_custom_section(&input, "producers", b"whatever");
    assert!(payload_kinds(&input).contains(&"custom"));

    let cleaned = rshooks_build::clean(&input, &opts()).expect("clean succeeds");

    assert!(
        !payload_kinds(&cleaned).contains(&"custom"),
        "custom section was not stripped"
    );
    let exports = export_names(&cleaned);
    assert_eq!(
        exports,
        vec!["hook".to_string()],
        "only `hook` should remain exported"
    );
}

#[test]
fn cleaner_keeps_cbak_when_present() {
    let src = r#"
    (module
      (import "env" "accept" (func $accept (param i32 i32 i64) (result i64)))
      (func $hook (param i32) (result i64)
        (call $accept (i32.const 0) (i32.const 0) (i64.const 0)))
      (func $cbak (param i32) (result i64)
        (call $accept (i32.const 0) (i32.const 0) (i64.const 0)))
      (export "hook" (func $hook))
      (export "cbak" (func $cbak)))
    "#;
    let cleaned = rshooks_build::clean(&wasm(src), &opts()).expect("clean succeeds");
    let mut exports = export_names(&cleaned);
    exports.sort();
    assert_eq!(exports, vec!["cbak".to_string(), "hook".to_string()]);
}

#[test]
fn cleaner_errors_without_hook_export() {
    let src = r#"
    (module
      (func $notthehook (param i32) (result i64) (i64.const 0))
      (export "notthehook" (func $notthehook)))
    "#;
    let err = rshooks_build::clean(&wasm(src), &opts()).unwrap_err();
    assert!(
        err.to_string().contains("hook"),
        "error should mention the missing `hook` export: {err}"
    );
}

#[test]
fn gc_drops_unreachable_function_and_remaps_calls() {
    let src = r#"
    (module
      (import "env" "accept" (func $accept (param i32 i32 i64) (result i64)))
      (func $dead (param i32) (result i64) (i64.const 0))
      (func $helper (param i32) (result i64) (i64.const 42))
      (func $hook (param i32) (result i64)
        (drop (call $helper (i32.const 0)))
        (call $accept (i32.const 0) (i32.const 0) (i64.const 0)))
      (export "hook" (func $hook)))
    "#;
    let cleaned = rshooks_build::clean(&wasm(src), &opts()).expect("clean succeeds");

    // Exactly 2 defined functions should remain (helper, hook) — `dead`
    // dropped — and hook's call to helper remapped from index 2 to 1.
    let mut new_func_count = 0u32;
    let mut hook_new_idx = None;
    for payload in wasmparser::Parser::new(0).parse_all(&cleaned) {
        match payload.expect("valid wasm") {
            wasmparser::Payload::FunctionSection(r) => new_func_count = r.count(),
            wasmparser::Payload::ExportSection(r) => {
                for e in r {
                    let e = e.expect("export");
                    if e.name == "hook" {
                        hook_new_idx = Some(e.index);
                    }
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        new_func_count, 2,
        "the unreachable `dead` function should have been dropped"
    );
    let hook_new_idx = hook_new_idx.expect("hook export present");
    assert_eq!(
        hook_new_idx, 2,
        "hook should now be function index 2 (import 0, helper 1, hook 2)"
    );

    // hook's body is defined-function local index 1 (second code-section
    // entry); check its `call` immediate.
    let mut code_bodies = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(&cleaned) {
        if let wasmparser::Payload::CodeSectionEntry(body) = payload.expect("valid wasm") {
            code_bodies.push(body.as_bytes().to_vec());
        }
    }
    let hook_body = code_bodies[1].clone();

    let reader = wasmparser::FunctionBody::new(wasmparser::BinaryReader::new(&hook_body, 0));
    let mut calls = Vec::new();
    let mut ops = reader.get_operators_reader().expect("operators");
    while !ops.eof() {
        if let wasmparser::Operator::Call { function_index } = ops.read().expect("op") {
            calls.push(function_index);
        }
    }
    assert_eq!(
        calls,
        vec![1, 0],
        "hook should call helper (remapped 2->1) then accept (unchanged 0)"
    );

    // Byte-level: re-encoded body must contain `call 1` (0x10 0x01), not
    // `call 2` (0x10 0x02).
    assert!(
        contains_bytes(&hook_body, &[0x10, 0x01]),
        "expected `call 1` in re-encoded body"
    );
    assert!(
        !contains_bytes(&hook_body, &[0x10, 0x02]),
        "stale `call 2` immediate leaked into re-encoded body"
    );
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

// Cleaner: trailing-zero data-segment trim (`docs/DESIGN.md` §6.2 step 3)

#[test]
fn cleaner_trims_trailing_zeros_from_active_data_segment() {
    // Offset 4 (not 0) so a bug that zeroed/dropped the offset expression
    // instead of trimming the payload would be caught too.
    let src = r#"
    (module
      (import "env" "accept" (func $accept (param i32 i32 i64) (result i64)))
      (memory 1)
      (data (i32.const 4) "AB\00\00\00\00\00")
      (func $hook (param i32) (result i64)
        (call $accept (i32.const 0) (i32.const 0) (i64.const 0)))
      (export "hook" (func $hook)))
    "#;
    let cleaned = rshooks_build::clean(&wasm(src), &opts()).expect("clean succeeds");
    let segs = data_segments(&cleaned);
    assert_eq!(
        segs,
        vec![(4, b"AB".to_vec())],
        "trailing zero bytes should be trimmed; offset must be unchanged"
    );
}

#[test]
fn cleaner_skips_trim_when_data_segments_overlap() {
    // Active segments apply in declaration order and may legally overlap:
    // here the second segment's trailing zero overwrites the first
    // segment's non-zero byte at address 6. Trimming that zero would leave
    // 0x43 ('C') in memory instead of 0x00, so the trim must be skipped
    // wholesale and both payloads pass through byte-identical.
    let src = r#"
    (module
      (import "env" "accept" (func $accept (param i32 i32 i64) (result i64)))
      (memory 1)
      (data (i32.const 4) "ABC")
      (data (i32.const 5) "X\00")
      (func $hook (param i32) (result i64)
        (call $accept (i32.const 0) (i32.const 0) (i64.const 0)))
      (export "hook" (func $hook)))
    "#;
    let cleaned = rshooks_build::clean(&wasm(src), &opts()).expect("clean succeeds");
    let segs = data_segments(&cleaned);
    assert_eq!(
        segs,
        vec![(4, b"ABC".to_vec()), (5, b"X\x00".to_vec())],
        "overlapping segments must disable the trailing-zero trim entirely"
    );
}

#[test]
fn cleaner_drops_all_zero_active_data_segment() {
    let src = r#"
    (module
      (import "env" "accept" (func $accept (param i32 i32 i64) (result i64)))
      (memory 1)
      (data (i32.const 0) "\00\00\00\00\00")
      (func $hook (param i32) (result i64)
        (call $accept (i32.const 0) (i32.const 0) (i64.const 0)))
      (export "hook" (func $hook)))
    "#;
    let cleaned = rshooks_build::clean(&wasm(src), &opts()).expect("clean succeeds");
    assert!(
        data_segments(&cleaned).is_empty(),
        "an all-zero segment should be dropped entirely, not emitted empty"
    );
}

#[test]
fn cleaner_leaves_data_segment_with_nonzero_tail_untouched() {
    let src = r#"
    (module
      (import "env" "accept" (func $accept (param i32 i32 i64) (result i64)))
      (memory 1)
      (data (i32.const 0) "ABCD")
      (func $hook (param i32) (result i64)
        (call $accept (i32.const 0) (i32.const 0) (i64.const 0)))
      (export "hook" (func $hook)))
    "#;
    let cleaned = rshooks_build::clean(&wasm(src), &opts()).expect("clean succeeds");
    assert_eq!(
        data_segments(&cleaned),
        vec![(0, b"ABCD".to_vec())],
        "a segment with no trailing zero byte must be left byte-for-byte alone"
    );
}

#[test]
fn cleaner_data_segment_trim_is_idempotent() {
    let src = r#"
    (module
      (import "env" "accept" (func $accept (param i32 i32 i64) (result i64)))
      (memory 1)
      (data (i32.const 0) "AB\00\00\00\00\00")
      (data (i32.const 16) "\00\00\00\00")
      (data (i32.const 32) "CD")
      (func $hook (param i32) (result i64)
        (call $accept (i32.const 0) (i32.const 0) (i64.const 0)))
      (export "hook" (func $hook)))
    "#;
    let once = rshooks_build::clean(&wasm(src), &opts()).expect("clean succeeds");
    let twice = rshooks_build::clean(&once, &opts()).expect("re-clean succeeds");
    assert_eq!(
        once, twice,
        "re-cleaning an already-trimmed module must be byte-identical (no re-trimming, no drift)"
    );
}

// Guard pass

const GUARDED_LOOP_HOOK: &str = r#"
(module
  (import "env" "_g" (func $g (param i32 i32) (result i32)))
  (func $hook (param i32) (result i64)
    (local $i i32)
    (loop $l
      (call $g (i32.const 1) (i32.const 10))
      drop
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br_if $l (i32.lt_u (local.get $i) (i32.const 10))))
    (i64.const 0))
  (export "hook" (func $hook)))
"#;

const UNGUARDED_LOOP_HOOK: &str = r#"
(module
  (func $hook (param i32) (result i64)
    (local $i i32)
    (loop $l
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br_if $l (i32.lt_u (local.get $i) (i32.const 10))))
    (i64.const 0))
  (export "hook" (func $hook)))
"#;

#[test]
fn validator_accepts_guarded_loop() {
    rshooks_build::validate(&wasm(GUARDED_LOOP_HOOK), &opts()).expect("guarded loop should pass");
}

#[test]
fn validator_rejects_unguarded_loop_with_location() {
    let err = rshooks_build::validate(&wasm(UNGUARDED_LOOP_HOOK), &opts()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("function 0"),
        "error should name the function: {msg}"
    );
    assert!(
        msg.contains("offset"),
        "error should give a byte offset: {msg}"
    );
    assert!(
        msg.to_lowercase().contains("guard"),
        "error should mention the missing guard: {msg}"
    );
    assert!(
        !msg.contains("looks like"),
        "a loop with no load/store/call should get no shape hint (conservative default): {msg}"
    );
}

// Guard pass: actionable hints for compiler-generated loop shapes

const COMPARE_LIKE_LOOP_HOOK: &str = r#"
(module
  (memory 1)
  (func $hook (param i32) (result i64)
    (local $i i32) (local $a i32) (local $b i32)
    (loop $l
      (local.set $a (i32.load8_u (local.get $i)))
      (local.set $b (i32.load8_u (i32.add (local.get $i) (i32.const 100))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br_if $l (i32.lt_u (local.get $i) (i32.const 20))))
    (i64.const 0))
  (export "hook" (func $hook)))
"#;

const COPY_LIKE_LOOP_HOOK: &str = r#"
(module
  (memory 1)
  (func $hook (param i32) (result i64)
    (local $i i32) (local $v i32)
    (loop $l
      (local.set $v (i32.load8_u (local.get $i)))
      (i32.store8 (i32.add (local.get $i) (i32.const 100)) (local.get $v))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br_if $l (i32.lt_u (local.get $i) (i32.const 300))))
    (i64.const 0))
  (export "hook" (func $hook)))
"#;

const ZERO_INIT_LIKE_LOOP_HOOK: &str = r#"
(module
  (memory 1)
  (func $hook (param i32) (result i64)
    (local $i i32)
    (loop $l
      (i32.store8 (local.get $i) (i32.const 0))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br_if $l (i32.lt_u (local.get $i) (i32.const 300))))
    (i64.const 0))
  (export "hook" (func $hook)))
"#;

// A loop that loads memory (like `COMPARE_LIKE_LOOP_HOOK`) but also calls an
// imported Hook API function — the shape a real business-logic loop over
// hook state would have. The heuristic must stay conservative: a `call`
// anywhere in the loop body suppresses the hint entirely, even with loads
// present.
const CALLING_LOOP_HOOK: &str = r#"
(module
  (import "env" "state" (func $state (param i32 i32 i32 i32) (result i64)))
  (memory 1)
  (func $hook (param i32) (result i64)
    (local $i i32)
    (loop $l
      (drop (call $state (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br_if $l (i32.lt_u (local.get $i) (i32.const 10))))
    (i64.const 0))
  (export "hook" (func $hook)))
"#;

#[test]
fn validator_hints_buf_eq_for_compare_like_loop() {
    let err = rshooks_build::validate(&wasm(COMPARE_LIKE_LOOP_HOOK), &opts()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("looks like a compiler-generated byte-compare loop"),
        "expected a compare-loop hint: {msg}"
    );
    assert!(
        msg.contains("buf_eq"),
        "hint should point at rshooks::buf_eq_<N>: {msg}"
    );
}

#[test]
fn validator_hints_static_for_copy_like_loop() {
    let err = rshooks_build::validate(&wasm(COPY_LIKE_LOOP_HOOK), &opts()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("looks like a compiler-generated memcpy-style copy loop"),
        "expected a copy-loop hint: {msg}"
    );
}

#[test]
fn validator_hints_static_for_zero_init_like_loop() {
    let err = rshooks_build::validate(&wasm(ZERO_INIT_LIKE_LOOP_HOOK), &opts()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("looks like a compiler-generated memset-style zero-init loop"),
        "expected a zero-init-loop hint: {msg}"
    );
    assert!(
        msg.contains("HookStatic"),
        "hint should point at rshooks::static_cell::HookStatic: {msg}"
    );
}

#[test]
fn validator_suppresses_hint_when_loop_calls_an_import() {
    let err = rshooks_build::validate(&wasm(CALLING_LOOP_HOOK), &opts()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("guard"),
        "should still report the missing guard: {msg}"
    );
    assert!(
        !msg.contains("looks like"),
        "a loop that calls an import should get no shape hint, even though it loads memory \
         too (conservative heuristic, avoids false positives on real business logic): {msg}"
    );
}

#[test]
fn auto_guard_inserts_exact_pattern_and_passes_revalidation() {
    let input = wasm(UNGUARDED_LOOP_HOOK);
    let o = Options {
        auto_guard: true,
        default_maxiter: 16,
        ..Options::default()
    };
    let out = rshooks_build::auto_guard(&input, &o).expect("auto-guard succeeds");

    // `_g` should be added as the first (only) import, shifting `hook`
    // from function index 0 to 1.
    let mut g_index = None;
    let mut hook_index = None;
    let mut n_func_imports = 0u32;
    for payload in wasmparser::Parser::new(0).parse_all(&out) {
        match payload.expect("valid wasm") {
            wasmparser::Payload::ImportSection(r) => {
                for imp in r.into_imports() {
                    let imp = imp.expect("import");
                    if let wasmparser::TypeRef::Func(_) = imp.ty {
                        if imp.module == "env" && imp.name == "_g" {
                            g_index = Some(n_func_imports);
                        }
                        n_func_imports += 1;
                    }
                }
            }
            wasmparser::Payload::ExportSection(r) => {
                for e in r {
                    let e = e.expect("export");
                    if e.name == "hook" {
                        hook_index = Some(e.index);
                    }
                }
            }
            _ => {}
        }
    }
    let g_index = g_index.expect("_g should have been added as an import");
    assert_eq!(g_index, 0);
    let hook_index = hook_index.expect("hook export present");
    assert_eq!(
        hook_index, 1,
        "hook should have shifted from 0 to 1 once `_g` was inserted"
    );

    // hook's body should have the exact inserted instruction sequence
    // immediately following the loop header.
    let mut bodies = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(&out) {
        if let wasmparser::Payload::CodeSectionEntry(body) = payload.expect("valid wasm") {
            bodies.push(body.as_bytes().to_vec());
        }
    }
    let hook_body = &bodies[(hook_index - n_func_imports) as usize];
    let reader = wasmparser::FunctionBody::new(wasmparser::BinaryReader::new(hook_body, 0));
    let mut ops = reader.get_operators_reader().expect("operators");
    let mut found_loop = false;
    while !ops.eof() {
        let op = ops.read().expect("op");
        if let wasmparser::Operator::Loop { .. } = op {
            found_loop = true;
            let a = ops.read().expect("i32.const id");
            let b = ops.read().expect("i32.const maxiter");
            let c = ops.read().expect("call _g");
            let d = ops.read().expect("drop");
            assert!(matches!(a, wasmparser::Operator::I32Const { .. }));
            assert!(matches!(b, wasmparser::Operator::I32Const { value: 16 }));
            match c {
                wasmparser::Operator::Call { function_index } => {
                    assert_eq!(function_index, g_index)
                }
                other => panic!("expected `call $_g`, got {other:?}"),
            }
            assert!(matches!(d, wasmparser::Operator::Drop));
            break;
        }
    }
    assert!(found_loop, "fixture should contain a loop");

    // Byte-level: the inserted sequence's tail (`call <g_index>; drop`)
    // must appear verbatim.
    assert!(
        contains_bytes(hook_body, &[0x10, g_index as u8, 0x1A]),
        "expected `call {g_index}; drop` (0x10 {g_index:#x} 0x1A) in the rebuilt body"
    );

    rshooks_build::validate(&out, &opts()).expect("auto-guarded module should re-validate cleanly");
}

// Validator hard-error rules

// `_g` is imported (never called — no loops here) purely to satisfy R1
// (`docs/DESIGN.md` §6.2b/§6.4): every api-version-0 module must import
// `_g`, even without a single loop. This is a real, vendored-checker-
// discovered rule, not a pipeline artifact — `build`/`clean` guarantee it
// via the flatten pass, but `validate()` is exercised directly here,
// bypassing that pass, so the fixture must supply it itself.
const MINIMAL_HOOK_ALREADY_CLEAN: &str = r#"
(module
  (import "env" "_g" (func $g (param i32 i32) (result i32)))
  (import "env" "accept" (func $accept (param i32 i32 i64) (result i64)))
  (func $hook (param i32) (result i64)
    (call $accept (i32.const 0) (i32.const 0) (i64.const 0)))
  (export "hook" (func $hook)))
"#;

#[test]
fn validator_accepts_minimal_hook() {
    rshooks_build::validate(&wasm(MINIMAL_HOOK_ALREADY_CLEAN), &opts())
        .expect("minimal hook should validate");
}

#[test]
fn validator_rejects_extra_export() {
    let src = r#"
    (module
      (memory 1)
      (func $hook (param i32) (result i64) (i64.const 0))
      (export "hook" (func $hook))
      (export "memory" (memory 0)))
    "#;
    let err = rshooks_build::validate(&wasm(src), &opts()).unwrap_err();
    assert!(err.to_string().contains("memory"), "{err}");
}

#[test]
fn validator_rejects_non_env_import() {
    let src = r#"
    (module
      (import "wasi_snapshot_preview1" "accept" (func $accept (param i32 i32 i64) (result i64)))
      (func $hook (param i32) (result i64) (i64.const 0))
      (export "hook" (func $hook)))
    "#;
    let err = rshooks_build::validate(&wasm(src), &opts()).unwrap_err();
    assert!(err.to_string().contains("env"), "{err}");
}

#[test]
fn validator_rejects_unknown_import_name() {
    let src = r#"
    (module
      (import "env" "not_a_real_hook_fn" (func $x (param i32) (result i32)))
      (func $hook (param i32) (result i64) (i64.const 0))
      (export "hook" (func $hook)))
    "#;
    let err = rshooks_build::validate(&wasm(src), &opts()).unwrap_err();
    assert!(err.to_string().contains("not_a_real_hook_fn"), "{err}");
}

#[test]
fn validator_rejects_wrong_import_signature() {
    let src = r#"
    (module
      (import "env" "accept" (func $accept (param i32) (result i64)))
      (func $hook (param i32) (result i64) (i64.const 0))
      (export "hook" (func $hook)))
    "#;
    let err = rshooks_build::validate(&wasm(src), &opts()).unwrap_err();
    assert!(err.to_string().contains("accept"), "{err}");
}

#[test]
fn validator_rejects_float_opcode() {
    let src = r#"
    (module
      (func $hook (param i32) (result i64)
        (local $f f32)
        (local.set $f (f32.const 1.0))
        (i64.const 0))
      (export "hook" (func $hook)))
    "#;
    let err = rshooks_build::validate(&wasm(src), &opts()).unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("float") || err.to_string().contains("MVP"),
        "{err}"
    );
}

#[test]
fn validator_rejects_call_indirect() {
    let src = r#"
    (module
      (type $ft (func (param i32) (result i64)))
      (table 1 funcref)
      (func $hook (param i32) (result i64)
        (call_indirect (type $ft) (local.get 0) (i32.const 0)))
      (export "hook" (func $hook)))
    "#;
    let err = rshooks_build::validate(&wasm(src), &opts()).unwrap_err();
    assert!(err.to_string().contains("call_indirect"), "{err}");
}

#[test]
fn validator_rejects_direct_recursion() {
    let src = r#"
    (module
      (func $hook (param i32) (result i64)
        (call $hook (i32.const 0)))
      (export "hook" (func $hook)))
    "#;
    let err = rshooks_build::validate(&wasm(src), &opts()).unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("recurs")
            || err.to_string().to_lowercase().contains("cycle"),
        "{err}"
    );
}

#[test]
fn validator_rejects_mutual_recursion() {
    let src = r#"
    (module
      (func $hook (param i32) (result i64)
        (call $helper (i32.const 0)))
      (func $helper (param i32) (result i64)
        (call $hook (i32.const 0)))
      (export "hook" (func $hook)))
    "#;
    let err = rshooks_build::validate(&wasm(src), &opts()).unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("recurs")
            || err.to_string().to_lowercase().contains("cycle"),
        "{err}"
    );
}

#[test]
fn validator_rejects_start_section() {
    let src = r#"
    (module
      (func $init)
      (start $init)
      (func $hook (param i32) (result i64) (i64.const 0))
      (export "hook" (func $hook)))
    "#;
    let err = rshooks_build::validate(&wasm(src), &opts()).unwrap_err();
    assert!(err.to_string().to_lowercase().contains("start"), "{err}");
}

#[test]
fn validator_rejects_multiple_memories() {
    let src = r#"
    (module
      (memory 1)
      (memory 1)
      (func $hook (param i32) (result i64) (i64.const 0))
      (export "hook" (func $hook)))
    "#;
    let err = rshooks_build::validate(&wasm(src), &opts()).unwrap_err();
    assert!(err.to_string().to_lowercase().contains("memor"), "{err}");
}

#[test]
fn validator_rejects_oversized_module() {
    let big = "x".repeat(70_000);
    let src = format!(
        r#"(module (memory 2) (data (i32.const 0) "{big}") (func $hook (param i32) (result i64) (i64.const 0)) (export "hook" (func $hook)))"#
    );
    let m = wasm(&src);
    assert!(m.len() > 65_535);
    let err = rshooks_build::validate(&m, &opts()).unwrap_err();
    assert!(
        err.to_string().contains("65,535") || err.to_string().to_lowercase().contains("exceed"),
        "{err}"
    );
}

#[test]
fn validator_allow_oversize_downgrades_to_warning() {
    let big = "x".repeat(70_000);
    // `_g` import: R1 (`docs/DESIGN.md` §6.2b/§6.4, see
    // `MINIMAL_HOOK_ALREADY_CLEAN` above) — `validate()` is called directly
    // here too, so the fixture must satisfy R1 itself.
    let src = format!(
        r#"(module (import "env" "_g" (func $g (param i32 i32) (result i32))) (memory 2) (data (i32.const 0) "{big}") (func $hook (param i32) (result i64) (i64.const 0)) (export "hook" (func $hook)))"#
    );
    let m = wasm(&src);
    let o = Options {
        allow_oversize: true,
        ..Options::default()
    };
    let report =
        rshooks_build::validate(&m, &o).expect("oversize should be a warning, not an error");
    assert!(report.oversize_allowed);
    assert!(report.warnings.iter().any(|w| w.contains("INVALID")));
}

// End-to-end: clean + check is idempotent

#[test]
fn end_to_end_clean_and_check_is_idempotent() {
    let src = r#"
    (module
      (import "env" "_g" (func $g (param i32 i32) (result i32)))
      (import "env" "accept" (func $accept (param i32 i32 i64) (result i64)))
      (memory 1)
      (export "memory" (memory 0))
      (func $hook (param i32) (result i64)
        (local $i i32)
        (loop $l
          (call $g (i32.const 1) (i32.const 10))
          drop
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br_if $l (i32.lt_u (local.get $i) (i32.const 10))))
        (call $accept (i32.const 0) (i32.const 0) (i64.const 0)))
      (export "hook" (func $hook)))
    "#;
    let input = wasm(src);

    let cleaned_once = rshooks_build::clean(&input, &opts()).expect("clean succeeds");
    rshooks_build::validate(&cleaned_once, &opts()).expect("cleaned output should validate");

    let cleaned_twice = rshooks_build::clean(&cleaned_once, &opts()).expect("re-clean succeeds");
    assert_eq!(
        cleaned_once, cleaned_twice,
        "cleaning an already-clean module must be a no-op"
    );
}

#[test]
fn run_pipeline_reports_fee_relevant_size() {
    // `MINIMAL_HOOK` has no loop and never calls `_g`, so it doesn't survive
    // the pipeline's final gate: the vendored upstream guard checker
    // (`docs/DESIGN.md` §6.5) unconditionally requires every API-version-0
    // module to import `_g`, regardless of loops. `GUARDED_LOOP_HOOK` calls
    // `_g` in a properly guarded loop, so it clears that gate and exercises
    // the fee-reporting behavior this test is actually about.
    let (out, report) =
        rshooks_build::run_pipeline(&wasm(GUARDED_LOOP_HOOK), &opts()).expect("pipeline succeeds");
    assert!(!out.is_empty());
    assert!(report.warnings.is_empty() || report.warnings.iter().all(|w| !w.contains("INVALID")));
    assert!(
        report.guard_verdict.is_some(),
        "api-version-0 success should carry the native checker's instruction counts"
    );
    let fee = rshooks_build::estimate_fee(out.len());
    assert_eq!(fee.drops, fee.bytes * 5000);
}
