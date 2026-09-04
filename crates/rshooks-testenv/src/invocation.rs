//! [`InvocationContext`]: everything scoped to exactly one
//! [`crate::TestEnv::invoke`] call — the design §4 limits table, in code.
//! Nothing here survives past the invocation it was created for; the
//! world-visible effects of a successful invocation (state writes,
//! committed emissions) are applied by [`crate::env::TestEnv::invoke`]
//! itself once the outcome is known.

use std::collections::HashMap;
use std::collections::HashSet;
use std::vec::Vec;

use crate::world::{EmitAttempt, EmittedTxn};

/// `hook_api::max_params` (vendored `Enum.h:401`): `hook_param_set`'s
/// per-invocation override-count budget (`TOO_MANY_PARAMS` past this).
/// `pub(crate)`: `crate::host::control::hook_param_set` is the one caller.
pub(crate) const MAX_PARAM_OVERRIDES: u32 = 16;

/// `etxn_reserve`'s upper bound (design §4: `n > 255` → `TOO_BIG`).
const MAX_RESERVE: u32 = 255;
/// `max_state_modifications` (vendored `Enum.h:397`).
const MAX_STATE_MODIFICATIONS: u32 = 256;
/// `maxNamespaces()` (vendored `Enum.h`).
const MAX_NAMESPACES: usize = 256;
/// One past `max_nonce` (vendored `Enum.h:399`, `max_nonce = 255`): the
/// 256th call to a given nonce family this invocation is refused. xahaud
/// tracks `ledger_nonce`'s and the emit-nonce family's (`etxn_nonce`,
/// `etxn_details`) counters separately (`HookAPI.cpp`'s
/// `hookCtx.ledger_nonce_counter` vs. `hookCtx.emit_nonce_counter`, each
/// checked against `max_nonce` independently), so each family gets its own
/// 256-call budget rather than sharing one.
const MAX_NONCES: u32 = 256;
// `max_slots` (xahau `Enum.h`) = 255: numbered slots `1..=255` — index `0`
// of `InvocationContext::slots` is never assigned (slot number `0` means
// "auto-assign" to the Hook API, not a real slot). No named constant: the
// `1..=255` range is enforced directly in `resolve_slot`
// (`NO_FREE_SLOTS`/`INVALID_ARGUMENT`).

/// The kind of content held in a numbered slot (design §3/§4 "slot
/// family", landed P2-D). Governs which `slot_*` operations a slot accepts:
/// `slot_subfield` needs [`Root`](SlotKind::Root)/[`Object`](SlotKind::Object);
/// `slot_subarray`/`slot_count` need [`Array`](SlotKind::Array);
/// `slot_float`/`slot_type(no, 1)` need [`Amount`](SlotKind::Amount).
/// `slot`/`slot_size`/`slot_type(no, 0)`/`slot_clear` work on every kind.
///
/// Mirrors xahaud's own distinction between the STBase entry a slot points
/// at (whose runtime `getSType()` decides `slot_type`/`slot_count`/
/// `slot_subarray`/`slot_subfield`'s behavior — see `HookAPI.cpp`'s
/// implementations, and `crate::host::slots`' module doc) and a synthesized
/// root/field code for `slot_type(no, 0)`, tracked as
/// [`SlotEntry::reported_code`] rather than recomputed from the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotKind {
    /// A whole transaction / ledger object / metadata / XPOP half — a bare
    /// canonical field sequence with no wrapping header or terminator
    /// (`otxn_slot`/`meta_slot`/`xpop_slot`/`slot_set`).
    Root,
    /// A nested `STObject` field's value, as produced by `slot_subfield`
    /// (or an array element, via `slot_subarray`): field-sequence bytes
    /// ending in the `0xE1` object-end marker (included in
    /// [`SlotEntry::bytes`]).
    Object,
    /// A nested `STArray` field's value, as produced by `slot_subfield`:
    /// element bytes ending in the `0xF1` array-end marker (included in
    /// [`SlotEntry::bytes`]).
    Array,
    /// A serialized `Amount` value (8 native / 48 IOU bytes), no header.
    Amount,
    /// Any other field's raw value bytes (VL length-prefix stripped for
    /// VL/AccountID fields — see `crate::emit_walk::field_value_payload`),
    /// no header.
    Scalar,
}

