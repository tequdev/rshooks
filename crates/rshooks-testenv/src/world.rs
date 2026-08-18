//! The persistent world model: everything that survives across
//! [`crate::TestEnv::invoke`] calls on the same [`crate::TestEnv`] (state,
//! the originating transaction, hook parameters, ledger fields, grants, and
//! every committed emission/attempt/trace so far).

use std::collections::{HashMap, HashSet};
use std::vec::Vec;

use crate::grant::Grant;
use crate::otxn::Otxn;

/// Default cap (in bytes) on a single hook-state value, matching xahaud's
/// `maxHookStateDataSize` at the default state scale of 1
/// (`crates/rshooks-build/vendor/xahaud/Enum.h`: `256U * hookStateScale`).
/// Configurable per [`crate::TestEnv`] via `max_state_value_len` — see that
/// method's doc comment.
pub(crate) const DEFAULT_MAX_STATE_VALUE_LEN: usize = 256;

/// A state entry's storage key: the entry's own account, its namespace, and
/// its 32-byte left-pad-normalized key (design §5.3 — `b"RR"` and its
/// left-padded 32-byte form address the same entry).
pub(crate) type StateAddr = ([u8; 20], [u8; 32], [u8; 32]);

/// Why an [`crate::TestEnv::invoke`]d hook's `emit` attempt did not become a
/// committed [`EmittedTxn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitFailureReason {
    /// `etxn_reserve` was never called this invocation (design §4:
    /// `PREREQUISITE_NOT_MET`).
    NoReserve,
    /// More `emit` attempts than the reserved count (design §4:
    /// `TOO_MANY_EMITTED_TXN`).
    ReserveExceeded,
    /// The blob failed the emission walker's acceptance grammar (design
    /// §5.6: `EMISSION_FAILURE`) — malformed framing, an unknown/out-of-order
    /// field, a required field missing or wrong, or an `EmitDetails` field
    /// whose bytes did not exactly match this invocation's `etxn_details()`.
    InvalidBlob,
}

/// One `emit` call this hook made, whether or not it became a committed
/// emission. Cumulative across every [`crate::TestEnv::invoke`] call on one
/// [`crate::TestEnv`] (design §2.4) — including attempts made during an
/// invocation that ultimately rolled back.
#[derive(Debug, Clone)]
pub struct EmitAttempt {
    /// The raw bytes the hook passed to `emit`.
    pub blob: Vec<u8>,
    /// `Ok(())` if the blob was accepted (it also appears in
    /// [`crate::TestEnv::emitted`]); `Err(reason)` if it was rejected.
    pub outcome: Result<(), EmitFailureReason>,
}

/// A transaction this hook successfully emitted (passed the emission
/// walker and was not later discarded by a `rollback!`/unknown panic in the
/// same invocation). Cumulative across every `invoke` call — see
/// [`crate::TestEnv::emitted`].
#[derive(Debug, Clone)]
pub struct EmittedTxn {
    /// The full, validated emitted-transaction bytes.
    pub blob: Vec<u8>,
    /// The hash `emit` returned for this transaction.
    pub hash: [u8; 32],
}

impl EmittedTxn {
    /// The raw emitted-transaction bytes.
    #[must_use]
    pub fn blob(&self) -> &[u8] {
        &self.blob
    }

    /// The emitted transaction's hash, as returned by `emit`.
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        self.hash
    }

    /// This transaction's `TransactionType` field, decoded as a
    /// [`rshooks::tx_type::TxType`] — `None` only if the blob is malformed
    /// in a way the emission walker should already have rejected (defensive;
    /// should not occur for any value reachable through
    /// [`crate::TestEnv::emitted`]).
    #[must_use]
    pub fn tx_type(&self) -> Option<rshooks::tx_type::TxType> {
        crate::emit_walk::top_level_transaction_type(&self.blob)
    }
}

/// One captured `trace`/`trace_num` call. Cumulative across every `invoke`
/// call — captured, never printed, unless a test chooses to inspect it.
#[derive(Debug, Clone)]
pub struct TraceLine {
    /// The trace message bytes.
    pub message: Vec<u8>,
    /// The trace's data payload — the raw `data` bytes for a `trace` call,
    /// or the 8-byte big-endian encoding of `number` for a `trace_num` call.
    pub data: Vec<u8>,
}

