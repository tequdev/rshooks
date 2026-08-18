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
    #![allow(clippy::unwrap_used)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    use super::*;

    fn fresh() -> (Rc<RefCell<World>>, Rc<RefCell<InvocationContext>>, Backend) {
        let world = Rc::new(RefCell::new(World::new()));
        let ctx = Rc::new(RefCell::new(InvocationContext::new(0)));
        let backend = Backend::new(Rc::clone(&world), Rc::clone(&ctx));
        (world, ctx, backend)
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