/// One numbered slot's content: the serialized field/object bytes the
/// numbered slot APIs (`slot`, `slot_subfield`, `slot_subarray`, ...) read
/// (design §3), an owned, independent copy. Deriving a child slot via
/// `slot_subfield`/`slot_subarray` copies bytes out of the parent rather
/// than aliasing it, so clearing the parent afterward cannot affect a
/// child — the read contract `examples/15_slot-objects`' `check_parent_clear`
/// pins, and `rshooks::slot_path!`'s doc comment relies on: "the host
/// copies the parent's storage into the child slot".
#[derive(Debug, Clone)]
pub(crate) struct SlotEntry {
    pub(crate) bytes: Vec<u8>,
    pub(crate) kind: SlotKind,
    /// The value `slot_type(no, 0)` reports: an ordinary field code
    /// (`(sti << 16) | field`, matching `rshooks::sfield::sfXxx.code()`'s
    /// encoding) for anything from `slot_subfield`/`slot_subarray`, or a
    /// synthesized root object code for a root slot — `crate::host::slots`'
    /// `ROOT_TRANSACTION`/`ROOT_LEDGER_ENTRY`/`ROOT_METADATA` constants,
    /// whose high 16 bits are rippled's real `STI_TRANSACTION`(10001)/
    /// `STI_LEDGERENTRY`(10002)/`STI_METADATA`(10004) (`XRPLF/rippled`,
    /// `include/xrpl/protocol/SField.h`), matching `rshooks::slot_obj`'s
    /// `ROOT_OBJECT_CODE_MIN..=ROOT_OBJECT_CODE_MAX` (10001..=10004)
    /// contract `examples/15_slot-objects`' `check_root_cast` pins.
    pub(crate) reported_code: u32,
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
    /// `ledger_nonce`'s own budget counter (`hookCtx.ledger_nonce_counter`
    /// in xahaud), independent of [`Self::emit_nonce_count`].
    ledger_nonce_count: u32,
    /// The emit-nonce family's budget counter, shared by `etxn_nonce` and
    /// `etxn_details` (xahaud's `hookCtx.emit_nonce_counter`:
    /// `HookAPI::etxn_details` calls `HookAPI::etxn_nonce()` directly, so
    /// both draw from the same counter and budget).
    emit_nonce_count: u32,
    retry_blocked: bool,
    static_take_set: HashSet<usize>,
    /// Whether the entry this invocation is running declares a `#[cbak]`
    /// body — set once by `crate::env::TestEnv::run_entry` before the
    /// backend runs, mirroring `hookDef->isFieldPresent(sfHookCallbackFee)`
    /// (`Xahau/xahaud` `dev`, `src/xrpld/app/tx/detail/Transactor.cpp:1408`),
    /// the on-chain source of `HookAPI::etxn_details`'s own
    /// `hookCtx.result.hasCallback` check (`HookAPI.cpp:914`) for whether
    /// `EmitDetails` gets an `EmitCallback` field.
    pub(crate) has_callback: bool,
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
    // Populated and read by `crate::host::slots`'s `slot_*` functions via
    // `resolve_slot`/`set_slot`/`clear_slot` below.
    /// Numbered slots `1..=255` (index `0` unused — slot `0` means
    /// "auto-assign", not a real slot; xahau's `max_slots` is 255).
    /// Invocation-scoped: a fresh, all-`None` array every `invoke` call,
    /// matching a fresh wasm instance on-chain (design §3). Boxed so a
    /// 256-element `Option<SlotEntry>` array doesn't bloat every
    /// `InvocationContext` (most invocations use zero or a handful of
    /// slots).
    pub(crate) slots: std::boxed::Box<[Option<SlotEntry>; 256]>,
    /// Whether `hook_again` was called this invocation (design §4:
    /// `ALREADY_SET` on a second call in the same invocation). Merged into
    /// [`crate::world::World::hook_again_requested`] by
    /// `crate::env::TestEnv::run_entry` only on `accept!` (P2-E — same
    /// stage-then-merge pattern as `pending_emissions`/`committed_emissions`,
    /// not a live world write).
    pub(crate) hook_again_called: bool,
    /// `hook_skip` directives recorded this invocation, in call order,
    /// **only the ones that succeeded** (upstream never records a rejected
    /// call — `HookAPI::hook_skip`, `Xahau/xahaud` `dev`,
    /// `src/xrpld/app/hook/detail/HookAPI.cpp:1745-1782`, only mutates
    /// `hookCtx.result.hookSkips` on success). Extended onto
    /// [`crate::world::World::skip_directives`] on `accept!` (P2-E, same
    /// stage-then-merge pattern as `hook_again_called`).
    pub(crate) skip_directives: Vec<([u8; 32], u32)>,
    /// This invocation's own `hook_param_set` writes — `(hook_hash, name)
    /// -> value` — merged into
    /// [`crate::world::World::hook_param_overrides`] on `accept!` (P2-E;
    /// design §4 "control leftovers": "committed only on accept, like
    /// state"). A `HashMap`, not a `Vec`: later calls overwrite earlier
    /// ones for the same `(hash, name)` key, matching upstream's
    /// `overrides[hash][name] = value` assignment (`HookAPI.cpp:1713-1744`)
    /// — only the final value per key survives to merge.
    pub(crate) pending_param_overrides: HashMap<([u8; 32], Vec<u8>), Vec<u8>>,
    /// `hook_param_set`'s own per-invocation call budget (`overrideCount`,
    /// `HookAPI.cpp:1727-1728`) — counts every *call* (not distinct keys),
    /// fresh each invocation (upstream: fresh per `HookResult`).
    pub(crate) param_override_count: u32,
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
            ledger_nonce_count: 0,
            emit_nonce_count: 0,
            retry_blocked: false,
            static_take_set: HashSet::new(),
            has_callback: false,
            last_etxn_details: None,
            pending_emissions: Vec::new(),
            emit_attempts: Vec::new(),
            slots: std::boxed::Box::new(core::array::from_fn(|_| None)),
            hook_again_called: false,
            skip_directives: Vec::new(),
            pending_param_overrides: HashMap::new(),
            param_override_count: 0,
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

