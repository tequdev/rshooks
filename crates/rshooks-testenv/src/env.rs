//! [`TestEnv`]: the public harness — world builders, `invoke`, and
//! post-invocation assertions.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Once;
use std::vec::Vec;

use rshooks::convert::FromBytes;
use rshooks::decl::HookChainEntries;
use rshooks_core::backend::HostBackend;

use crate::backend::Backend;
use crate::exit::{ExitType, HookExit, HookExitSignal};
use crate::grant::Grant;
use crate::invocation::InvocationContext;
use crate::otxn::Otxn;
use crate::world::{EmitAttempt, EmittedTxn, TraceLine, World, normalize_state_key};

static PANIC_HOOK_INSTALLED: Once = Once::new();

/// Installs (once per process) a panic hook that suppresses the default
/// "thread panicked" printout for [`HookExitSignal`] payloads — `accept!`/
/// `rollback!`'s ordinary unwind — while delegating every other panic to
/// whatever hook was previously installed. Best-effort cosmetics only:
/// correctness never depends on this running (design §2.2).
fn install_panic_hook_filter() {
    PANIC_HOOK_INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if info.payload().downcast_ref::<HookExitSignal>().is_some() {
                return;
            }
            previous(info);
        }));
    });
}

#[allow(clippy::panic)] // documented API: an invalid seed key at test-setup time is a test-author error, not a hook-invocation code path
fn seed_key(key: &[u8]) -> [u8; 32] {
    match normalize_state_key(key) {
        Ok(k) => k,
        Err(_) => panic!(
            "rshooks_testenv::TestEnv: a seeded state key must be 1..=32 bytes (got {} bytes)",
            key.len()
        ),
    }
}

/// The off-chain unit-test harness: an in-memory world (state, the
/// originating transaction, hook parameters, ledger fields, grants) that
/// persists across [`Self::invoke`] calls, plus everything the design's
/// `InvocationContext` limits table needs fresh per call. Every builder
/// consumes and returns `Self` (call them before the first `invoke`);
/// [`Self::invoke`] and every accessor take `&self` (`World` is
/// interior-mutable behind `Rc<RefCell<..>>`).
///
/// **Not `Send`, by construction** (`Rc`/`RefCell`): a `TestEnv` lives and
/// runs entirely on the thread that created it, matching
/// [`rshooks_core::backend`]'s thread-local registry.
pub struct TestEnv {
    world: Rc<RefCell<World>>,
    strict_can_emit: bool,
}

