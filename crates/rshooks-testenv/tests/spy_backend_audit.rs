//! Spy-backend audit (design §2.1): installs a call-counting
//! [`HostBackend`] and drives every public `rshooks` API in the bridged
//! families (state, otxn, hook_ctx, ledger, control, etxn, trace), then
//! asserts each one reached the backend.
//!
//! This is independent of `rshooks-testenv`'s own [`rshooks_testenv::TestEnv`] —
//! `HostBackend` is `pub` (only `#[doc(hidden)]`) precisely so a downstream
//! crate can implement it directly, which this test exercises.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use rshooks::prelude::*;
use rshooks::static_cell::HookStatic;
use rshooks_core::backend::{HostBackend, install};
// `rshooks::prelude::*` re-exports `rshooks::error::Result` (fixed
// `HookError` error type); `HostBackend`'s own methods need plain
// `core::result::Result<T, i64>` — this explicit import wins over the glob.
use std::result::Result;

#[derive(Default)]
struct SpyBackend {
    hit: RefCell<BTreeSet<&'static str>>,
}

impl SpyBackend {
    fn mark(&self, name: &'static str) {
        self.hit.borrow_mut().insert(name);
    }
}

impl HostBackend for SpyBackend {
    fn state(&self, _key: &[u8]) -> Result<Vec<u8>, i64> {
        self.mark("state");
        Ok(Vec::new())
    }
    fn state_set(&self, _key: &[u8], _data: &[u8]) -> Result<i64, i64> {
        self.mark("state_set");
        Ok(0)
    }
    fn state_foreign(
        &self,
        _key: &[u8],
        _ns: Option<&[u8; 32]>,
        _acc: Option<&[u8; 20]>,
    ) -> Result<Vec<u8>, i64> {
        self.mark("state_foreign");
        Ok(Vec::new())
    }
    fn state_foreign_set(
        &self,
        _key: &[u8],
        _data: &[u8],
        _ns: Option<&[u8; 32]>,
        _acc: Option<&[u8; 20]>,
    ) -> Result<i64, i64> {
        self.mark("state_foreign_set");
        Ok(0)
    }
    fn otxn_field(&self, _field_id: u32) -> Result<Vec<u8>, i64> {
        self.mark("otxn_field");
        Ok(Vec::new())
    }
    fn otxn_type(&self) -> i64 {
        self.mark("otxn_type");
        0
    }
    fn otxn_id(&self, _flags: u32) -> Result<Vec<u8>, i64> {
        self.mark("otxn_id");
        Ok(vec![0u8; 32])
    }
    fn otxn_param(&self, _name: &[u8]) -> Result<Vec<u8>, i64> {
        self.mark("otxn_param");
        Ok(Vec::new())
    }
    fn otxn_burden(&self) -> i64 {
        self.mark("otxn_burden");
        1
    }
    fn otxn_generation(&self) -> i64 {
        self.mark("otxn_generation");
        0
    }
    fn hook_param(&self, _name: &[u8]) -> Result<Vec<u8>, i64> {
        self.mark("hook_param");
        Ok(Vec::new())
    }
    fn hook_account(&self) -> Result<[u8; 20], i64> {
        self.mark("hook_account");
        Ok([0u8; 20])
    }
    fn hook_hash(&self, _hook_no: i32) -> Result<[u8; 32], i64> {
        self.mark("hook_hash");
        Ok([0u8; 32])
    }
    fn hook_pos(&self) -> i64 {
        self.mark("hook_pos");
        0
    }
    fn ledger_seq(&self) -> i64 {
        self.mark("ledger_seq");
        1
    }
    fn ledger_last_time(&self) -> i64 {
        self.mark("ledger_last_time");
        0
    }
    fn ledger_last_hash(&self) -> Result<[u8; 32], i64> {
        self.mark("ledger_last_hash");
        Ok([0u8; 32])
    }
    fn ledger_nonce(&self) -> Result<[u8; 32], i64> {
        self.mark("ledger_nonce");
        Ok([0u8; 32])
    }
    fn fee_base(&self) -> i64 {
        self.mark("fee_base");
        10
    }
    fn etxn_reserve(&self, count: u32) -> i64 {
        self.mark("etxn_reserve");
        i64::from(count)
    }
    fn etxn_fee_base(&self, _tx_blob: &[u8]) -> i64 {
        self.mark("etxn_fee_base");
        10
    }
    fn etxn_details(&self) -> Result<Vec<u8>, i64> {
        self.mark("etxn_details");
        Ok(Vec::new())
    }
    fn etxn_burden(&self) -> i64 {
        self.mark("etxn_burden");
        1
    }
    fn etxn_generation(&self) -> i64 {
        self.mark("etxn_generation");
        1
    }
    fn etxn_nonce(&self) -> Result<[u8; 32], i64> {
        self.mark("etxn_nonce");
        Ok([0u8; 32])
    }
    fn emit(&self, _tx_blob: &[u8]) -> Result<[u8; 32], i64> {
        self.mark("emit");
        Ok([0u8; 32])
    }
    fn accept(&self, _msg: &[u8], _code: i64) -> ! {
        self.mark("accept");
        panic!("SpyBackend::accept");
    }
    fn rollback(&self, _msg: &[u8], _code: i64) -> ! {
        self.mark("rollback");
        panic!("SpyBackend::rollback");
    }
    fn trace(&self, _msg: &[u8], _data: &[u8], _as_hex: bool) -> i64 {
        self.mark("trace");
        0
    }
    fn trace_num(&self, _msg: &[u8], _num: i64) -> i64 {
        self.mark("trace_num");
        0
    }
    fn static_take_allowed(&self, _cell_addr: usize) -> bool {
        self.mark("static_take_allowed");
        true
    }
}

