//! [`InvocationContext`]: everything scoped to exactly one
//! [`crate::TestEnv::invoke`] call — the design §4 limits table, in code.
//! Nothing here survives past the invocation it was created for; the
//! world-visible effects of a successful invocation (state writes,
//! committed emissions) are applied by [`crate::env::TestEnv::invoke`]
//! itself once the outcome is known.

use std::collections::HashSet;
use std::vec::Vec;

use crate::world::{EmitAttempt, EmittedTxn};

/// `etxn_reserve`'s upper bound (design §4: `n > 255` → `TOO_BIG`).
const MAX_RESERVE: u32 = 255;
/// `max_state_modifications` (vendored `Enum.h:397`).
const MAX_STATE_MODIFICATIONS: u32 = 256;
/// `maxNamespaces()` (vendored `Enum.h`).
const MAX_NAMESPACES: usize = 256;
/// One past `max_nonce` (vendored `Enum.h:399`, `max_nonce = 255`): the
/// 256th nonce call this invocation (`nonce_count == 256`) is refused.
const MAX_NONCES: u32 = 256;
// `max_slots` (xahau `Enum.h`) = 255: numbered slots `1..=255` — index `0`
// of `InvocationContext::slots` is never assigned (slot number `0` means
// "auto-assign" to the Hook API, not a real slot). No named constant yet:
// nothing enforces this budget until `slot_*` semantics land in P2-D.

/// The kind of content held in a numbered slot (design §3, "slot family").
/// Scaffolding only as of P2-A — the real leaf-type taxonomy
/// (STObject/STArray/scalar/...) lands with `slot_*` semantics in P2-D;
/// nothing constructs a [`SlotEntry`] yet, so only a placeholder variant
/// exists so the type is nameable now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // scaffolding (P2-A): a real variant is constructed once slot_* semantics land (P2-D)
pub(crate) enum SlotType {
    /// Placeholder — no `slot_*` semantics populate a real variant yet.
    Unknown,
}

/// One numbered slot's content: the serialized field/object bytes the
/// numbered slot APIs (`slot`, `slot_subfield`, `slot_subarray`, ...) read
/// (design §3). Scaffolding only as of P2-A — see [`SlotType`]'s doc
/// comment.
#[derive(Debug, Clone)]
#[allow(dead_code)] // scaffolding (P2-A): fields are read once slot_* semantics land (P2-D)
pub(crate) struct SlotEntry {
    pub(crate) bytes: Vec<u8>,
    pub(crate) leaf_type: SlotType,
}

/// Per-invocation state the design §4 table specifies. See each field's
/// accompanying method for the exact default/increment/error semantics.
pub(crate) struct InvocationContext {
    /// Identifies this invocation for nonce derivation
    /// (`H(invocation_counter ‖ call_counter)`).
    pub(crate) invocation_id: u64,
    call_counter: u64,
    reserve: Option<u32>,
    emit_count: u32,
    state_mod_count: u32,
    namespaces_touched: HashSet<([u8; 20], [u8; 32])>,
    nonce_count: u32,
    retry_blocked: bool,
    static_take_set: HashSet<usize>,
    /// Bytes returned by the most recent `etxn_details()` call this
    /// invocation — the emission walker requires an `EmitDetails` field to
    /// match these exactly (design §5.6).
    pub(crate) last_etxn_details: Option<Vec<u8>>,
    /// Validated emissions from this invocation, pending commit into the
    /// world on `accept!` (discarded on `rollback!`/unknown panic).
    pub(crate) pending_emissions: Vec<EmittedTxn>,
    /// Every `emit` attempt this invocation made, successful or not —
    /// always recorded into the world regardless of how this invocation
    /// exits (design §2.4: `emit_attempts()` is cumulative and includes
    /// rolled-back invocations).
    pub(crate) emit_attempts: Vec<EmitAttempt>,

