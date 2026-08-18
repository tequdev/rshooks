# Off-Chain Unit Tests

Every other page in this book runs against a real Hook API — either the
live standalone node the end-to-end suite (`docs/E2E-TESTING.md` at the
repository root) deploys to, or, implicitly, the wasm host your Hook
eventually ships to.
This page covers a third option: `rshooks-testenv`, a mock host that runs a
`#[hooks]` chain entry as plain native Rust under `cargo test` — no wasm
build, no Docker, no node — with assertions on state changes, accept/rollback
exits, and emitted transactions.

## Positioning: not a fidelity oracle

`cargo test` against `rshooks-testenv` answers one question fast: *is my
hook's logic right*, in milliseconds, iterating faster than a wasm build
lets you. It is not a substitute for the end-to-end suite. Fee and reserve
economics, real instruction counting, real guard enforcement, ledger
objects/keylets/slots, XFL float operations, signature verification, and
multi-hook chain execution (including `HookOn` trigger routing) are all out
of scope here — see [What this harness does not model](#what-this-harness-does-not-model)
below for the complete list. Treat the two suites as complementary: reach
for `rshooks-testenv` while you're writing and refactoring hook logic, and
rely on the end-to-end suite to confirm the compiled artifact behaves the
same way on a real ledger.

## Setup

`rshooks-testenv` is a separate crate; both it and `rshooks`'s `testenv`
feature belong in `[dev-dependencies]` — a hook crate ships `no_std` with
neither of them linked into its wasm artifact.

```toml
[lib]
crate-type = ["cdylib", "rlib"]   # rlib only needed for tests/ integration tests

[dependencies]
rshooks = { version = "0.0.1", features = ["host-panic-handler"] }

[dev-dependencies]
rshooks = { version = "0.0.1", features = ["testenv"] }
rshooks-testenv = "0.0.1"
```

Declaring `rshooks` twice — once in `[dependencies]`, once in
`[dev-dependencies]` with a different feature list — is intentional, not a
mistake: Cargo unifies the two into one feature set (`host-panic-handler` +
`testenv`, plus rshooks's default `panic-handler`) for `cargo test` builds,
while a plain `cargo build`/`cargo rustc --crate-type cdylib` (what
`rshooks build` actually runs) never activates dev-dependencies at all, so
`testenv` never reaches the shipped wasm. `crate-type = ["cdylib", "rlib"]`
is only needed if you write tests in a separate `tests/` directory (below);
an in-crate `#[cfg(test)]` module needs neither the extra crate-type nor a
separate `pub` export, just the chain struct itself being `pub` (which
every `#[hooks]` struct in this book already is).

### Two layouts

`rshooks-testenv` supports either style, and a crate can use both at once:

- **`tests/*.rs` integration tests** treat the hook crate as an ordinary
  library dependency (`use my_hook::MyChain;`). This needs the `rlib`
  crate-type addition above, but the crate's `#![no_std]` attribute is
  untouched — the library itself never changes.
- **An in-crate `#[cfg(test)] mod tests` module** lives directly in
  `src/lib.rs`. Because `#![no_std]` and a `std`-using test module can't
  coexist unconditionally, the attribute becomes
  `#![cfg_attr(not(test), no_std)]`: `no_std` stays in force for every real
  build (including the wasm build — `cfg(test)` is never set there), and
  only the `cargo test` compilation of the crate itself switches it off. A
  `#[cfg(test)]` module contributes nothing to any non-test build, wasm
  included, so this changes nothing about what ships.

`examples/02_state-counter` uses the `tests/` layout
(`examples/02_state-counter/tests/counter.rs`); `examples/10_emit-txn`
demonstrates both side by side — `examples/10_emit-txn/tests/emit.rs` and an
in-crate module at the bottom of `examples/10_emit-txn/src/lib.rs`. Both
examples' `README.md` show the exact `cargo test` invocation.

## A worked example

This is `examples/02_state-counter` reduced to its `#[hooks]` declaration —
unchanged from every other page in this book:

```rust,ignore
#[hooks]
pub struct StateCounter {
    #[state(key = b"counter")]
    counter: State<u64>,
}

#[hooks]
impl StateCounter {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> i64 {
        let count = self.counter.get().unwrap_or(Some(0)).unwrap_or(0);
        let next = count.wrapping_add(1);
        if self.counter.set(&next).is_err() {
            rollback!(b"state-counter: state_set failed", StateCounterError::StateSetFailed);
        }
        accept!(b"state-counter: incremented", next as i64)
    }
}
```

and the real test file that drives it, `examples/02_state-counter/tests/counter.rs`:

```rust,ignore
use rshooks_testenv::prelude::*;
use state_counter::{StateCounter, StateCounterError};

fn env() -> TestEnv {
    TestEnv::new()
        .hook_account([1u8; 20])
        .otxn(Otxn::new(TxType::Invoke).account([2u8; 20]))
}

#[test]
fn first_invoke_counts_to_one() {
    let env = env();
    let exit = env.invoke::<StateCounter>(0);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(exit.code, 1);
    assert_eq!(env.state_typed::<u64>(b"counter"), Some(1));
}

#[test]
fn counter_persists_across_invocations() {
    let env = env();
    env.invoke::<StateCounter>(0);
    env.invoke::<StateCounter>(0);
    assert_eq!(env.state_typed::<u64>(b"counter"), Some(2));
}

#[test]
fn state_set_failure_rolls_back_without_persisting() {
    // Cap the value size below the 8-byte `u64` write this hook always
    // attempts, forcing `state_set` to fail so the hook's own rollback
    // path runs.
    let env = env().max_state_value_len(4);
    let exit = env.invoke::<StateCounter>(0);
    assert_eq!(exit.exit, ExitType::Rollback);
    assert_eq!(exit.code, StateCounterError::StateSetFailed.code());
    assert_eq!(env.state_typed::<u64>(b"counter"), None);
}
```

`rshooks_testenv::prelude::*` pulls in `TestEnv`, `HookExit`/`ExitType`,
`Otxn`, `Grant`, `EmittedTxn`/`EmitAttempt`/`TraceLine`, and
`rshooks::decl::HookChainEntries` — the trait `invoke`'s type parameter is
bound by, implemented automatically on every `#[hooks]` chain struct on a
non-wasm target. `env()` builds a fresh `TestEnv` per test (it consumes and
returns `self`, so every builder call before the first `invoke` is a plain
chain), and `invoke::<StateCounter>(0)` runs the entry declared
`#[hook(0, ...)]` directly, by index — see [Direct-entry invocation](#direct-entry-invocation-no-hookon-filtering)
below for what "directly" means. The third test shows the general technique
for forcing a hook's own failure branch deterministically: override a
`TestEnv` world limit (here, `max_state_value_len`) so the exact Hook API
call the hook makes fails, rather than trying to construct byte-level input
that happens to trigger it.

## Assertions API tour

Every accessor below is a method on `TestEnv`, taking `&self` — `World` is
interior-mutable, so you never need a `mut` binding, even across multiple
`invoke` calls on the same env.

- **`state(key) -> Option<Vec<u8>>`** / **`state_typed::<T>(key) -> Option<T>`**
  read this hook's own state (own account, own namespace, or a namespace set
  via `TestEnv::own_namespace`). `None` covers both "no entry" and "the key
  itself is malformed" for `state`; `state_typed` additionally *panics* if
  the entry is present but fails to decode as `T` — a decode failure at
  assertion time is a test-author bug (the wrong value type), not something
  worth silently reporting as absence.
- **`emitted() -> Vec<EmittedTxn>`** is every transaction this env has
  committed via `accept!`, cumulative across every `invoke` call so far.
  `EmittedTxn::blob()` gives the raw bytes for byte-level assertions,
  `EmittedTxn::hash()` the hash `emit` returned, and `EmittedTxn::tx_type()`
  the decoded `TxType` (see [Emitting Transactions](../emit/emitting.md) for
  the shape a `txn_template!`-built blob actually has).
- **`emit_attempts() -> Vec<EmitAttempt>`** is every `emit` call, successful
  or not, cumulative — including attempts made during an invocation that
  ultimately rolled back. `EmitAttempt::outcome` is `Ok(())` (also present
  in `emitted()`) or `Err(EmitFailureReason)` (`NoReserve`,
  `ReserveExceeded`, or `InvalidBlob`).
- **`traces() -> Vec<TraceLine>`** is every `trace`/`trace_num` call this
  env has seen, cumulative, captured rather than printed — inspect it
  explicitly in a test instead of scrolling terminal output.

### Exit types

`invoke` returns a `HookExit { exit: ExitType, code: i64, msg: Vec<u8> }`.
`HookExit::is_success()` is `true` only for `ExitType::Accept`:

| `ExitType` | Where it comes from | World effect | `is_success()` |
|---|---|---|---|
| `Accept` | `accept!(msg, code)` | state writes and this invocation's validated emissions are committed | `true` |
| `Rollback` | `rollback!(msg, code)` | this invocation's state snapshot is restored, its emissions discarded | `false` |
| `Return` | a bare `return code` (no `accept!`/`rollback!`) | **provisionally** treated like `Rollback` (snapshot restored) | `false` |

`ExitType::Return`'s mapping is explicitly marked provisional in the
harness's own doc comments: no live-node evidence yet pins the real
on-chain commit semantics of a bare `return` from a Hook entry (xahaud's own
`ExitType` internally is `UNSET`/`WASM_ERROR`/`ROLLBACK`/`ACCEPT`, with no
documented committed semantics for a plain return). The harness picks the
conservative reading — a test can't pass on a state write that production
might silently discard — and a differential end-to-end test exists
specifically to pin the real behavior; once it lands, this table (and the
harness's mapping, if it turns out to need one) will be updated to match.
Until then, don't write a hook that relies on a bare `return` committing
anything.

## World builders

Every builder below is on `TestEnv`, consumes `self`, and returns `Self` —
call them before the first `invoke`. Everything they set is part of the
*persistent* world and survives across every `invoke` call on the same
`TestEnv`; only what a single invocation itself does (state modification
count, emit reserve, nonce budget, and so on) resets per call.

- **`hook_account(acc)`** — this hook's own account, read back by
  `hook_account()`.
- **`hook_hash(hook_no, hash)`** — the hash of the hook installed at chain
  position `hook_no`, read back by `hook_hash(hook_no)`.
- **`hook_pos(pos)`** — this hook's own position in its chain, read back by
  `hook_pos()`. Chains aren't auto-executed in Phase 1 (every `invoke` runs
  exactly one entry), but a chain is still *representable* this way — useful
  together with `hook_hash`/`grant` for testing foreign-write authorization
  logic that depends on which hook is currently running.
- **`hook_param(name, value)`** — a Hook API parameter attached to this
  hook, read back by `hook_param(name)`.
- **`otxn(Otxn)`** — the originating transaction every `invoke` call sees.
  `Otxn::new(tx_type)` starts one with every field absent; chain
  `.account(acc)`, `.destination(acc)`, `.amount_drops(drops)`, `.param(name,
  value)`, `.id(hash)`, or the general escape hatch `.field_raw(sfield,
  bytes)` for any `sfXxx` code not covered by a dedicated method. Every
  field is stored as its raw value bytes — what `otxn_field` would actually
  write into a caller buffer, no STObject header, no VL length prefix.
- **`otxn_emitted(burden, generation)`** — marks the seeded otxn as itself
  an emitted transaction (seeds `otxn_burden`/`otxn_generation`); absent,
  the otxn models an ordinary non-emitted transaction
  (`otxn_burden() == 1`, `otxn_generation() == 0`).
- **`state_entry(key, value)`** — pre-seeds one of this hook's own state
  entries (own account, own namespace). `key` must be `1..=32` bytes.
- **`foreign_state_entry(ns, acc, key, value)`** — pre-seeds a state entry
  belonging to another `(account, namespace)`.
- **`grant(target_account, ns, authorize)`** — models a `HookGrant` on
  `target_account`'s ledger object, authorizing a hook matched by
  `authorize: Grant` to write into `(target_account, ns)` via
  `state_foreign_set`. `Grant::hook_hash(hash)` matches by the currently
  invoked position's hook hash regardless of account, `Grant::account(acc)`
  by hook account regardless of hash, `Grant::both(hash, acc)` requires
  both, and `Grant::any()` is unconditional. Direction matters: a write to
  this hook's *own* account never consults grants at all — grants only
  gate a write into *another* account's namespace, exactly like the real
  Hook API. Matching is presence-only (no signature verification); anything
  deeper stays end-to-end territory.
- **`ledger_seq(seq)`** / **`ledger_time(t)`** — the current ledger
  sequence and the previous ledger's close time, read back by `ledger_seq()`
  / `ledger_last_time()`.
- **`own_namespace(ns)`** — overrides this hook's own default state
  namespace (`[0u8; 32]` unless called).
- **`max_state_value_len(n)`** — overrides the cap (bytes, default 256,
  matching xahaud's `maxHookStateDataSize` at state scale 1) on a single
  state value; a write over the cap fails, exactly like the real host —
  the technique the worked example above uses to force a deterministic
  `state_set` failure.
- **`base_fee_drops(drops)`** — overrides the per-drop base fee the
  `etxn_fee_base`/`fee_base` approximation multiplies by (default 10 drops;
  see [Fees are an explicit approximation](#what-this-harness-does-not-model)).
- **`strict_can_emit(true)`** — opt-in (default off): after `invoke`,
  asserts every transaction type this invocation committed to `emitted()`
  is one the invoked entry's `#[hook(.., can_emit = [..])]` list declares.
  A violation panics — a test-author assertion, not a Hook API error path.

## Direct-entry invocation: no `HookOn` filtering

`invoke::<C>(index)` is a **direct entry call**: it runs the declared entry
at `index` unconditionally, even if the seeded `Otxn`'s transaction type
would never have triggered that entry on-chain (`#[hook(.., on = [..])]`'s
`HookOn` filtering is not evaluated at all in Phase 1). This is the one
place `TestEnv::strict_can_emit` matters — the entry's own declarations are
still checked for `can_emit`, just not for `on`. Reproducing "does this
chain's `HookOn` routing actually dispatch to the entry I expect" is
end-to-end territory; see [Hook Chains](../concepts/chains.md) for how
`HookOn` is computed from `on = [..]`.

## Phase 1 coverage

The mock backend answers exactly this surface — every other Hook API
function keeps returning `NOT_IMPLEMENTED`, the same as any other native
(non-testenv) build:

| Family | Covered |
|---|---|
| State | `state`, `state_set`, `state_foreign`, `state_foreign_set`, and every as-int64 variant (`state_u64`, `state_foreign_u64`, ...) |
| Originating transaction | `otxn_field`, `otxn_type`, `otxn_id`, `otxn_param`, `otxn_burden`, `otxn_generation` |
| Hook identity | `hook_param`, `hook_account`, `hook_hash(hook_no)`, `hook_pos` |
| Ledger | `ledger_seq`, `ledger_last_time`, `ledger_last_hash`, `ledger_nonce` |
| Fees | `fee_base` (constant) |
| Emission | `etxn_reserve`, `etxn_fee_base`, `etxn_details`, `etxn_burden`, `etxn_generation`, `etxn_nonce`, `emit` |
| Control | `accept`, `rollback` |
| Tracing | `trace`, `trace_num` |

Everything below is a documented **Phase 2** gap, not an oversight — it
always returns `NOT_IMPLEMENTED` under `testenv`, exactly as it does without
`testenv`:

- `float_*` (XFL arithmetic — see [XFL: Decimal Floating Point](../data/xfl.md))
- `slot_*`/`sto_*`/`otxn_slot` (the Slot API and STO codec — see
  [Slots and Ledger Objects](../data/slots.md))
- `util_*` and the `keylet_*` family (see [Keylets](../data/keylets.md))
- `hook_again`/`hook_skip`
- cbak flows: `invoke::<C>(index)` only ever runs an entry's `hook` body,
  never its paired `#[cbak(index)]` — a `#[cbak]`'s function pointer is
  recorded in the same generated table (`NativeEntry::cbak`) so no macro
  rework is needed once cbak testing lands, but there is no way to invoke it
  through this harness yet.
- chain execution / multi-hook ordering (see [Direct-entry invocation](#direct-entry-invocation-no-hookon-filtering)
  above)

## What this harness does not model

Documented, not accidental — each of these stays end-to-end (or
`rshooks build check`) territory:

- **`rshooks::raw` (direct `rshooks_core` calls) bypasses the mock
  entirely.** The backend only intercepts the `rshooks` wrapper layer (the
  `api::*` functions this book otherwise covers); a hook that reaches past
  it into `rshooks_core` directly keeps hitting the real `NOT_IMPLEMENTED`
  host stubs under `testenv`, exactly as on any other native build. This
  is deliberate — `rshooks_core` is documented elsewhere in this book as
  the project's own WCE escape hatch, and that hatch stays real on native
  builds too, rather than silently gaining mock coverage the wasm build
  doesn't have.
- **Statics outside `HookStatic` are not reset between invocations.** An
  ordinary hand-declared `static` keeps its value across every `invoke`
  call in the same process, unlike the fresh-wasm-instance-per-invocation
  reality on-chain. `HookStatic` (see [Emitting Transactions](../emit/emitting.md)'s
  statics idiom) is the one pattern this harness *does* reset correctly: on
  native under `testenv`, `take()` hands out a freshly leaked clone of the
  static's pristine value once per invocation (not once per process), so a
  `HookStatic`-held template like `examples/10_emit-txn`'s `TXN` behaves the
  same on the second `invoke` call as it did on the first. This is why
  `HookStatic`'s payload type must implement `Clone` — an unconditional
  requirement of the type, identical on every target, not something that
  only shows up under `testenv`.
- **No `HookOn` trigger filtering** — covered above.
- **Fees are an explicit approximation**: `etxn_fee_base`'s default
  responder is `base_fee_drops × etxn_burden`, not a parse of the actual
  transaction blob through xahaud's real ledger fee calculator. Override
  `base_fee_drops` if a test needs a specific number, but don't treat the
  result as the real on-chain fee.
- **`ExitType::Return` is provisional** — covered above.
- **Guard enforcement is not native-testable.** `_g` keeps returning `0`
  natively, with or without `testenv`; guard correctness is
  `rshooks build check`'s job (see [The `rshooks` CLI](../build/cli.md)) and
  the end-to-end suite's, not this harness's.
- **Signature verification, real reserve/consensus economics, and instruction
  metering** are unmodeled entirely.

## Where to go next

- [Hook Chains](../concepts/chains.md) covers `HookOn`/`can_emit` and how a
  `#[state(...)]` field declared once is shared across every entry in a
  chain — background for `TestEnv::strict_can_emit` and the `hook_hash`/
  `hook_pos` builders above.
- [Accept, Rollback, and Errors](../concepts/errors.md) covers
  `accept!`/`rollback!`/`hook_errors!` themselves — this page only covers
  how their outcomes surface through `TestEnv::invoke`.
- [Emitting Transactions](../emit/emitting.md) covers `txn_template!` and
  `HookStatic` in full — background for the emission-capture assertions and
  the `HookStatic` reset behavior above.
- `docs/E2E-TESTING.md` (repository root) covers the live-node suite this
  page's harness deliberately does not replace.