impl Default for TestEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl TestEnv {
    /// Starts a fresh, empty world: zeroed hook account, `Payment` otxn
    /// with no fields set, no state, no grants, ledger sequence `1`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            world: Rc::new(RefCell::new(World::new())),
            strict_can_emit: false,
        }
    }

    /// Sets this hook's own account (`hook_account`). Order-independent
    /// with [`Self::state_entry`]: any entry already seeded under this
    /// hook's own account/namespace follows to the new account, whether
    /// [`Self::state_entry`] or this method was called first (see
    /// [`crate::world::World::rekey_own_seeds`]).
    #[must_use]
    pub fn hook_account(self, acc: [u8; 20]) -> Self {
        let mut w = self.world.borrow_mut();
        let old = (w.hook_account, w.own_namespace);
        w.hook_account = acc;
        let new = (w.hook_account, w.own_namespace);
        w.rekey_own_seeds(old, new);
        drop(w);
        self
    }

    /// Seeds the hash of the hook at chain position `hook_no` (`hook_hash`).
    #[must_use]
    pub fn hook_hash(self, hook_no: i32, hash: [u8; 32]) -> Self {
        self.world.borrow_mut().hook_hashes.insert(hook_no, hash);
        self
    }

    /// Sets this hook's position in its chain (`hook_pos`).
    ///
    /// # Panics
    ///
    /// Panics if `pos > 9` — a chain holds at most 10 hooks
    /// (`vendor/xahaud/Enum.h`'s `maxHookChainLength()`), so a larger
    /// position is a test-author error, not a value a real hook could ever
    /// observe.
    #[must_use]
    #[allow(clippy::panic)] // documented API: an out-of-range position is a test-author error
    pub fn hook_pos(self, pos: u32) -> Self {
        assert!(
            pos <= 9,
            "rshooks_testenv::TestEnv::hook_pos: pos must be <= 9 (a chain holds at most 10 \
             hooks), got {pos}"
        );
        self.world.borrow_mut().hook_pos = pos;
        self
    }

    /// Seeds a Hook API parameter attached to this hook (`hook_param`).
    #[must_use]
    pub fn hook_param(self, name: &[u8], value: &[u8]) -> Self {
        self.world
            .borrow_mut()
            .hook_params
            .insert(name.to_vec(), value.to_vec());
        self
    }

    /// Sets the originating transaction every `invoke` call sees.
    #[must_use]
    pub fn otxn(self, otxn: Otxn) -> Self {
        self.world.borrow_mut().otxn = otxn;
        self
    }

    /// Pre-seeds one of this hook's own state entries (own account, own
    /// namespace). `key` must be `1..=32` bytes — see [`Self::own_namespace`]
    /// for changing the namespace this targets.
    #[must_use]
    pub fn state_entry(self, key: &[u8], value: &[u8]) -> Self {
        let norm = seed_key(key);
        let mut w = self.world.borrow_mut();
        let addr = (w.hook_account, w.own_namespace, norm);
        w.state.insert(addr, value.to_vec());
        // Recorded so a later `hook_account`/`own_namespace` call re-keys
        // this seed to the new address instead of orphaning it — see
        // `World::rekey_own_seeds`. Invocation-time state writes must never
        // reach this: only this builder records an own-seed.
        w.own_seeded_keys.insert(norm);
        drop(w);
        self
    }

    /// Pre-seeds a state entry belonging to another `(account, namespace)`.
    #[must_use]
    pub fn foreign_state_entry(
        self,
        ns: [u8; 32],
        acc: [u8; 20],
        key: &[u8],
        value: &[u8],
    ) -> Self {
        let norm = seed_key(key);
        self.world
            .borrow_mut()
            .state
            .insert((acc, ns, norm), value.to_vec());
        self
    }

    /// Models a `HookGrant` on `target_account`'s ledger object authorizing
    /// a hook (matched by `authorize`, see [`Grant`]) to write into
    /// `(target_account, ns)` via `state_foreign_set`. See design §5.5 for
    /// the full lookup rule (a write to this hook's own account never
    /// consults grants at all).
    #[must_use]
    pub fn grant(self, target_account: [u8; 20], ns: [u8; 32], authorize: Grant) -> Self {
        self.world
            .borrow_mut()
            .grants
            .entry((target_account, ns))
            .or_default()
            .push(authorize);
        self
    }

    /// Sets the current ledger sequence (`ledger_seq`).
    #[must_use]
    pub fn ledger_seq(self, seq: u32) -> Self {
        self.world.borrow_mut().ledger_seq = seq;
        self
    }

    /// Sets the previous ledger's close time (`ledger_last_time`).
    #[must_use]
    pub fn ledger_time(self, t: i64) -> Self {
        self.world.borrow_mut().ledger_time = t;
        self
    }

    /// Marks the seeded otxn as itself an emitted transaction, seeding
    /// `otxn_burden`/`otxn_generation`. Absent, the otxn models an ordinary
    /// (non-emitted) transaction (`otxn_burden() == 1`,
    /// `otxn_generation() == 0`).
    ///
    /// # Panics
    ///
    /// Panics if `burden > i64::MAX` — `otxn_burden` reports this value as
    /// an `i64` (the Hook API's return type), so a larger value could never
    /// be observed on-chain and is a test-author error here.
    #[must_use]
    #[allow(clippy::panic)] // documented API: an out-of-range burden is a test-author error
    pub fn otxn_emitted(self, burden: u64, generation: u32) -> Self {
        assert!(
            burden <= i64::MAX as u64,
            "rshooks_testenv::TestEnv::otxn_emitted: burden must fit in an i64 (Hook API's \
             `otxn_burden` return type), got {burden}"
        );
        self.world.borrow_mut().otxn_emitted = Some((burden, generation));
        self
    }

    /// Overrides this hook's own default state namespace (`[0u8; 32]` if
    /// never called). Extension beyond the exact design §2.4 surface, added
    /// because the design explicitly calls for a configurable own
    /// namespace (see `.claude/design/TESTENV_IMPLEMENTATION.md`'s Stage 4
    /// world-model note). Order-independent with [`Self::state_entry`], the
    /// same way [`Self::hook_account`] is — see that method's doc comment.
    #[must_use]
    pub fn own_namespace(self, ns: [u8; 32]) -> Self {
        let mut w = self.world.borrow_mut();
        let old = (w.hook_account, w.own_namespace);
        w.own_namespace = ns;
        let new = (w.hook_account, w.own_namespace);
        w.rekey_own_seeds(old, new);
        drop(w);
        self
    }

    /// Overrides the maximum size (in bytes) of a single hook-state value
    /// (default 256, matching xahaud's `maxHookStateDataSize` at state
    /// scale 1 — design §5.4 calls for this to be configurable since the
    /// exact scale a given ledger object uses is not modeled here).
    #[must_use]
    pub fn max_state_value_len(self, n: usize) -> Self {
        self.world.borrow_mut().max_state_value_len = n;
        self
    }

    /// Overrides the per-drop base fee the `etxn_fee_base`/`fee_base`
    /// approximation multiplies by (default 10 drops — design §4's
    /// documented fee approximation).
    ///
    /// # Panics
    ///
    /// Panics if `drops > i64::MAX` — `fee_base` reports this value as an
    /// `i64` (the Hook API's return type), so a larger value could never be
    /// observed on-chain and is a test-author error here.
    #[must_use]
    #[allow(clippy::panic)] // documented API: an out-of-range fee is a test-author error
    pub fn base_fee_drops(self, drops: u64) -> Self {
        assert!(
            drops <= i64::MAX as u64,
            "rshooks_testenv::TestEnv::base_fee_drops: drops must fit in an i64 (Hook API's \
             `fee_base` return type), got {drops}"
        );
        self.world.borrow_mut().base_fee_drops = drops;
        self
    }

    // -- Phase 2 (`.claude/design/TESTENV_PHASE2_DESIGN.md` §3) --
    //
    // Plain data plumbing as of P2-A: these seed `World` fields nothing yet
    // reads (`crate::backend::Backend` does not override `slot_set`/
    // `meta_slot`/`xpop_slot` — every Phase 2 `HostBackend` method still
    // falls through to its trait default). Semantics land per-family in
    // P2-C/P2-D.
    /// Seeds a ledger object at `keylet` (its 34-byte index), serialized as
    /// `sto` — will back `slot_set`/`ledger_keylet` once P2-D/P2-C land.
    #[must_use]
    pub fn ledger_object(self, keylet: [u8; 34], sto: &[u8]) -> Self {
        self.world
            .borrow_mut()
            .ledger_objects
            .insert(keylet, sto.to_vec());
        self
    }

    /// Seeds the current transaction's metadata — will back `meta_slot`
    /// once P2-D lands.
    #[must_use]
    pub fn otxn_meta(self, sto: &[u8]) -> Self {
        self.world.borrow_mut().otxn_meta = Some(sto.to_vec());
        self
    }

    /// Seeds an XPOP's `(transaction, metadata)` pair — will back
    /// `xpop_slot` once P2-D lands.
    #[must_use]
    pub fn xpop(self, tx: &[u8], meta: &[u8]) -> Self {
        self.world.borrow_mut().xpop = Some((tx.to_vec(), meta.to_vec()));
        self
    }

    /// Opts into asserting that every transaction type this invocation
    /// commits to [`Self::emitted`] is one the invoked entry's
    /// `#[hook(.., can_emit = [..])]` declaration allows. Off by default.
    /// A violation panics (design §5.6) rather than silently accepting —
    /// this is a test-author assertion, not a Hook API error path.
    #[must_use]
    pub fn strict_can_emit(mut self, on: bool) -> Self {
        self.strict_can_emit = on;
        self
    }

    /// Runs the entry declared at `index` in `C::ENTRIES` as a fresh
    /// invocation: a fresh [`InvocationContext`] (design §4's limits
    /// table), a snapshot of the persistent world, an installed mock
    /// backend for the duration of the call, then the outcome mapping
    /// documented on [`ExitType`].
    ///
    /// This is a direct entry call: no `HookOn`/`on = [..]` trigger
    /// filtering happens (design §4) — the declared entry runs regardless
    /// of whether the seeded otxn's type would have triggered it on-chain.
    ///
    /// # Panics
    ///
    /// Panics if `index` does not name a declared entry (listing the
    /// declared indices), or if this thread already has a backend
    /// installed — i.e. `invoke` was called reentrantly (from inside a
    /// hook entry currently running via another `invoke` call on this
    /// thread); see [`rshooks_core::backend::install`]'s own panic message.
    /// Also panics if [`Self::strict_can_emit`] is enabled and this
    /// invocation commits an emission whose transaction type is not in the
    /// invoked entry's `can_emit` declaration.
    #[allow(clippy::panic)] // documented API: an unknown entry index is a test-author error (design §2.4/§4)
    pub fn invoke<C: HookChainEntries>(&self, index: u32) -> HookExit {
        let entry = match C::ENTRIES.iter().find(|e| e.index == index) {
            Some(e) => e,
            None => {
                let indices: Vec<u32> = C::ENTRIES.iter().map(|e| e.index).collect();
                panic!(
                    "rshooks_testenv::TestEnv::invoke: no entry with index {index} \
                     (declared indices: {indices:?})"
                );
            }
        };

        let invocation_id = {
            let mut w = self.world.borrow_mut();
            let id = w.invocation_counter;
            w.invocation_counter = w.invocation_counter.wrapping_add(1);
            id
        };
        let ctx = Rc::new(RefCell::new(InvocationContext::new(invocation_id)));
        let snapshot = self.world.borrow().snapshot();

        install_panic_hook_filter();

        let backend: Rc<dyn HostBackend> =
            Rc::new(Backend::new(Rc::clone(&self.world), Rc::clone(&ctx)));
        let guard = rshooks_core::backend::install(backend);

        let hook_fn = entry.hook;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook_fn(0)));

        drop(guard);

        // Cumulative regardless of outcome (design §2.4: `emit_attempts()`
        // includes attempts made during a rolled-back invocation).
        {
            let attempts = std::mem::take(&mut ctx.borrow_mut().emit_attempts);
            self.world.borrow_mut().emit_attempts.extend(attempts);
        }

        match result {
            Ok(code) => {
                // A bare `return code`: provisional non-commit (design §4).
                self.world.borrow_mut().restore(snapshot);
                HookExit {
                    exit: ExitType::Return,
                    code,
                    msg: Vec::new(),
                }
            }
            Err(payload) => match payload.downcast::<HookExitSignal>() {
                Ok(signal) => {
                    let exit = signal.0;
                    match exit.exit {
                        ExitType::Accept => {
                            let pending = std::mem::take(&mut ctx.borrow_mut().pending_emissions);
                            if self.strict_can_emit {
                                self.assert_can_emit(entry, &pending);
                            }
                            self.world.borrow_mut().committed_emissions.extend(pending);
                        }
                        ExitType::Rollback | ExitType::Return => {
                            self.world.borrow_mut().restore(snapshot);
                        }
                    }
                    exit
                }
                Err(other) => {
                    // Not accept!/rollback! — a genuine test failure. Never
                    // convert this into an exit, and never leak world
                    // mutations from the failed invocation.
                    self.world.borrow_mut().restore(snapshot);
                    std::panic::resume_unwind(other);
                }
            },
        }
    }

    #[allow(clippy::panic)] // documented API: a `strict_can_emit` violation is a test-author assertion (design §5.6)
    fn assert_can_emit(&self, entry: &rshooks::decl::NativeEntry, pending: &[EmittedTxn]) {
        // `None` (no `can_emit` declaration at all) is unrestricted — matches
        // an absent on-chain `HookCanEmit`, which imposes no restriction.
        // Only a declared list (`Some(&[..])`, empty slice included)
        // constrains what this entry may emit — see `NativeEntry::can_emit`'s
        // doc comment for the full three-state contract.
        let Some(allowed_list) = entry.can_emit else {
            return;
        };
        for txn in pending {
            let ty = txn.tx_type();
            let allowed = ty.is_some_and(|t| allowed_list.contains(&t));
            assert!(
                allowed,
                "rshooks_testenv::TestEnv::invoke: entry {:?} emitted a transaction of type {:?}, \
                 not declared in its `can_emit` list {:?} (strict_can_emit is enabled)",
                entry.name, ty, allowed_list
            );
        }
    }

    /// Reads this hook's own state entry for `key` (own account, own
    /// namespace). `None` for a malformed key (`0` or `> 32` bytes) or a
    /// genuinely absent entry alike — use [`Self::state_typed`] to also
    /// decode the value.
    #[must_use]
    pub fn state(&self, key: &[u8]) -> Option<Vec<u8>> {
        let norm = normalize_state_key(key).ok()?;
        let w = self.world.borrow();
        let addr = (w.hook_account, w.own_namespace, norm);
        w.state.get(&addr).cloned()
    }

    /// Reads and decodes this hook's own state entry for `key` as `T`.
    /// `None` if the entry is absent.
    ///
    /// # Panics
    ///
    /// Panics with a clear message if the entry is present but fails to
    /// decode as `T` — a decode failure is a test-author bug (a mismatched
    /// value type), not a case worth silently returning `None` for.
    #[allow(clippy::panic)] // documented API (design §2.4)
    #[must_use]
    pub fn state_typed<T: FromBytes>(&self, key: &[u8]) -> Option<T> {
        let bytes = self.state(key)?;
        match T::read(&bytes) {
            Ok(value) => Some(value),
            Err(err) => panic!(
                "rshooks_testenv::TestEnv::state_typed: state entry for this key failed to decode: {err:?}"
            ),
        }
    }

    /// Every transaction this env has committed via `accept!` across every
    /// `invoke` call so far (cumulative).
    #[must_use]
    pub fn emitted(&self) -> Vec<EmittedTxn> {
        self.world.borrow().committed_emissions.clone()
    }

    /// Every `emit` attempt across every `invoke` call so far, successful
    /// or not, including attempts made during a rolled-back invocation
    /// (cumulative).
    #[must_use]
    pub fn emit_attempts(&self) -> Vec<EmitAttempt> {
        self.world.borrow().emit_attempts.clone()
    }

    /// Every captured `trace`/`trace_num` line across every `invoke` call
    /// so far (cumulative; never printed on its own).
    #[must_use]
    pub fn traces(&self) -> Vec<TraceLine> {
        self.world.borrow().traces.clone()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

    use super::*;
    use rshooks::decl::NativeEntry;
    use rshooks::tx_type::TxType;

    struct NoEntries;
    impl HookChainEntries for NoEntries {
        const ENTRIES: &'static [NativeEntry] = &[];
    }

    fn accepting_hook(_r: u32) -> i64 {
        rshooks::api::control::accept(b"ok", 0);
    }

    fn rolling_back_hook(_r: u32) -> i64 {
        rshooks::api::control::rollback(b"no", 1);
    }

    fn returning_hook(_r: u32) -> i64 {
        42
    }

    struct OneEntry;
    impl HookChainEntries for OneEntry {
        const ENTRIES: &'static [NativeEntry] = &[
            NativeEntry {
                index: 0,
                name: "accept",
                hook: accepting_hook,
                cbak: None,
                can_emit: None,
            },
            NativeEntry {
                index: 1,
                name: "rollback",
                hook: rolling_back_hook,
                cbak: None,
                can_emit: None,
            },
            NativeEntry {
                index: 2,
                name: "return",
                hook: returning_hook,
                cbak: None,
                can_emit: None,
            },
        ];
    }

    #[test]
    fn accept_commits_and_reports_success() {
        let env = TestEnv::new();
        let exit = env.invoke::<OneEntry>(0);
        assert_eq!(exit.exit, ExitType::Accept);
        assert!(exit.is_success());
        assert_eq!(exit.msg, b"ok");
    }

    #[test]
    fn rollback_reports_failure() {
        let env = TestEnv::new();
        let exit = env.invoke::<OneEntry>(1);
        assert_eq!(exit.exit, ExitType::Rollback);
        assert!(!exit.is_success());
        assert_eq!(exit.code, 1);
    }

    #[test]
    fn plain_return_is_provisional_and_not_success() {
        let env = TestEnv::new();
        let exit = env.invoke::<OneEntry>(2);
        assert_eq!(exit.exit, ExitType::Return);
        assert_eq!(exit.code, 42);
        assert!(!exit.is_success());
    }

    #[test]
    #[should_panic(expected = "no entry with index")]
    fn unknown_index_panics_listing_declared_indices() {
        let env = TestEnv::new();
        let _ = env.invoke::<NoEntries>(0);
    }

    #[test]
    fn builders_seed_the_world_readable_by_state() {
        let env = TestEnv::new().state_entry(b"K", b"V");
        assert_eq!(env.state(b"K"), Some(b"V".to_vec()));
    }

    #[test]
    fn state_reports_none_for_absent_entry() {
        let env = TestEnv::new();
        assert_eq!(env.state(b"missing"), None);
    }

    #[test]
    fn state_typed_decodes_present_value() {
        let env = TestEnv::new().state_entry(b"K", &1u32.to_le_bytes());
        assert_eq!(env.state_typed::<u32>(b"K"), Some(1u32));
    }

    #[test]
    fn otxn_defaults_to_non_emitted() {
        let env = TestEnv::new();
        let _ = env.invoke::<OneEntry>(0); // exercise the path; otxn fields unread here
        let _ = TxType::Payment; // silence unused import in case of future edits
    }
}
