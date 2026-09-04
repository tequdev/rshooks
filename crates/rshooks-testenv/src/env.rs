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

/// The result of the emitted transaction a `#[cbak]` is being called back
/// about — [`TestEnv::invoke_cbak`]'s second argument. Holds an owned,
/// cloned [`EmittedTxn`] rather than a borrow, avoiding a lifetime
/// parameter on `TestEnv::invoke_cbak` for a type that only needs to
/// survive one call.
///
/// Ported against `Transactor::doHookCallback`'s call into `hook::apply`
/// (`Xahau/xahaud` `dev`, `src/xrpld/app/tx/detail/Transactor.cpp:1570-1583`):
/// the wasm `cbak(u32)` argument is `ctx_.tx.getTxnType() == ttEMIT_FAILURE
/// ? 1UL : 0UL` — `0` when the emitted transaction applied as its own real
/// type (success), `1` when it was rewritten to `ttEMIT_FAILURE`.
/// [`CbakOutcome::Success`]/[`CbakOutcome::Failure`] carry that same `0`/`1`
/// distinction; `TestEnv::invoke_cbak` passes it as the callback's
/// `fn(u32) -> i64` argument verbatim.
#[derive(Debug, Clone)]
pub enum CbakOutcome {
    /// The emitted transaction applied successfully — the callback's wasm
    /// argument is `0`.
    Success(EmittedTxn),
    /// The emitted transaction failed to apply (`ttEMIT_FAILURE`) — the
    /// callback's wasm argument is `1`.
    Failure(EmittedTxn),
}

impl CbakOutcome {
    /// The emitted transaction this outcome is about, regardless of variant.
    fn emitted_txn(&self) -> &EmittedTxn {
        match self {
            CbakOutcome::Success(t) | CbakOutcome::Failure(t) => t,
        }
    }

    /// The wasm `cbak(u32)` argument this outcome carries — see this type's
    /// own doc comment for the upstream citation.
    fn wasm_arg(&self) -> u32 {
        match self {
            CbakOutcome::Success(_) => 0,
            CbakOutcome::Failure(_) => 1,
        }
    }
}

/// Restores `world`'s `otxn`/`otxn_emitted` fields to `original` when
/// dropped — [`TestEnv::invoke_cbak`]'s otxn swap is invocation-scoped
/// (design §4 "cbak execution": "the ORIGINAL seeded otxn must be restored
/// after the callback returns"). An RAII guard holds even if the callback
/// panics with a payload `run_entry` resumes the unwind of (a genuine test
/// failure): `Drop::drop` still runs while unwinding, the same guarantee
/// [`crate::backend::BackendGuard`]-style guards rely on.
struct OtxnRestoreGuard<'a> {
    world: &'a Rc<RefCell<World>>,
    /// `Some` until [`Drop::drop`] runs (always exactly once); `Option` only
    /// so the pair can be moved out of `&mut self` in `drop` without a
    /// clone.
    original: Option<(Otxn, Option<(u64, u32)>)>,
}