static SCRATCH: HookStatic<[u8; 4]> = HookStatic::new([0, 0, 0, 0]);

#[test]
fn every_phase1_backend_method_is_reached() {
    let spy = Rc::new(SpyBackend::default());
    let guard = install(Rc::clone(&spy) as Rc<dyn HostBackend>);

    let mut buf32 = [0u8; 32];
    let mut buf20 = [0u8; 20];
    let ns = [0u8; 32];
    let acc = [0u8; 20];

    let _ = state(&mut buf32, &[0u8; 32]);
    let _ = state_set(&[1, 2, 3], &[0u8; 32]);
    let _ = rshooks::api::state::state_foreign(&mut buf32, &[0u8; 32], &ns, &acc);
    let _ = rshooks::api::state::state_foreign_set(&[1, 2], &[0u8; 32], &ns, &acc);
    let _ = otxn_field(&mut buf32, 0u32);
    let _ = otxn_type();
    let _ = otxn_id(&mut buf32, 0);
    let _ = otxn_param(&mut buf32, b"x");
    let _ = otxn_burden();
    let _ = otxn_generation();
    let _ = hook_param(&mut buf32, b"x");
    let _ = hook_account(&mut buf20);
    let _ = hook_hash(&mut buf32, 0);
    let _ = hook_pos();
    let _ = ledger_seq();
    let _ = ledger_last_time();
    let _ = ledger_last_hash(&mut buf32);
    let _ = ledger_nonce(&mut buf32);
    let _ = fee_base();
    let _ = etxn_reserve(1);
    let _ = etxn_fee_base(&[0u8; 4]);
    let mut ed_buf = [0u8; EMIT_DETAILS_MAX_LEN];
    let _ = etxn_details(&mut ed_buf);
    let _ = etxn_burden();
    let _ = etxn_generation();
    let mut nonce_buf = [0u8; 32];
    let _ = etxn_nonce(&mut nonce_buf);
    let mut emit_out = [0u8; 32];
    let _ = emit(&mut emit_out, &[0u8; 4]);
    let _ = trace(b"m", b"d", false);
    let _ = trace_num(b"m", 1);
    let _ = catch_unwind(AssertUnwindSafe(|| accept(b"", 0)));
    let _ = catch_unwind(AssertUnwindSafe(|| rollback(b"", 0)));
    let _ = SCRATCH.take();

    drop(guard);

    let hit = spy.hit.borrow();
    let expected: &[&str] = &[
        "state",
        "state_set",
        "state_foreign",
        "state_foreign_set",
        "otxn_field",
        "otxn_type",
        "otxn_id",
        "otxn_param",
        "otxn_burden",
        "otxn_generation",
        "hook_param",
        "hook_account",
        "hook_hash",
        "hook_pos",
        "ledger_seq",
        "ledger_last_time",
        "ledger_last_hash",
        "ledger_nonce",
        "fee_base",
        "etxn_reserve",
        "etxn_fee_base",
        "etxn_details",
        "etxn_burden",
        "etxn_generation",
        "etxn_nonce",
        "emit",
        "accept",
        "rollback",
        "trace",
        "trace_num",
        "static_take_allowed",
    ];
    for name in expected {
        assert!(
            hit.contains(name),
            "backend method `{name}` was never reached"
        );
    }
}