    // -- Phase 2 (`.claude/design/TESTENV_PHASE2_DESIGN.md` §3) --
    //
    // Plain data plumbing as of P2-A — see `world.rs`'s matching Phase 2
    // block for why nothing here is populated or read yet.
    /// Numbered slots `1..=255` (index `0` unused — slot number `0` means
    /// "auto-assign" to the Hook API, not a real slot; xahau's `max_slots`
    /// is 255). Invocation-scoped: a fresh, all-`None` array every `invoke` call,
    /// matching a fresh wasm instance on-chain (design §3). Boxed so a
    /// 256-element array of `Option<SlotEntry>` doesn't bloat every
    /// `InvocationContext` (most invocations use zero or a handful of
    /// slots).
    #[allow(dead_code)] // scaffolding (P2-A): read/written once slot_* semantics land (P2-D)
    pub(crate) slots: std::boxed::Box<[Option<SlotEntry>; 256]>,
    /// Whether `hook_again` was called this invocation (design §4:
    /// `ALREADY_SET` on a second call within the same invocation).
    #[allow(dead_code)] // scaffolding (P2-A): read/written once hook_again lands (P2-E)
    pub(crate) hook_again_called: bool,
    /// `hook_skip` directives recorded this invocation, to be moved onto
    /// [`crate::world::World::skip_directives`] once P2-E implements the
    /// commit rule (design §4 records them "verbatim"; exact commit-on-exit
    /// semantics are not yet pinned as of this scaffolding).
    #[allow(dead_code)] // scaffolding (P2-A): read/written once hook_skip lands (P2-E)
    pub(crate) skip_directives: Vec<([u8; 32], u32)>,
}

impl InvocationContext {
    pub(crate) fn new(invocation_id: u64) -> Self {
        Self {
            invocation_id,
            call_counter: 0,
            reserve: None,
            emit_count: 0,
            state_mod_count: 0,
            namespaces_touched: HashSet::new(),
            nonce_count: 0,
            retry_blocked: false,
            static_take_set: HashSet::new(),
            last_etxn_details: None,
            pending_emissions: Vec::new(),
            emit_attempts: Vec::new(),
            slots: std::boxed::Box::new(core::array::from_fn(|_| None)),
            hook_again_called: false,
            skip_directives: Vec::new(),
        }
    }

    /// `etxn_reserve(count)`: `0` → `TOO_SMALL`; `> 255` → `TOO_BIG`; a
    /// second call → `ALREADY_SET`.
    pub(crate) fn reserve(&mut self, count: u32) -> Result<u32, i64> {
        if self.reserve.is_some() {
            return Err(rshooks_core::ALREADY_SET);
        }
        if count == 0 {
            return Err(rshooks_core::TOO_SMALL);
        }
        if count > MAX_RESERVE {
            return Err(rshooks_core::TOO_BIG);
        }
        self.reserve = Some(count);
        Ok(count)
    }

    /// Requires a prior successful [`Self::reserve`] call — backs `emit`,
    /// `etxn_fee_base`, `etxn_details`, `etxn_burden` (design §4:
    /// `PREREQUISITE_NOT_MET` otherwise).
    pub(crate) fn require_reserved(&self) -> Result<u32, i64> {
        self.reserve.ok_or(rshooks_core::PREREQUISITE_NOT_MET)
    }

    /// Successful emissions recorded so far this invocation — compared
    /// against the reserved count *before* blob validation (design §4:
    /// exceeding the reserved count is `TOO_MANY_EMITTED_TXN` regardless of
    /// blob shape).
    pub(crate) fn emit_count(&self) -> u32 {
        self.emit_count
    }

    /// Records one successful emission (only a blob that passed the
    /// emission walker reaches this — see design §4: the emit counter
    /// "increments on each emit attempt that passes blob validation").
    pub(crate) fn record_emitted(&mut self, txn: EmittedTxn) {
        self.emit_count = self.emit_count.saturating_add(1);
        self.emit_attempts.push(EmitAttempt {
            blob: txn.blob.clone(),
            outcome: Ok(()),
        });
        self.pending_emissions.push(txn);
    }

    /// Records a rejected `emit` attempt (never counted against the
    /// reserve, never committed).
    pub(crate) fn record_emit_failure(
        &mut self,
        blob: Vec<u8>,
        reason: crate::world::EmitFailureReason,
    ) {
        self.emit_attempts.push(EmitAttempt {
            blob,
            outcome: Err(reason),
        });
    }

