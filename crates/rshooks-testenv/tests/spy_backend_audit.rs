//! Spy-backend audit (design §2.1): installs a call-counting
//! [`HostBackend`] and drives every public `rshooks` API in the bridged
//! families (state, otxn, hook_ctx, ledger, control, etxn, trace, plus the
//! Phase 2 families float, slot, sto, util, keylet), then asserts each one
//! reached the backend.
//!
//! Independent of `rshooks-testenv`'s own [`rshooks_testenv::TestEnv`] —
//! `HostBackend` is `pub` (only `#[doc(hidden)]`) so a downstream crate can
//! implement it directly, which this test exercises.

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
use rshooks_core::backend::{HostBackend, KeyletArg, install};
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

    // -- Phase 2 --

    fn float_set(&self, _exponent: i32, _mantissa: i64) -> i64 {
        self.mark("float_set");
        0
    }
    fn float_multiply(&self, _f1: i64, _f2: i64) -> i64 {
        self.mark("float_multiply");
        0
    }
    fn float_mulratio(&self, _f1: i64, _round_up: u32, _numerator: u32, _denominator: u32) -> i64 {
        self.mark("float_mulratio");
        0
    }
    fn float_negate(&self, _f1: i64) -> i64 {
        self.mark("float_negate");
        0
    }
    fn float_compare(&self, _f1: i64, _f2: i64, _mode: u32) -> i64 {
        self.mark("float_compare");
        0
    }
    fn float_sum(&self, _f1: i64, _f2: i64) -> i64 {
        self.mark("float_sum");
        0
    }
    fn float_sto(
        &self,
        _currency: Option<&[u8]>,
        _issuer: Option<&[u8]>,
        _amount: i64,
        _field_code: u32,
    ) -> Result<Vec<u8>, i64> {
        self.mark("float_sto");
        Ok(vec![0u8; 8])
    }
    fn float_sto_set(&self, _sto: &[u8]) -> i64 {
        self.mark("float_sto_set");
        0
    }
    fn float_invert(&self, _f1: i64) -> i64 {
        self.mark("float_invert");
        0
    }
    fn float_divide(&self, _f1: i64, _f2: i64) -> i64 {
        self.mark("float_divide");
        0
    }
    fn float_one(&self) -> i64 {
        self.mark("float_one");
        0
    }
    fn float_mantissa(&self, _f1: i64) -> i64 {
        self.mark("float_mantissa");
        0
    }
    fn float_sign(&self, _f1: i64) -> i64 {
        self.mark("float_sign");
        0
    }
    fn float_int(&self, _f1: i64, _decimal_places: u32, _absolute: u32) -> i64 {
        self.mark("float_int");
        0
    }
    fn float_log(&self, _f1: i64) -> i64 {
        self.mark("float_log");
        0
    }
    fn float_root(&self, _f1: i64, _n: u32) -> i64 {
        self.mark("float_root");
        0
    }
    fn slot(&self, _slot_no: u32) -> Result<Vec<u8>, i64> {
        self.mark("slot");
        Ok(Vec::new())
    }
    fn slot_clear(&self, _slot_no: u32) -> i64 {
        self.mark("slot_clear");
        0
    }
    fn slot_count(&self, _slot_no: u32) -> i64 {
        self.mark("slot_count");
        0
    }
    fn slot_float(&self, _slot_no: u32) -> i64 {
        self.mark("slot_float");
        0
    }
    fn slot_set(&self, _data: &[u8], _slot_into: u32) -> i64 {
        self.mark("slot_set");
        1
    }
    fn slot_size(&self, _slot_no: u32) -> i64 {
        self.mark("slot_size");
        0
    }
    fn slot_subarray(&self, _parent_slot: u32, _array_id: u32, _new_slot: u32) -> i64 {
        self.mark("slot_subarray");
        1
    }
    fn slot_subfield(&self, _parent_slot: u32, _field_id: u32, _new_slot: u32) -> i64 {
        self.mark("slot_subfield");
        1
    }
    fn slot_type(&self, _slot_no: u32, _flags: u32) -> i64 {
        self.mark("slot_type");
        0
    }
    fn otxn_slot(&self, _slot_into: u32) -> i64 {
        self.mark("otxn_slot");
        1
    }
    fn meta_slot(&self, _slot_into: u32) -> i64 {
        self.mark("meta_slot");
        1
    }
    fn xpop_slot(&self, _slot_no_tx: u32, _slot_no_meta: u32) -> i64 {
        self.mark("xpop_slot");
        0
    }
    fn sto_subfield(&self, _sto: &[u8], _field_id: u32) -> i64 {
        self.mark("sto_subfield");
        0
    }
    fn sto_subarray(&self, _array: &[u8], _index: u32) -> i64 {
        self.mark("sto_subarray");
        0
    }
    fn sto_validate(&self, _sto: &[u8]) -> i64 {
        self.mark("sto_validate");
        1
    }
    fn sto_emplace(&self, _source: &[u8], _field: &[u8], _field_id: u32) -> Result<Vec<u8>, i64> {
        self.mark("sto_emplace");
        Ok(Vec::new())
    }
    fn sto_erase(&self, _source: &[u8], _field_id: u32) -> Result<Vec<u8>, i64> {
        self.mark("sto_erase");
        Ok(Vec::new())
    }
    fn util_sha512h(&self, _data: &[u8]) -> Result<[u8; 32], i64> {
        self.mark("util_sha512h");
        Ok([0u8; 32])
    }
    fn util_accid(&self, _r_address: &[u8]) -> Result<Vec<u8>, i64> {
        self.mark("util_accid");
        Ok(vec![0u8; 20])
    }
    fn util_raddr(&self, _accid: &[u8]) -> Result<Vec<u8>, i64> {
        self.mark("util_raddr");
        Ok(Vec::new())
    }
    fn util_verify(&self, _data: &[u8], _signature: &[u8], _public_key: &[u8]) -> i64 {
        self.mark("util_verify");
        0
    }
    fn util_keylet(&self, _keylet_type: u32, _args: [KeyletArg<'_>; 6]) -> Result<[u8; 34], i64> {
        self.mark("util_keylet");
        Ok([0u8; 34])
    }
    fn ledger_keylet(&self, _low: &[u8], _high: &[u8]) -> Result<Vec<u8>, i64> {
        self.mark("ledger_keylet");
        Ok(vec![0u8; 34])
    }
    fn hook_again(&self) -> i64 {
        self.mark("hook_again");
        0
    }
    fn hook_skip(&self, _hash: &[u8], _flags: u32) -> i64 {
        self.mark("hook_skip");
        0
    }
    fn hook_param_set(&self, _value: &[u8], _name: &[u8], _hook_hash: &[u8]) -> i64 {
        self.mark("hook_param_set");
        0
    }
    fn prepare(&self, _template: &[u8]) -> Result<Vec<u8>, i64> {
        self.mark("prepare");
        Ok(Vec::new())
    }
    fn trace_float(&self, _msg: &[u8], _value: i64) -> i64 {
        self.mark("trace_float");
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

/// Drives every newly bridged Phase 2 public API (float, slot, sto, util,
/// keylet, plus the 7 sites that were unbridged in Phase 1 — see
/// `crates/rshooks/testenv-call-sites.txt`) and asserts each one reaches
/// the backend. Mirrors [`every_phase1_backend_method_is_reached`]'s shape.
#[test]
fn every_phase2_backend_method_is_reached() {
    use rshooks::types::{AccountId, CurrencyCode, Hash, Keylet, NameSpace, StateKey};
    use rshooks::xfl::XFL;

    let spy = Rc::new(SpyBackend::default());
    let guard = install(Rc::clone(&spy) as Rc<dyn HostBackend>);

    // -- float (api/float.rs direct sites + xfl.rs's 14 float_* sites) --
    let mut sto_out = [0u8; 48];
    let _ = rshooks::api::float::float_sto(&mut sto_out, None, None, XFL::one(), 0u32);
    let _ = rshooks::api::float::float_sto_set(&sto_out);
    let _ = rshooks::api::float::slot_float(1);
    let _ = XFL::new(0, 1);
    let _ = XFL::one();
    let one = XFL::one();
    let _ = one.invert();
    let _ = one.mulratio(false, 1, 2);
    let _ = one.mantissa();
    let _ = one.sign();
    let _ = one.to_int(0, false);
    let _ = one.compare(one, 1);
    let _ = one.log();
    let _ = one.root(2);
    let _ = -one;
    let _ = one + one;
    let _ = one * one;
    let _ = one / one;

    // -- slot --
    let mut buf32 = [0u8; 32];
    let _ = rshooks::api::slot::slot(&mut buf32, 1);
    let _ = rshooks::api::slot::slot_u64(1);
    let _ = rshooks::api::slot::slot_clear(1);
    let _ = rshooks::api::slot::slot_count(1);
    let _ = rshooks::api::slot::slot_set(&buf32, 0);
    let _ = rshooks::api::slot::slot_size(1);
    let _ = rshooks::api::slot::slot_subarray(1, 0, 0);
    let _ = rshooks::api::slot::slot_subfield(1, 0u32, 0);
    let _ = rshooks::api::slot::slot_type(1, 0);
    let _ = rshooks::api::slot::meta_slot(0);
    let _ = rshooks::api::slot::xpop_slot(1, 2);
    let _ = rshooks::api::otxn::otxn_slot(0);

    // -- sto --
    let sto = [0u8; 4];
    let mut sto_buf = [0u8; 8];
    let _ = rshooks::api::sto::sto_validate(&sto);
    let _ = rshooks::api::sto::sto_subfield(&sto, 0u32);
    let _ = rshooks::api::sto::sto_subarray(&sto, 0);
    let _ = rshooks::api::sto::sto_emplace(&mut sto_buf, &sto, &sto, 0u32);
    let _ = rshooks::api::sto::sto_erase(&mut sto_buf, &sto, 0u32);

    // -- util --
    let mut util_out = [0u8; 64];
    let _ = rshooks::api::util::util_raddr(&mut util_out, &[0u8; 20]);
    let _ = rshooks::api::util::util_accid(&mut util_out, b"raddress");
    let _ = rshooks::api::util::util_verify(b"data", b"sig", b"pubkey");
    let _ = rshooks::api::util::util_sha512h(&mut util_out, b"data");
    let mut keylet_out = [0u8; 34];
    let _ = rshooks::api::util::util_keylet(&mut keylet_out, 1, 0, 0, 0, 0, 0, 0);

    // -- keylet (all 26 typed helpers) --
    let account = AccountId::default();
    let account_b = AccountId::default();
    let hash = Hash::default();
    let key = StateKey::default();
    let namespace = NameSpace::default();
    let currency = CurrencyCode::default();
    let dir = Keylet::default();
    let _ = rshooks::api::keylet::keylet_hook(&account);
    let _ = rshooks::api::keylet::keylet_hook_state(&account, &key, &namespace);
    let _ = rshooks::api::keylet::keylet_account(&account);
    let _ = rshooks::api::keylet::keylet_amendments();
    let _ = rshooks::api::keylet::keylet_child(&hash);
    let _ = rshooks::api::keylet::keylet_skip(None);
    let _ = rshooks::api::keylet::keylet_skip(Some(1));
    let _ = rshooks::api::keylet::keylet_fees();
    let _ = rshooks::api::keylet::keylet_negative_unl();
    let _ = rshooks::api::keylet::keylet_line(&account, &account_b, &currency);
    let _ = rshooks::api::keylet::keylet_offer(&account, 1);
    let _ = rshooks::api::keylet::keylet_quality(&dir, 1, 1);
    let _ = rshooks::api::keylet::keylet_emitted_dir();
    let _ = rshooks::api::keylet::keylet_ticket(&account, 1);
    let _ = rshooks::api::keylet::keylet_signers(&account);
    let _ = rshooks::api::keylet::keylet_check(&account, 1);
    let _ = rshooks::api::keylet::keylet_deposit_preauth(&account, &account_b);
    let _ = rshooks::api::keylet::keylet_unchecked(&hash);
    let _ = rshooks::api::keylet::keylet_owner_dir(&account);
    let _ = rshooks::api::keylet::keylet_page(&hash, 1, 1);
    let _ = rshooks::api::keylet::keylet_escrow(&account, 1);
    let _ = rshooks::api::keylet::keylet_paychan(&account, &account_b, 1);
    let _ = rshooks::api::keylet::keylet_emitted(&hash);
    let _ = rshooks::api::keylet::keylet_nft_offer(&account, 1);
    let _ = rshooks::api::keylet::keylet_hook_definition(&hash);
    let _ = rshooks::api::keylet::keylet_hook_state_dir(&account, &namespace);
    let _ = rshooks::api::keylet::keylet_cron(&account, 1);

    // -- ledger_keylet --
    let _ = rshooks::api::ledger::ledger_keylet(&mut keylet_out, &[0u8; 34], &[0u8; 34]);

    // -- control leftovers / hook_param_set / prepare / trace_float --
    let _ = hook_again();
    let _ = hook_skip(&[0u8; 32], 0);
    let _ = rshooks::api::hook_ctx::hook_param_set(b"v", b"x", &[0u8; 32]);
    let mut prepare_out = [0u8; 8];
    let _ = rshooks::api::etxn::prepare(&mut prepare_out, &[0u8; 4]);
    let _ = trace_float(b"m", XFL::one());

    drop(guard);

    let hit = spy.hit.borrow();
    let expected: &[&str] = &[
        "float_set",
        "float_multiply",
        "float_mulratio",
        "float_negate",
        "float_compare",
        "float_sum",
        "float_sto",
        "float_sto_set",
        "float_invert",
        "float_divide",
        "float_one",
        "float_mantissa",
        "float_sign",
        "float_int",
        "float_log",
        "float_root",
        "slot",
        "slot_clear",
        "slot_count",
        "slot_float",
        "slot_set",
        "slot_size",
        "slot_subarray",
        "slot_subfield",
        "slot_type",
        "otxn_slot",
        "meta_slot",
        "xpop_slot",
        "sto_subfield",
        "sto_subarray",
        "sto_validate",
        "sto_emplace",
        "sto_erase",
        "util_sha512h",
        "util_accid",
        "util_raddr",
        "util_verify",
        "util_keylet",
        "ledger_keylet",
        "hook_again",
        "hook_skip",
        "hook_param_set",
        "prepare",
        "trace_float",
    ];
    for name in expected {
        assert!(
            hit.contains(name),
            "backend method `{name}` was never reached"
        );
    }
}

/// Drives `rshooks::xfl_unchecked::XFLUnchecked`'s operators specifically,
/// independent of [`every_phase2_backend_method_is_reached`]'s `XFL`
/// (checked) exercise above: `XFL`'s operators already prove
/// `float_negate`/`float_sum`/`float_multiply`/`float_divide` are
/// *reachable* through `xfl.rs`'s own bridging, but that alone would not
/// catch a regression that broke `xfl_unchecked.rs`'s separate raw call
/// sites, which could bypass any installed backend entirely.
#[test]
fn xfl_unchecked_operators_reach_the_backend() {
    use rshooks::xfl::XFL;

    let spy = Rc::new(SpyBackend::default());
    let guard = install(Rc::clone(&spy) as Rc<dyn HostBackend>);

    let one = XFL::one().unchecked();
    let _ = -one;
    let _ = one + one;
    let _ = one * one;
    let _ = one / one;
    let _ = one.validate();

    drop(guard);

    let hit = spy.hit.borrow();
    for name in [
        "float_negate",
        "float_sum",
        "float_multiply",
        "float_divide",
    ] {
        assert!(
            hit.contains(name),
            "backend method `{name}` was never reached via XFLUnchecked"
        );
    }
}
