//! [`Backend`]: the [`rshooks_core::backend::HostBackend`] implementation
//! over one [`crate::TestEnv`]'s [`World`] plus the current invocation's
//! [`InvocationContext`] — the Phase-1 coverage list from
//! `.claude/design/TESTENV_DESIGN.md` §6. Everything outside that list
//! keeps the trait's default `NOT_IMPLEMENTED` body.

use std::cell::RefCell;
use std::rc::Rc;
use std::vec::Vec;

use rshooks_core::backend::HostBackend;
use sha2::{Digest, Sha256};

use crate::details::{EmitDetailsInputs, build_etxn_details};
use crate::exit::{ExitType, HookExit, HookExitSignal};
use crate::invocation::InvocationContext;
use crate::world::{EmitFailureReason, EmittedTxn, World, normalize_state_key};

/// The mock host for exactly one [`crate::TestEnv::invoke`] call: reads and
/// writes through to the shared, persistent [`World`], while every
/// invocation-scoped limit lives in [`InvocationContext`].
pub(crate) struct Backend {
    world: Rc<RefCell<World>>,
    ctx: Rc<RefCell<InvocationContext>>,
}

impl Backend {
    pub(crate) fn new(world: Rc<RefCell<World>>, ctx: Rc<RefCell<InvocationContext>>) -> Self {
        Self { world, ctx }
    }

    /// `otxn_burden` × `reserved_count` (design §4's `etxn_burden` fan-out
    /// formula), overflow → `FEE_TOO_LARGE`.
    fn compute_etxn_burden(&self, reserved: u32) -> Result<u64, i64> {
        let otxn_burden = {
            let w = self.world.borrow();
            match w.otxn_emitted {
                Some((b, _)) => b,
                None => 1,
            }
        };
        otxn_burden
            .checked_mul(u64::from(reserved))
            .ok_or(rshooks_core::FEE_TOO_LARGE)
    }

    /// `otxn_generation + 1` (design §4's `etxn_generation` derivation).
    fn compute_etxn_generation(&self) -> i64 {
        let w = self.world.borrow();
        let g = match w.otxn_emitted {
            Some((_, g)) => g,
            None => 0,
        };
        i64::from(g).saturating_add(1)
    }
}

/// A deterministic, test-only stand-in for a real transaction hash — SHA-256
/// of the emitted blob. Never meant to match a real ledger's hash algorithm
/// (that is e2e-only territory); only used so distinct emitted blobs get
/// distinct, reproducible hashes.
fn deterministic_hash(blob: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(blob);
    hasher.finalize().into()
}

impl HostBackend for Backend {
    fn state(&self, key: &[u8]) -> Result<Vec<u8>, i64> {
        let norm = normalize_state_key(key)?;
        let w = self.world.borrow();
        let addr = (w.hook_account, w.own_namespace, norm);
        w.state
            .get(&addr)
            .cloned()
            .ok_or(rshooks_core::DOESNT_EXIST)
    }

    fn state_set(&self, key: &[u8], data: &[u8]) -> Result<i64, i64> {
        let norm = normalize_state_key(key)?;
        let (hook_account, own_ns, max_len) = {
            let w = self.world.borrow();
            (w.hook_account, w.own_namespace, w.max_state_value_len)
        };
        if data.len() > max_len {
            return Err(rshooks_core::TOO_BIG);
        }
        let addr_ns = (hook_account, own_ns);
        {
            let ctx = self.ctx.borrow();
            ctx.check_state_modification_budget()?;
            ctx.check_namespace_budget(&addr_ns)?;
        }
        {
            let mut w = self.world.borrow_mut();
            let key_addr = (hook_account, own_ns, norm);
            if data.is_empty() {
                w.state.remove(&key_addr);
            } else {
                w.state.insert(key_addr, data.to_vec());
            }
        }
        self.ctx.borrow_mut().record_state_modification(addr_ns);
        Ok(data.len() as i64)
    }