    /// Checks (without mutating) whether one more state write is within
    /// `max_state_modifications`. Design §4: "a write arriving when the
    /// counter is already `>= max` is refused ... and neither counts nor
    /// changes state" — callers must call this *before* performing the
    /// write, and only call [`Self::record_state_modification`] once the
    /// write itself has actually happened.
    pub(crate) fn check_state_modification_budget(&self) -> Result<(), i64> {
        if self.state_mod_count >= MAX_STATE_MODIFICATIONS {
            return Err(rshooks_core::TOO_MANY_STATE_MODIFICATIONS);
        }
        Ok(())
    }

    /// Records one successful modifying write (set or delete), and — if
    /// `addr` (the written `(account, namespace)`) is new this invocation —
    /// counts it against the namespace budget. Design §4: `> 256` distinct
    /// namespaces → `TOO_MANY_NAMESPACES`. Callers must have already
    /// checked [`Self::check_namespace_budget`] for a genuinely new `addr`
    /// before performing the write, mirroring
    /// [`Self::check_state_modification_budget`]'s contract.
    pub(crate) fn record_state_modification(&mut self, addr: ([u8; 20], [u8; 32])) {
        self.state_mod_count = self.state_mod_count.saturating_add(1);
        self.namespaces_touched.insert(addr);
    }

    /// Checks (without mutating) whether writing into a *new* `addr` is
    /// within the namespace budget. Returns `Ok(())` unconditionally if
    /// `addr` was already touched this invocation (re-writing an existing
    /// namespace never costs budget).
    pub(crate) fn check_namespace_budget(&self, addr: &([u8; 20], [u8; 32])) -> Result<(), i64> {
        if self.namespaces_touched.contains(addr) {
            return Ok(());
        }
        if self.namespaces_touched.len() >= MAX_NAMESPACES {
            return Err(rshooks_core::TOO_MANY_NAMESPACES);
        }
        Ok(())
    }

    /// Draws the next deterministic nonce (design §4: `H(invocation_counter
    /// ‖ call_counter)`, shared by `etxn_nonce`/`ledger_nonce`). `> 256`
    /// nonce calls this invocation → `TOO_MANY_NONCES`.
    pub(crate) fn next_nonce(&mut self) -> Result<[u8; 32], i64> {
        if self.nonce_count >= MAX_NONCES {
            return Err(rshooks_core::TOO_MANY_NONCES);
        }
        let call = self.call_counter;
        self.nonce_count = self.nonce_count.saturating_add(1);
        self.call_counter = self.call_counter.saturating_add(1);
        Ok(hash_counters(self.invocation_id, call))
    }

    /// A nonce for a purpose that must not consume the shared
    /// `etxn_nonce`/`ledger_nonce` budget above — used for `EmitDetails`'s
    /// `EmitNonce` field, which xahaud derives independently (see
    /// `crates/rshooks-testenv/src/details.rs`'s module doc comment for why
    /// this is a documented simplification rather than a pinned protocol
    /// derivation).
    pub(crate) fn next_details_nonce(&mut self) -> [u8; 32] {
        let call = self.call_counter;
        self.call_counter = self.call_counter.saturating_add(1);
        hash_counters(self.invocation_id, call.wrapping_add(1u64 << 32))
    }

    /// Whether a foreign write is blocked by an earlier authorization
    /// failure this invocation (design §5.5/§4:
    /// `PREVIOUS_FAILURE_PREVENTS_RETRY`).
    pub(crate) fn retry_blocked(&self) -> bool {
        self.retry_blocked
    }

    /// Marks the invocation-local retry-block flag. Only ever called for an
    /// authorization failure — never a size/limit failure (design §5.5).
    pub(crate) fn set_retry_blocked(&mut self) {
        self.retry_blocked = true;
    }

    /// `HookStatic`'s per-invocation take-set: `true` (and records the
    /// claim) the first time `cell_addr` is asked for this invocation;
    /// `false` on every later ask.
    pub(crate) fn static_take_allowed(&mut self, cell_addr: usize) -> bool {
        self.static_take_set.insert(cell_addr)
    }
}