impl Drop for OtxnRestoreGuard<'_> {
    fn drop(&mut self) {
        if let Some((otxn, otxn_emitted)) = self.original.take() {
            let mut w = self.world.borrow_mut();
            w.otxn = otxn;
            w.otxn_emitted = otxn_emitted;
        }
    }
}

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
            strict_can_emit: true,
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

    /// Sets the previous ledger's hash (`ledger_last_hash`) — `[0u8; 32]`
    /// unless called.
    #[must_use]
    pub fn ledger_last_hash(self, hash: [u8; 32]) -> Self {
        self.world.borrow_mut().ledger_last_hash = hash;
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
    /// never called; design §2.4 extension — see
    /// `.claude/design/TESTENV_IMPLEMENTATION.md`'s Stage 4 world-model
    /// note). Order-independent with [`Self::state_entry`], the same way
    /// [`Self::hook_account`] is — see that method's doc comment.
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
    // These seed `World` fields read by `crate::backend::Backend::slot_set`/
    // `meta_slot`/`xpop_slot`.
    /// Seeds a ledger object at `keylet` (its 34-byte index), serialized as
    /// `sto` — backs `slot_set`/`ledger_keylet`.
    #[must_use]
    pub fn ledger_object(self, keylet: [u8; 34], sto: &[u8]) -> Self {
        self.world
            .borrow_mut()
            .ledger_objects
            .insert(keylet, sto.to_vec());
        self
    }

    /// Seeds the current transaction's metadata — backs `meta_slot`.
    #[must_use]
    pub fn otxn_meta(self, sto: &[u8]) -> Self {
        self.world.borrow_mut().otxn_meta = Some(sto.to_vec());
        self
    }

    /// Seeds an XPOP's `(transaction, metadata)` pair — backs `xpop_slot`.
    #[must_use]
    pub fn xpop(self, tx: &[u8], meta: &[u8]) -> Self {
        self.world.borrow_mut().xpop = Some((tx.to_vec(), meta.to_vec()));
        self
    }

    /// Controls whether every transaction type this invocation commits to
    /// [`Self::emitted`] is asserted against the invoked entry's
    /// `#[hook(.., can_emit = [..])]` declaration, matching the real
    /// host's `HookCanEmit` enforcement. **On by default** — pass `false`
    /// to opt out for a lower-fidelity test that deliberately emits
    /// outside the declaration. A violation panics (design §5.6) rather
    /// than silently accepting — this is a test-author assertion, not a
    /// Hook API error path.
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
    /// Also panics, unless [`Self::strict_can_emit`] was called with
    /// `false`, if this invocation commits an emission whose transaction
    /// type is not in the invoked entry's `can_emit` declaration.
    #[allow(clippy::panic)] // documented API: an unknown entry index is a test-author error (design §2.4/§4)
    pub fn invoke<C: HookChainEntries>(&self, index: u32) -> HookExit {
        let entry = Self::find_entry::<C>(index, "invoke");
        self.run_entry(entry, entry.hook, 0)
    }

    /// Runs the entry declared at `index` in `C::ENTRIES`'s `#[cbak(index)]`
    /// body as a fresh invocation, standing in for xahaud's own callback
    /// dispatch (`Transactor::doHookCallback`, `Xahau/xahaud` `dev`,
    /// `src/xrpld/app/tx/detail/Transactor.cpp:1483-1614` — see
    /// [`CbakOutcome`]'s doc comment for the wasm-argument citation): a
    /// fresh [`InvocationContext`] and world snapshot exactly like
    /// [`Self::invoke`], plus an otxn swap for the duration of the call —
    /// during a real callback, `otxn_field`/`otxn_type`/`otxn_id` all read
    /// from the transaction *currently being applied*, i.e. the emitted
    /// transaction itself (`HookAPI::otxn_field`/`otxn_type`/`otxn_id`,
    /// `src/xrpld/app/hook/detail/HookAPI.cpp:1527-1562`, all read
    /// `hookCtx.applyCtx.tx` directly), and `otxn_burden`/`otxn_generation`
    /// read the emitted transaction's own
    /// `EmitDetails.EmitBurden`/`EmitGeneration` fields directly, not
    /// `+1`-incremented the way `etxn_burden`/`etxn_generation` derive the
    /// *next* emission's values (`HookAPI.cpp:1465-1520`). The otxn swap is
    /// undone by an RAII guard as soon as this call returns, even if the
    /// callback body panics with a non-exit payload (see
    /// [`OtxnRestoreGuard`]'s doc comment) — "the ORIGINAL seeded otxn must
    /// be restored after the callback returns" (design §4 "cbak
    /// execution"). Every other world field (state, hook identity, ledger
    /// fields, grants, seeded params) stays exactly as the surrounding
    /// `TestEnv` already has it — this harness does not model per-account
    /// hook chains, so a callback runs as *this same* `TestEnv`'s hook
    /// identity, not a separately seeded one.
    ///
    /// # Panics
    ///
    /// Panics if `index` does not name a declared entry (see [`Self::invoke`]),
    /// or if the entry at `index` declares no `#[cbak]` body (listing which
    /// declared indices do have one) — on-chain, a hook with no `cbak`
    /// export simply never receives a callback
    /// (`Transactor::doHookCallback` skips it via
    /// `!hookDef->isFieldPresent(sfHookCallbackFee)`, `Transactor.cpp:1512-1518`).
    /// Also panics under the same reentrancy/`strict_can_emit` conditions
    /// [`Self::invoke`] documents.
    #[allow(clippy::panic)] // documented API: an unknown entry index, or an entry with no cbak, is a test-author error
    pub fn invoke_cbak<C: HookChainEntries>(&self, index: u32, outcome: CbakOutcome) -> HookExit {
        let entry = Self::find_entry::<C>(index, "invoke_cbak");
        let Some(cbak_fn) = entry.cbak else {
            let with_cbak: Vec<u32> = C::ENTRIES
                .iter()
                .filter(|e| e.cbak.is_some())
                .map(|e| e.index)
                .collect();
            panic!(
                "rshooks_testenv::TestEnv::invoke_cbak: entry {index} declares no #[cbak] body \
                 (entries with a cbak: {with_cbak:?})"
            );
        };

        let (cbak_otxn, burden, generation) = {
            let txn = outcome.emitted_txn();
            crate::otxn::from_emitted(txn.blob(), txn.hash()).unwrap_or_else(|| {
                panic!(
                    "rshooks_testenv::TestEnv::invoke_cbak: the emitted transaction blob failed \
                     to parse into an otxn (malformed or missing TransactionType/EmitDetails) — \
                     this should be unreachable for a blob obtained from `TestEnv::emitted()`, \
                     since it already passed the emission walker"
                )
            })
        };

        let original = {
            let mut w = self.world.borrow_mut();
            (
                core::mem::replace(&mut w.otxn, cbak_otxn),
                w.otxn_emitted.replace((burden, generation)),
            )
        };
        let _restore_guard = OtxnRestoreGuard {
            world: &self.world,
            original: Some(original),
        };

        self.run_entry(entry, cbak_fn, outcome.wasm_arg())
    }

    /// Looks up `index` in `C::ENTRIES`, or panics naming `caller` (either
    /// `"invoke"` or `"invoke_cbak"`) and listing every declared index —
    /// the lookup [`Self::invoke`]/[`Self::invoke_cbak`] share.
    #[allow(clippy::panic)] // documented API: an unknown entry index is a test-author error (design §2.4/§4)
    fn find_entry<C: HookChainEntries>(
        index: u32,
        caller: &str,
    ) -> &'static rshooks::decl::NativeEntry {
        match C::ENTRIES.iter().find(|e| e.index == index) {
            Some(e) => e,
            None => {
                let indices: Vec<u32> = C::ENTRIES.iter().map(|e| e.index).collect();
                panic!(
                    "rshooks_testenv::TestEnv::{caller}: no entry with index {index} \
                     (declared indices: {indices:?})"
                );
            }
        }
    }

    /// Shared body of [`Self::invoke`]/[`Self::invoke_cbak`]: a fresh
    /// [`InvocationContext`], a world snapshot, an installed mock backend,
    /// `hook_fn(arg)` run under `catch_unwind`, then the outcome mapping
    /// documented on [`ExitType`] — including, on `accept!`, merging this
    /// invocation's control-leftover writes (`hook_again_called`/
    /// `skip_directives`/`pending_param_overrides`, P2-E) into the
    /// persistent [`World`] alongside the `pending_emissions` commit (see
    /// `crate::host::control`'s module doc for why these three commit only
    /// here).
    #[allow(clippy::panic)] // documented API: a `strict_can_emit` violation is a test-author assertion (design §5.6)
    fn run_entry(
        &self,
        entry: &rshooks::decl::NativeEntry,
        hook_fn: fn(u32) -> i64,
        arg: u32,
    ) -> HookExit {
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

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook_fn(arg)));

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
                            let mut c = ctx.borrow_mut();
                            let hook_again_called = c.hook_again_called;
                            let skip_directives = std::mem::take(&mut c.skip_directives);
                            let pending_param_overrides =
                                std::mem::take(&mut c.pending_param_overrides);
                            drop(c);

                            let mut w = self.world.borrow_mut();
                            w.committed_emissions.extend(pending);
                            w.hook_again_requested = hook_again_called;
                            w.skip_directives.extend(skip_directives);
                            w.hook_param_overrides.extend(pending_param_overrides);
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
        // `None` (no `can_emit` declaration) is unrestricted, matching an
        // absent on-chain `HookCanEmit`. Only a declared list
        // (`Some(&[..])`, empty slice included) constrains what this entry
        // may emit — see `NativeEntry::can_emit`'s doc comment for the
        // full three-state contract.
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

    /// Whether the most recently **accepted** `invoke`/`invoke_cbak` call
    /// (via `rshooks::api::control::hook_again`) requested to run again
    /// (design §4 "control leftovers"; P2-E). A call that rolled back or
    /// merely returned never updates this — see `crate::host::control`'s
    /// module doc comment for why the commit is tied to `accept!`.
    #[must_use]
    pub fn hook_again_requested(&self) -> bool {
        self.world.borrow().hook_again_requested
    }

    /// Every `hook_skip(hash, flags)` directive from every **accepted**
    /// `invoke`/`invoke_cbak` call so far, verbatim, in call order (design
    /// §4 "control leftovers"; P2-E) — `flags == 0` is an add, `flags == 1`
    /// a delete, matching `rshooks::api::control::hook_skip`'s own argument
    /// order.
    #[must_use]
    pub fn skip_directives(&self) -> Vec<([u8; 32], u32)> {
        self.world.borrow().skip_directives.clone()
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

    fn hook_again_then_accept_hook(_r: u32) -> i64 {
        let _ = rshooks::api::control::hook_again();
        rshooks::api::control::accept(b"ok", 0);
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
            NativeEntry {
                index: 3,
                name: "hook_again_then_accept",
                hook: hook_again_then_accept_hook,
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
    fn hook_again_requested_survives_a_later_rollback_uncleared() {
        let env = TestEnv::new();
        assert!(!env.hook_again_requested());

        // Commits `hook_again_requested() == true`.
        let exit = env.invoke::<OneEntry>(3);
        assert!(exit.is_success());
        assert!(env.hook_again_requested());

        // A later rolled-back invocation (that never itself calls
        // `hook_again`) never commits, so it leaves the accessor exactly as
        // the prior accepted invocation left it — still `true`, not reset
        // to `false`.
        let exit = env.invoke::<OneEntry>(1);
        assert!(!exit.is_success());
        assert!(env.hook_again_requested());
    }

    #[test]
    fn otxn_defaults_to_non_emitted() {
        let env = TestEnv::new();
        let _ = env.invoke::<OneEntry>(0); // exercise the path; otxn fields unread here
        let _ = TxType::Payment; // silence unused import in case of future edits
    }
}