    fn state_foreign(
        &self,
        key: &[u8],
        ns: Option<&[u8; 32]>,
        acc: Option<&[u8; 20]>,
    ) -> Result<Vec<u8>, i64> {
        let norm = normalize_state_key(key)?;
        let w = self.world.borrow();
        let target_acc = acc.copied().unwrap_or(w.hook_account);
        let target_ns = ns.copied().unwrap_or(w.own_namespace);
        let addr = (target_acc, target_ns, norm);
        w.state
            .get(&addr)
            .cloned()
            .ok_or(rshooks_core::DOESNT_EXIST)
    }

    fn state_foreign_set(
        &self,
        key: &[u8],
        data: &[u8],
        ns: Option<&[u8; 32]>,
        acc: Option<&[u8; 20]>,
    ) -> Result<i64, i64> {
        let norm = normalize_state_key(key)?;
        let (hook_account, own_ns, max_len, current_hook_hash) = {
            let w = self.world.borrow();
            (
                w.hook_account,
                w.own_namespace,
                w.max_state_value_len,
                w.current_hook_hash(),
            )
        };
        let target_acc = acc.copied().unwrap_or(hook_account);
        let target_ns = ns.copied().unwrap_or(own_ns);
        let is_foreign = target_acc != hook_account;

        if is_foreign {
            if self.ctx.borrow().retry_blocked() {
                return Err(rshooks_core::PREVIOUS_FAILURE_PREVENTS_RETRY);
            }
            let authorized = {
                let w = self.world.borrow();
                w.grants.get(&(target_acc, target_ns)).is_some_and(|list| {
                    list.iter()
                        .any(|g| g.matches(current_hook_hash, hook_account))
                })
            };
            if !authorized {
                self.ctx.borrow_mut().set_retry_blocked();
                return Err(rshooks_core::NOT_AUTHORIZED);
            }
        }

        if data.len() > max_len {
            return Err(rshooks_core::TOO_BIG);
        }
        let addr_ns = (target_acc, target_ns);
        {
            let ctx = self.ctx.borrow();
            ctx.check_state_modification_budget()?;
            ctx.check_namespace_budget(&addr_ns)?;
        }
        {
            let mut w = self.world.borrow_mut();
            let key_addr = (target_acc, target_ns, norm);
            if data.is_empty() {
                w.state.remove(&key_addr);
            } else {
                w.state.insert(key_addr, data.to_vec());
            }
        }
        self.ctx.borrow_mut().record_state_modification(addr_ns);
        Ok(data.len() as i64)
    }

    fn otxn_field(&self, field_id: u32) -> Result<Vec<u8>, i64> {
        self.world
            .borrow()
            .otxn
            .fields
            .get(&field_id)
            .cloned()
            .ok_or(rshooks_core::DOESNT_EXIST)
    }

    fn otxn_type(&self) -> i64 {
        i64::from(self.world.borrow().otxn.tx_type.code())
    }

    fn otxn_id(&self, _flags: u32) -> Result<Vec<u8>, i64> {
        Ok(self.world.borrow().otxn.id.to_vec())
    }

    fn otxn_param(&self, name: &[u8]) -> Result<Vec<u8>, i64> {
        self.world
            .borrow()
            .otxn
            .params
            .get(name)
            .cloned()
            .ok_or(rshooks_core::DOESNT_EXIST)
    }

    #[allow(clippy::expect_used)] // documented API: `TestEnv::otxn_emitted` validates burden fits in i64 before it ever reaches `World`
    fn otxn_burden(&self) -> i64 {
        match self.world.borrow().otxn_emitted {
            Some((b, _)) => i64::try_from(b).expect(
                "rshooks_testenv::Backend::otxn_burden: TestEnv::otxn_emitted validates \
                 burden fits in i64",
            ),
            None => 1,
        }
    }

    fn otxn_generation(&self) -> i64 {
        match self.world.borrow().otxn_emitted {
            Some((_, g)) => i64::from(g),
            None => 0,
        }
    }

    fn hook_param(&self, name: &[u8]) -> Result<Vec<u8>, i64> {
        self.world
            .borrow()
            .hook_params
            .get(name)
            .cloned()
            .ok_or(rshooks_core::DOESNT_EXIST)
    }

    fn hook_account(&self) -> Result<[u8; 20], i64> {
        Ok(self.world.borrow().hook_account)
    }