/// The persistent world: state maps, the originating transaction, hook
/// identity/params, ledger fields, grants, and every emission/trace so far.
/// Lives behind an `Rc<RefCell<..>>` in [`crate::TestEnv`] — see that
/// type's doc comment.
pub(crate) struct World {
    pub(crate) hook_account: [u8; 20],
    pub(crate) hook_hashes: HashMap<i32, [u8; 32]>,
    pub(crate) hook_pos: u32,
    pub(crate) hook_params: HashMap<Vec<u8>, Vec<u8>>,
    pub(crate) otxn: Otxn,
    /// `Some((burden, generation))` when [`crate::TestEnv::otxn_emitted`] was
    /// used to seed this env; `None` models an ordinary (non-emitted) otxn.
    pub(crate) otxn_emitted: Option<(u64, u32)>,
    /// This hook's own default namespace — `[0u8; 32]` unless overridden via
    /// [`crate::TestEnv::own_namespace`].
    pub(crate) own_namespace: [u8; 32],
    pub(crate) state: HashMap<StateAddr, Vec<u8>>,
    /// The normalized keys of every entry seeded via
    /// [`crate::TestEnv::state_entry`] (own account, own namespace at
    /// *some* point — not necessarily the current one). [`Self::rekey_own_seeds`]
    /// consults this set to know which `state` entries a later
    /// `hook_account`/`own_namespace` builder call must follow to their new
    /// address; a `foreign_state_entry` seed is never added here, so it is
    /// never re-keyed even if it happens to land at the same address.
    pub(crate) own_seeded_keys: HashSet<[u8; 32]>,
    pub(crate) grants: HashMap<([u8; 20], [u8; 32]), Vec<Grant>>,
    pub(crate) ledger_seq: u32,
    pub(crate) ledger_time: i64,
    pub(crate) max_state_value_len: usize,
    pub(crate) base_fee_drops: u64,
    pub(crate) committed_emissions: Vec<EmittedTxn>,
    pub(crate) emit_attempts: Vec<EmitAttempt>,
    pub(crate) traces: Vec<TraceLine>,
    /// Monotonically increasing per-invoke counter, mixed into every
    /// deterministic nonce this world hands out (design §4: `H(invocation_counter
    /// ‖ call_counter)`).
    pub(crate) invocation_counter: u64,
}

impl World {
    pub(crate) fn new() -> Self {
        Self {
            hook_account: [0u8; 20],
            hook_hashes: HashMap::new(),
            hook_pos: 0,
            hook_params: HashMap::new(),
            otxn: Otxn::new(rshooks::tx_type::TxType::Payment),
            otxn_emitted: None,
            own_namespace: [0u8; 32],
            state: HashMap::new(),
            own_seeded_keys: HashSet::new(),
            grants: HashMap::new(),
            ledger_seq: 1,
            ledger_time: 0,
            max_state_value_len: DEFAULT_MAX_STATE_VALUE_LEN,
            base_fee_drops: 10,
            committed_emissions: Vec::new(),
            emit_attempts: Vec::new(),
            traces: Vec::new(),
            invocation_counter: 0,
        }
    }

    /// The hash of the hook currently at `hook_pos`, if seeded via
    /// [`crate::TestEnv::hook_hash`].
    pub(crate) fn current_hook_hash(&self) -> Option<[u8; 32]> {
        self.hook_hashes.get(&(self.hook_pos as i32)).copied()
    }

    /// Moves every recorded own-seed entry (see [`Self::own_seeded_keys`])
    /// from `old` (account, namespace) to `new`, so a `state_entry` seed
    /// keeps being readable at whatever address `hook_account`/
    /// `own_namespace` currently name — regardless of which builder call
    /// came first. A no-op if `old == new`. A `foreign_state_entry` seed
    /// that happens to sit at `old` is never touched: only normalized keys
    /// in `own_seeded_keys` are candidates for the move.
    pub(crate) fn rekey_own_seeds(&mut self, old: ([u8; 20], [u8; 32]), new: ([u8; 20], [u8; 32])) {
        if old == new {
            return;
        }
        let keys: Vec<[u8; 32]> = self.own_seeded_keys.iter().copied().collect();
        for key in keys {
            if let Some(value) = self.state.remove(&(old.0, old.1, key)) {
                self.state.insert((new.0, new.1, key), value);
            }
        }
    }

    /// Snapshot of every field a rolled-back/restored invocation must undo:
    /// the state map and the committed-emission list. Everything else
    /// (params, otxn, ledger fields, grants) is not writable by a hook
    /// invocation, so it needs no snapshot/restore.
    pub(crate) fn snapshot(&self) -> WorldSnapshot {
        WorldSnapshot {
            state: self.state.clone(),
            committed_emissions_len: self.committed_emissions.len(),
        }
    }

    pub(crate) fn restore(&mut self, snap: WorldSnapshot) {
        self.state = snap.state;
        self.committed_emissions
            .truncate(snap.committed_emissions_len);
    }
}

/// See [`World::snapshot`]/[`World::restore`].
pub(crate) struct WorldSnapshot {
    state: HashMap<StateAddr, Vec<u8>>,
    committed_emissions_len: usize,
}

/// Left-pad-normalizes a hook-state key per design §5.3 / xahaud's own
/// `state`/`state_set`/`state_foreign(_set)` rule: `0` bytes is `TOO_SMALL`,
/// more than 32 is `TOO_BIG`, otherwise the key is left-zero-padded to a
/// full 32 bytes (so `b"RR"` and its already-32-byte left-padded form
/// address the same entry).
pub(crate) fn normalize_state_key(key: &[u8]) -> Result<[u8; 32], i64> {
    if key.is_empty() {
        return Err(rshooks_core::TOO_SMALL);
    }
    if key.len() > 32 {
        return Err(rshooks_core::TOO_BIG);
    }
    let mut buf = [0u8; 32];
    let pad = 32usize.saturating_sub(key.len());
    if let Some(dst) = buf.get_mut(pad..) {
        dst.copy_from_slice(key);
    }
    Ok(buf)
}