/// Deterministic per-invocation nonce derivation: SHA-256 of the two
/// counters' big-endian bytes, taken directly as the 32-byte nonce.
fn hash_counters(invocation_id: u64, call_counter: u64) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(invocation_id.to_be_bytes());
    hasher.update(call_counter.to_be_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    use super::*;

    #[test]
    fn reserve_rejects_zero_and_too_large_and_double_set() {
        let mut ctx = InvocationContext::new(0);
        assert_eq!(ctx.reserve(0), Err(rshooks_core::TOO_SMALL));
        assert_eq!(ctx.reserve(256), Err(rshooks_core::TOO_BIG));
        assert_eq!(ctx.reserve(1), Ok(1));
        assert_eq!(ctx.reserve(2), Err(rshooks_core::ALREADY_SET));
    }

    #[test]
    fn require_reserved_is_prerequisite_not_met_until_set() {
        let mut ctx = InvocationContext::new(0);
        assert_eq!(
            ctx.require_reserved(),
            Err(rshooks_core::PREREQUISITE_NOT_MET)
        );
        ctx.reserve(1).expect("valid reserve");
        assert_eq!(ctx.require_reserved(), Ok(1));
    }

    #[test]
    fn emit_count_tracks_only_successful_emissions() {
        let mut ctx = InvocationContext::new(0);
        assert_eq!(ctx.emit_count(), 0);
        ctx.reserve(1).expect("valid reserve");
        ctx.record_emitted(EmittedTxn {
            blob: Vec::new(),
            hash: [0u8; 32],
        });
        assert_eq!(ctx.emit_count(), 1);
    }

    #[test]
    fn state_modification_budget_rejects_at_the_boundary() {
        let mut ctx = InvocationContext::new(0);
        for _ in 0..256 {
            assert_eq!(ctx.check_state_modification_budget(), Ok(()));
            ctx.record_state_modification(([0u8; 20], [0u8; 32]));
        }
        assert_eq!(
            ctx.check_state_modification_budget(),
            Err(rshooks_core::TOO_MANY_STATE_MODIFICATIONS)
        );
    }

    #[test]
    fn namespace_budget_only_costs_for_genuinely_new_namespaces() {
        let mut ctx = InvocationContext::new(0);
        let addr = ([1u8; 20], [2u8; 32]);
        for _ in 0..1000 {
            assert_eq!(ctx.check_namespace_budget(&addr), Ok(()));
            ctx.record_state_modification(addr);
        }
        // Still only one distinct namespace touched.
        assert_eq!(ctx.namespaces_touched.len(), 1);
    }

    #[test]
    fn namespace_budget_rejects_the_257th_distinct_namespace() {
        let mut ctx = InvocationContext::new(0);
        for i in 0..256u16 {
            let mut ns = [0u8; 32];
            ns[0] = (i >> 8) as u8;
            ns[1] = (i & 0xFF) as u8;
            let addr = ([0u8; 20], ns);
            assert_eq!(ctx.check_namespace_budget(&addr), Ok(()));
            ctx.record_state_modification(addr);
        }
        let overflow_addr = ([0u8; 20], [0xFFu8; 32]);
        assert_eq!(
            ctx.check_namespace_budget(&overflow_addr),
            Err(rshooks_core::TOO_MANY_NAMESPACES)
        );
    }

    #[test]
    fn nonce_budget_rejects_at_256() {
        let mut ctx = InvocationContext::new(0);
        for _ in 0..256 {
            assert!(ctx.next_nonce().is_ok());
        }
        assert_eq!(ctx.next_nonce(), Err(rshooks_core::TOO_MANY_NONCES));
    }

    #[test]
    fn nonces_are_deterministic_given_the_same_counters() {
        let a = hash_counters(1, 2);
        let b = hash_counters(1, 2);
        let c = hash_counters(1, 3);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn retry_block_flag_starts_clear_and_can_be_set() {
        let mut ctx = InvocationContext::new(0);
        assert!(!ctx.retry_blocked());
        ctx.set_retry_blocked();
        assert!(ctx.retry_blocked());
    }

    #[test]
    fn static_take_allowed_is_take_once_per_invocation() {
        let mut ctx = InvocationContext::new(0);
        assert!(ctx.static_take_allowed(0x1000));
        assert!(!ctx.static_take_allowed(0x1000));
        assert!(ctx.static_take_allowed(0x2000));
    }
}