    fn hook_hash(&self, hook_no: i32) -> Result<[u8; 32], i64> {
        self.world
            .borrow()
            .hook_hashes
            .get(&hook_no)
            .copied()
            .ok_or(rshooks_core::DOESNT_EXIST)
    }

    fn hook_pos(&self) -> i64 {
        i64::from(self.world.borrow().hook_pos)
    }

    fn ledger_seq(&self) -> i64 {
        i64::from(self.world.borrow().ledger_seq)
    }

    fn ledger_last_time(&self) -> i64 {
        self.world.borrow().ledger_time
    }

    fn ledger_last_hash(&self) -> Result<[u8; 32], i64> {
        // Not independently seedable in Phase 1 (design §2.4 has no
        // builder for it) — a fixed, documented zero value.
        Ok([0u8; 32])
    }

    fn ledger_nonce(&self) -> Result<[u8; 32], i64> {
        self.ctx.borrow_mut().next_nonce()
    }

    #[allow(clippy::expect_used)] // documented API: `TestEnv::base_fee_drops` validates drops fits in i64 before it ever reaches `World`
    fn fee_base(&self) -> i64 {
        i64::try_from(self.world.borrow().base_fee_drops).expect(
            "rshooks_testenv::Backend::fee_base: TestEnv::base_fee_drops validates drops \
             fits in i64",
        )
    }

    fn etxn_reserve(&self, count: u32) -> i64 {
        match self.ctx.borrow_mut().reserve(count) {
            Ok(v) => i64::from(v),
            Err(e) => e,
        }
    }

    fn etxn_fee_base(&self, _tx_blob: &[u8]) -> i64 {
        let reserved = match self.ctx.borrow().require_reserved() {
            Ok(r) => r,
            Err(e) => return e,
        };
        let burden = match self.compute_etxn_burden(reserved) {
            Ok(b) => b,
            Err(e) => return e,
        };
        let base = self.world.borrow().base_fee_drops;
        match base.checked_mul(burden).and_then(|v| i64::try_from(v).ok()) {
            Some(v) => v,
            None => rshooks_core::FEE_TOO_LARGE,
        }
    }

    fn etxn_details(&self) -> Result<Vec<u8>, i64> {
        let reserved = self.ctx.borrow().require_reserved()?;
        let burden = self.compute_etxn_burden(reserved)?;
        let generation = self.compute_etxn_generation();
        let generation = u32::try_from(generation).map_err(|_| rshooks_core::FEE_TOO_LARGE)?;
        let (parent_txn_id, hook_hash) = {
            let w = self.world.borrow();
            (w.otxn.id, w.current_hook_hash().unwrap_or([0u8; 32]))
        };
        let nonce = self.ctx.borrow_mut().next_details_nonce();
        // cbak flows are Phase 2 (design §6) — this harness never
        // populates `EmitCallback`.
        let details = build_etxn_details(&EmitDetailsInputs {
            generation,
            burden,
            parent_txn_id,
            nonce,
            hook_hash,
            callback: None,
        });
        self.ctx.borrow_mut().last_etxn_details = Some(details.clone());
        Ok(details)
    }

    fn etxn_burden(&self) -> i64 {
        let reserved = match self.ctx.borrow().require_reserved() {
            Ok(r) => r,
            Err(e) => return e,
        };
        match self.compute_etxn_burden(reserved) {
            Ok(b) => i64::try_from(b).unwrap_or(rshooks_core::FEE_TOO_LARGE),
            Err(e) => e,
        }
    }

    fn etxn_generation(&self) -> i64 {
        self.compute_etxn_generation()
    }

    fn etxn_nonce(&self) -> Result<[u8; 32], i64> {
        self.ctx.borrow_mut().next_nonce()
    }

