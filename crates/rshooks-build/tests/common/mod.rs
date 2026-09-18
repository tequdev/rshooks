//! Shared test-only helpers: raw custom-section encoding, plus the `wasmi`
//! differential-test harness used by the flatten/unnest pass tests
//! (`docs/DESIGN.md` §6.2b/§6.2c). Pulled in via `mod common;` from the
//! integration tests, and via `#[path]` from `optimizer`'s in-crate unit
//! tests; holds no `#[test]`s of its own.
//!
//! Test code is exempt from the workspace's panic-freedom lints (per
//! `docs/DESIGN.md` §8).
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::cell::RefCell;
use std::rc::Rc;

use wasmi::{Caller, Engine, Linker, Module, Store};

// ---------------------------------------------------------------------
// Raw custom-section encoding
// ---------------------------------------------------------------------

pub fn write_leb128(mut n: u64, out: &mut Vec<u8>) {
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

/// Appends a raw custom section (id 0) to an existing wasm binary.
pub fn append_custom_section(wasm: &[u8], name: &str, payload: &[u8]) -> Vec<u8> {
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

/// Encodes a `target_features` custom-section payload (a feature count
/// followed by one `(prefix, name)` entry per feature, each declared
/// required with `+`) — the format clang emits for any non-`mvp` target
/// CPU.
pub fn target_features_payload(features: &[&str]) -> Vec<u8> {
    let mut payload = Vec::new();
    write_leb128(features.len() as u64, &mut payload);
    for feature in features {
        payload.push(b'+');
        write_leb128(feature.len() as u64, &mut payload);
        payload.extend_from_slice(feature.as_bytes());
    }
    payload
}

// ---------------------------------------------------------------------
// wasmi differential-test harness (flatten/unnest)
// ---------------------------------------------------------------------

/// Host state: the shared `env::obs` call-observation log.
pub struct HostState {
    pub log: Rc<RefCell<Vec<(i32, i32)>>>,
}

/// Instantiates `wasm` against the standard `env::obs` host stub — logging
/// every call as an `(a, b)` pair and answering with the pure,
/// deterministic `a.wrapping_mul(1000).wrapping_add(b)` — and, if
/// `define_g` is set, a stubbed `env::_g` guard import too. Calls the named
/// export (`(i32) -> i64`) with `param` and returns its outcome (`Ok` on a
/// normal return, `Err` on a trap) plus the full call-observation log
/// recorded up to that point.
pub fn run(
    wasm: &[u8],
    export: &str,
    param: i32,
    define_g: bool,
) -> (Result<i64, wasmi::Error>, Vec<(i32, i32)>) {
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
    if define_g {
        linker
            .func_wrap(
                "env",
                "_g",
                |_caller: Caller<'_, HostState>, _a: i32, _b: i32| -> i32 { 1 },
            )
            .expect("define env::_g");
    }
    let instance = linker
        .instantiate(&mut store, &module)
        .expect("all imports satisfied")
        .start(&mut store)
        .expect("no start function to run, or it succeeds");
    let entry = instance
        .get_typed_func::<i32, i64>(&store, export)
        .unwrap_or_else(|_| panic!("`{export}` export with signature (i32) -> i64"));
    let result = entry.call(&mut store, param);
    let calls = log.borrow().clone();
    (result, calls)
}