    /// Draws the next deterministic `ledger_nonce` (design §4:
    /// `H(invocation_counter ‖ call_counter)`). `> 256` `ledger_nonce` calls
    /// this invocation → `TOO_MANY_NONCES`; independent of
    /// [`Self::next_emit_nonce`]'s budget.
    pub(crate) fn next_ledger_nonce(&mut self) -> Result<[u8; 32], i64> {
        if self.ledger_nonce_count >= MAX_NONCES {
            return Err(rshooks_core::TOO_MANY_NONCES);
        }
        let call = self.call_counter;
        self.ledger_nonce_count = self.ledger_nonce_count.saturating_add(1);
        self.call_counter = self.call_counter.saturating_add(1);
        Ok(hash_counters(self.invocation_id, call))
    }

    /// Draws the next deterministic emit nonce, shared by `etxn_nonce` and
    /// `etxn_details` (xahaud's `HookAPI::etxn_details` calls
    /// `HookAPI::etxn_nonce()` directly, so both draw from the same
    /// `emit_nonce_counter`/budget). `> 256` combined `etxn_nonce`/
    /// `etxn_details` calls this invocation → `TOO_MANY_NONCES`; independent
    /// of [`Self::next_ledger_nonce`]'s budget.
    pub(crate) fn next_emit_nonce(&mut self) -> Result<[u8; 32], i64> {
        if self.emit_nonce_count >= MAX_NONCES {
            return Err(rshooks_core::TOO_MANY_NONCES);
        }
        let call = self.call_counter;
        self.emit_nonce_count = self.emit_nonce_count.saturating_add(1);
        self.call_counter = self.call_counter.saturating_add(1);
        Ok(hash_counters(self.invocation_id, call))
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

    /// Reads slot `no`, if occupied (`None` for `0`, an out-of-range
    /// number, or an empty slot alike — callers map to `DOESNT_EXIST`).
    pub(crate) fn slot_entry(&self, no: u32) -> Option<&SlotEntry> {
        self.slots.get(no as usize).and_then(Option::as_ref)
    }

    /// Occupies slot `no` with `entry`. A no-op if `no` is out of range —
    /// unreachable via any real caller, since every `host::slots` call site
    /// only passes a number [`Self::resolve_slot`] returned (`.get_mut`,
    /// not direct indexing, so panic-freedom holds without re-proving it
    /// at each write site).
    pub(crate) fn set_slot(&mut self, no: usize, entry: SlotEntry) {
        if let Some(slot) = self.slots.get_mut(no) {
            *slot = Some(entry);
        }
    }

    /// Frees slot `no` unconditionally (no `DOESNT_EXIST` reporting — that
    /// belongs to `host::slots::slot_clear`, the hook-facing entry point).
    /// Same `.get_mut`-based write as [`Self::set_slot`], for a cleanup
    /// path that doesn't care whether the slot was occupied.
    pub(crate) fn clear_slot(&mut self, no: usize) {
        if let Some(slot) = self.slots.get_mut(no) {
            *slot = None;
        }
    }

    /// Resolves a `slot_into`/`new_slot` argument to a concrete slot
    /// number: the design's own simplification of xahaud's FIFO-then-
    /// monotonic-counter allocator (`.claude/design/TESTENV_PHASE2_DESIGN.md`
    /// §4 "slot family"; upstream's `HookAPI::get_free_slot` — a freed-slot
    /// FIFO queue falling back to a never-rewinding counter that
    /// permanently exhausts past `max_slots` — is ported nowhere in this
    /// harness): `0` picks the
    /// lowest-numbered free slot in `1..=255` (`NO_FREE_SLOTS` if none are
    /// free); an explicit `1..=255` is returned as-is, silently overwriting
    /// whatever previously occupied it (matching xahaud's own unconditional
    /// `hookCtx.slot[new_slot] = ...` assignment); anything `> 255` is
    /// `INVALID_ARGUMENT`.
    pub(crate) fn resolve_slot(&self, requested: u32) -> Result<usize, i64> {
        if requested == 0 {
            for i in 1..=255usize {
                if self.slots.get(i).is_some_and(Option::is_none) {
                    return Ok(i);
                }
            }
            return Err(rshooks_core::NO_FREE_SLOTS);
        }
        if requested > 255 {
            return Err(rshooks_core::INVALID_ARGUMENT);
        }
        Ok(requested as usize)
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
    fn ledger_nonce_budget_rejects_at_256() {
        let mut ctx = InvocationContext::new(0);
        for _ in 0..256 {
            assert!(ctx.next_ledger_nonce().is_ok());
        }
        assert_eq!(ctx.next_ledger_nonce(), Err(rshooks_core::TOO_MANY_NONCES));
    }

    #[test]
    fn emit_nonce_budget_rejects_at_256() {
        let mut ctx = InvocationContext::new(0);
        for _ in 0..256 {
            assert!(ctx.next_emit_nonce().is_ok());
        }
        assert_eq!(ctx.next_emit_nonce(), Err(rshooks_core::TOO_MANY_NONCES));
    }

    #[test]
    fn ledger_and_emit_nonce_budgets_are_independent() {
        let mut ctx = InvocationContext::new(0);
        for _ in 0..256 {
            assert!(ctx.next_ledger_nonce().is_ok());
        }
        // The ledger-nonce family is exhausted, but the emit-nonce family
        // still has its own, untouched 256-call budget.
        for _ in 0..256 {
            assert!(ctx.next_emit_nonce().is_ok());
        }
        assert_eq!(ctx.next_ledger_nonce(), Err(rshooks_core::TOO_MANY_NONCES));
        assert_eq!(ctx.next_emit_nonce(), Err(rshooks_core::TOO_MANY_NONCES));
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