    fn emit(&self, tx_blob: &[u8]) -> Result<[u8; 32], i64> {
        // Each `RefCell` read below is taken into an owned value on its own
        // statement, never as a `match`/`if` scrutinee — a scrutinee's
        // temporary `Ref` is kept alive for the whole `match`/`if`
        // expression (Rust's temporary-scope rule), which would still be
        // borrowed when an arm below needs `borrow_mut()`.
        let require_result = self.ctx.borrow().require_reserved();
        let reserved = match require_result {
            Ok(r) => r,
            Err(e) => {
                self.ctx
                    .borrow_mut()
                    .record_emit_failure(tx_blob.to_vec(), EmitFailureReason::NoReserve);
                return Err(e);
            }
        };
        let emit_count = self.ctx.borrow().emit_count();
        if emit_count >= reserved {
            self.ctx
                .borrow_mut()
                .record_emit_failure(tx_blob.to_vec(), EmitFailureReason::ReserveExceeded);
            return Err(rshooks_core::TOO_MANY_EMITTED_TXN);
        }

        let expected = self.ctx.borrow().last_etxn_details.clone();
        match crate::emit_walk::validate_emit_blob(tx_blob, expected.as_deref()) {
            Ok(()) => {
                let hash = deterministic_hash(tx_blob);
                self.ctx.borrow_mut().record_emitted(EmittedTxn {
                    blob: tx_blob.to_vec(),
                    hash,
                });
                Ok(hash)
            }
            Err(()) => {
                self.ctx
                    .borrow_mut()
                    .record_emit_failure(tx_blob.to_vec(), EmitFailureReason::InvalidBlob);
                Err(rshooks_core::EMISSION_FAILURE)
            }
        }
    }

    #[allow(clippy::panic)] // documented API: this is the accept! exit mechanism itself (design §2.2), not an error path
    fn accept(&self, msg: &[u8], code: i64) -> ! {
        std::panic::panic_any(HookExitSignal(HookExit {
            exit: ExitType::Accept,
            code,
            msg: msg.to_vec(),
        }))
    }

    #[allow(clippy::panic)] // documented API: this is the rollback! exit mechanism itself (design §2.2), not an error path
    fn rollback(&self, msg: &[u8], code: i64) -> ! {
        std::panic::panic_any(HookExitSignal(HookExit {
            exit: ExitType::Rollback,
            code,
            msg: msg.to_vec(),
        }))
    }

    fn trace(&self, msg: &[u8], data: &[u8], _as_hex: bool) -> i64 {
        self.world
            .borrow_mut()
            .traces
            .push(crate::world::TraceLine {
                message: msg.to_vec(),
                data: data.to_vec(),
            });
        0
    }

    fn trace_num(&self, msg: &[u8], num: i64) -> i64 {
        self.world
            .borrow_mut()
            .traces
            .push(crate::world::TraceLine {
                message: msg.to_vec(),
                data: num.to_be_bytes().to_vec(),
            });
        0
    }

    fn static_take_allowed(&self, cell_addr: usize) -> bool {
        self.ctx.borrow_mut().static_take_allowed(cell_addr)
    }

    // -- float_* (P2-B — `.claude/design/TESTENV_PHASE2_DESIGN.md` §4
    // "float_*"). Pure functions, no `self` access needed; delegated to
    // `crate::host::float` per this file's module doc comment.

    fn float_set(&self, exponent: i32, mantissa: i64) -> i64 {
        crate::host::float::float_set(exponent, mantissa)
    }

    fn float_multiply(&self, f1: i64, f2: i64) -> i64 {
        crate::host::float::float_multiply(f1, f2)
    }

    fn float_mulratio(&self, f1: i64, round_up: u32, numerator: u32, denominator: u32) -> i64 {
        crate::host::float::float_mulratio(f1, round_up, numerator, denominator)
    }

    fn float_negate(&self, f1: i64) -> i64 {
        crate::host::float::float_negate(f1)
    }

    fn float_compare(&self, f1: i64, f2: i64, mode: u32) -> i64 {
        crate::host::float::float_compare(f1, f2, mode)
    }

    fn float_sum(&self, f1: i64, f2: i64) -> i64 {
        crate::host::float::float_sum(f1, f2)
    }

    fn float_sto(
        &self,
        currency: Option<&[u8]>,
        issuer: Option<&[u8]>,
        amount: i64,
        field_code: u32,
    ) -> Result<Vec<u8>, i64> {
        crate::host::float::float_sto(currency, issuer, amount, field_code)
    }

    fn float_sto_set(&self, sto: &[u8]) -> i64 {
        crate::host::float::float_sto_set(sto)
    }

    fn float_invert(&self, f1: i64) -> i64 {
        crate::host::float::float_invert(f1)
    }

    fn float_divide(&self, f1: i64, f2: i64) -> i64 {
        crate::host::float::float_divide(f1, f2)
    }

    fn float_one(&self) -> i64 {
        crate::host::float::float_one()
    }

    fn float_mantissa(&self, f1: i64) -> i64 {
        crate::host::float::float_mantissa(f1)
    }

    fn float_sign(&self, f1: i64) -> i64 {
        crate::host::float::float_sign(f1)
    }

    fn float_int(&self, f1: i64, decimal_places: u32, absolute: u32) -> i64 {
        crate::host::float::float_int(f1, decimal_places, absolute)
    }

    fn float_log(&self, f1: i64) -> i64 {
        crate::host::float::float_log(f1)
    }

    fn float_root(&self, f1: i64, n: u32) -> i64 {
        crate::host::float::float_root(f1, n)
    }

    // -- util_* (P2-B — `.claude/design/TESTENV_PHASE2_DESIGN.md` §4
    // "util_* + ledger_keylet"). Pure functions, no `self` access needed;
    // delegated to `crate::host::util`/`crate::host::keylet` per this
    // file's module doc comment.

    fn util_sha512h(&self, data: &[u8]) -> Result<[u8; 32], i64> {
        Ok(crate::host::util::util_sha512h(data))
    }

    fn util_accid(&self, r_address: &[u8]) -> Result<Vec<u8>, i64> {
        crate::host::util::util_accid(r_address)
    }

    fn util_raddr(&self, accid: &[u8]) -> Result<Vec<u8>, i64> {
        crate::host::util::util_raddr(accid)
    }

    fn util_verify(&self, data: &[u8], signature: &[u8], public_key: &[u8]) -> i64 {
        crate::host::util::util_verify(data, signature, public_key)
    }

    fn util_keylet(
        &self,
        keylet_type: u32,
        args: [rshooks_core::backend::KeyletArg<'_>; 6],
    ) -> Result<[u8; 34], i64> {
        crate::host::keylet::util_keylet(keylet_type, args)
    }

    // `ledger_keylet` needs `World` access (a seeded ledger-object search),
    // unlike every other P2-C function above — see `host::util`'s module
    // doc comment for why it stays here rather than in a `host::` submodule
    // (mirrors the state family's own `self.world.borrow()` pattern).
    // Search rule: the smallest seeded 34-byte keylet whose 32-byte key is
    // strictly greater than `low`'s and less-than-or-equal-to `high`'s (the
    // half-open-below/closed-above range `(low, high]`) — ported from
    // `HookAPI::ledger_keylet` (`Xahau/xahaud`, branch `dev`,
    // `src/xrpld/app/hook/detail/HookAPI.cpp`), which calls
    // `view().succ(klLo.key, klHi.key.next())`: `View::succ` finds the
    // smallest key strictly greater than its first argument that is less
    // than its (exclusive) second argument, and passing `klHi.key.next()`
    // (the immediate successor of `high`) as that exclusive bound makes the
    // overall range inclusive of `high` itself. This port compares directly
    // against `high` (`<=`) instead of reproducing `.next()`'s wraparound
    // arithmetic on an all-`0xFF` key — an intentional simplification for
    // an edge case unreachable in practice (a real keylet hash landing on
    // the maximum possible 256-bit value), noted here per this stage's
    // "state the assumption" policy for details not pinned by a source (1-3
    // list in the design doc). `low`/`high` must be well-formed 34-byte
    // keylets and share the same 2-byte type prefix (`DOES_NOT_MATCH`
    // otherwise, matching `klLo.type != klHi.type`); the output keylet's
    // type prefix is always taken from `low` (`Keylet kl_out{klLo.type,
    // *found}` — never looked up from the found object itself, since
    // `succ()` searches the *global* key space with no type filter).
    fn ledger_keylet(&self, low: &[u8], high: &[u8]) -> Result<Vec<u8>, i64> {
        if low.len() != 34 || high.len() != 34 {
            return Err(rshooks_core::INVALID_ARGUMENT);
        }
        let (lo_type, lo_key) = low.split_at(2);
        let (hi_type, hi_key) = high.split_at(2);
        if lo_type != hi_type {
            return Err(rshooks_core::DOES_NOT_MATCH);
        }
        let w = self.world.borrow();
        let found = w
            .ledger_objects
            .keys()
            .map(|k| &k[2..34])
            .filter(|k| *k > lo_key && *k <= hi_key)
            .min();
        match found {
            Some(key) => {
                let mut out = Vec::with_capacity(34);
                out.extend_from_slice(lo_type);
                out.extend_from_slice(key);
                Ok(out)
            }
            None => Err(rshooks_core::DOESNT_EXIST),
        }
    }

    // `trace_float` needs `World` access (captures into `World::traces`),
    // so — like `trace`/`trace_num` above — it lands directly here rather
    // than in a `host::` submodule; `host::control`'s module doc comment
    // notes the same for this function specifically. Stores the message
    // verbatim and the raw XFL `i64` bit pattern as its big-endian 8-byte
    // encoding, mirroring `trace_num`'s own convention exactly (a full
    // xahaud-faithful "Float mantissa*10^(exponent)" text rendering is not
    // reproduced — design doc §4 marks that as not required, only that the
    // raw value itself is captured for a test to inspect/decode).
    fn trace_float(&self, msg: &[u8], value: i64) -> i64 {
        self.world.borrow_mut().traces.push(crate::world::TraceLine {
            message: msg.to_vec(),
            data: value.to_be_bytes().to_vec(),
        });
        0
    }
}

/// Boundary tests for the checked `u64` → `i64` conversions this file
/// applies to protocol values (design review FIX 7): values guaranteed
/// valid by [`crate::TestEnv`]'s now-validating builders (`otxn_emitted`,
/// `base_fee_drops`) pass through unchanged at the `i64::MAX` boundary, and
/// a burden/reserve product that still overflows past `i64::MAX` (despite
/// fitting in `u64`) reports `FEE_TOO_LARGE` rather than wrapping. White-box
/// (constructs `Backend` directly over a fresh `World`/`InvocationContext`)
/// so `reserved` can be pinned precisely, independent of any one `#[hooks]`
/// chain's hardcoded `etxn_reserve` call.
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    use super::*;

    fn fresh() -> (Rc<RefCell<World>>, Rc<RefCell<InvocationContext>>, Backend) {
        let world = Rc::new(RefCell::new(World::new()));
        let ctx = Rc::new(RefCell::new(InvocationContext::new(0)));
        let backend = Backend::new(Rc::clone(&world), Rc::clone(&ctx));
        (world, ctx, backend)
    }

    // -- ledger_keylet (P2-C: `(low, high]` range search over
    // `World::ledger_objects`) --

    fn kl(ty: u16, key_byte: u8) -> [u8; 34] {
        let mut out = [0u8; 34];
        out[0..2].copy_from_slice(&ty.to_be_bytes());
        out[33] = key_byte;
        out
    }

    #[test]
    fn ledger_keylet_returns_smallest_key_in_range() {
        let (world, _ctx, backend) = fresh();
        {
            let mut w = world.borrow_mut();
            w.ledger_objects.insert(kl(0x64, 5), std::vec![]);
            w.ledger_objects.insert(kl(0x64, 9), std::vec![]);
            w.ledger_objects.insert(kl(0x64, 20), std::vec![]);
        }
        let low = kl(0x64, 3);
        let high = kl(0x64, 10);
        assert_eq!(backend.ledger_keylet(&low, &high), Ok(kl(0x64, 5).to_vec()));
    }

    #[test]
    fn ledger_keylet_lower_bound_is_exclusive() {
        let (world, _ctx, backend) = fresh();
        world.borrow_mut().ledger_objects.insert(kl(0x64, 5), std::vec![]);
        let low = kl(0x64, 5); // the seeded entry sits exactly at `low`
        let high = kl(0x64, 10);
        assert_eq!(
            backend.ledger_keylet(&low, &high),
            Err(rshooks_core::DOESNT_EXIST)
        );
    }

    #[test]
    fn ledger_keylet_upper_bound_is_inclusive() {
        let (world, _ctx, backend) = fresh();
        world.borrow_mut().ledger_objects.insert(kl(0x64, 10), std::vec![]);
        let low = kl(0x64, 5);
        let high = kl(0x64, 10); // the seeded entry sits exactly at `high`
        assert_eq!(
            backend.ledger_keylet(&low, &high),
            Ok(kl(0x64, 10).to_vec())
        );
    }

    #[test]
    fn ledger_keylet_empty_range_is_doesnt_exist() {
        let (_world, _ctx, backend) = fresh();
        let low = kl(0x64, 0);
        let high = kl(0x64, 0xFF);
        assert_eq!(
            backend.ledger_keylet(&low, &high),
            Err(rshooks_core::DOESNT_EXIST)
        );
    }

    #[test]
    fn ledger_keylet_type_mismatch_is_does_not_match() {
        let (_world, _ctx, backend) = fresh();
        let low = kl(0x64, 0);
        let high = kl(0x53, 0xFF); // different ltXXX type prefix
        assert_eq!(
            backend.ledger_keylet(&low, &high),
            Err(rshooks_core::DOES_NOT_MATCH)
        );
    }

    #[test]
    fn ledger_keylet_wrong_length_is_invalid_argument() {
        let (_world, _ctx, backend) = fresh();
        assert_eq!(
            backend.ledger_keylet(&[0u8; 10], &[0u8; 34]),
            Err(rshooks_core::INVALID_ARGUMENT)
        );
    }

    // -- trace_float (P2-C) --

    #[test]
    fn trace_float_captures_message_and_raw_bits() {
        let (world, _ctx, backend) = fresh();
        let bits: i64 = 6_089_866_696_204_910_592; // XFL!(1) — see host::float's FLOAT_ONE
        assert_eq!(backend.trace_float(b"Amount", bits), 0);
        let traces = world.borrow().traces.clone();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].message, b"Amount".to_vec());
        assert_eq!(traces[0].data, bits.to_be_bytes().to_vec());
    }

    #[test]
    fn otxn_burden_at_i64_max_passes_through() {
        let (world, _ctx, backend) = fresh();
        world.borrow_mut().otxn_emitted = Some((i64::MAX as u64, 0));
        assert_eq!(backend.otxn_burden(), i64::MAX);
    }

    #[test]
    fn fee_base_at_i64_max_passes_through() {
        let (world, _ctx, backend) = fresh();
        world.borrow_mut().base_fee_drops = i64::MAX as u64;
        assert_eq!(backend.fee_base(), i64::MAX);
    }

    #[test]
    fn etxn_burden_exact_i64_max_passes_through() {
        let (world, ctx, backend) = fresh();
        world.borrow_mut().otxn_emitted = Some((i64::MAX as u64, 0));
        ctx.borrow_mut().reserve(1).unwrap();
        assert_eq!(backend.etxn_burden(), i64::MAX);
    }

    #[test]
    fn etxn_burden_product_past_i64_max_is_fee_too_large() {
        // `i64::MAX * 2` fits in `u64` (so the `checked_mul` inside
        // `compute_etxn_burden` never fires), but exceeds `i64::MAX` — the
        // case the added `i64::try_from` cast exists to catch.
        let (world, ctx, backend) = fresh();
        world.borrow_mut().otxn_emitted = Some((i64::MAX as u64, 0));
        ctx.borrow_mut().reserve(2).unwrap();
        assert_eq!(backend.etxn_burden(), rshooks_core::FEE_TOO_LARGE);
    }

    #[test]
    fn etxn_fee_base_product_past_i64_max_is_fee_too_large() {
        let (world, ctx, backend) = fresh();
        {
            let mut w = world.borrow_mut();
            w.otxn_emitted = Some((i64::MAX as u64, 0));
            w.base_fee_drops = 2;
        }
        ctx.borrow_mut().reserve(1).unwrap();
        assert_eq!(backend.etxn_fee_base(&[]), rshooks_core::FEE_TOO_LARGE);
    }
}
