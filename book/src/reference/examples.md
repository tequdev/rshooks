# Examples Index

`examples/` is a runnable catalog of Hooks written with `rshooks`, built
with `rshooks` (from the `rshooks-build` package) — its own Cargo workspace, separate from the root
workspace, because these crates are `no_std` `cdylib`s with a Hook-specific
release profile that must not leak into `rshooks-core`/`rshooks`/
`rshooks-build`, and they don't build for host targets. Every code sample in
this book is adapted from one of these.

## Reading order: 01–16

Numbered in **suggested reading order** — start at `01_accept-all` and work
down; each one builds on ideas from the examples before it. The `example`
column is each crate's actual package name (Cargo package names can't start
with a digit, so only the directory is prefixed).

| # | example | demonstrates | book chapter |
|---|---|---|---|
| 01 | `accept-all` | minimal hook: `accept` everything (starter template) | [Anatomy of a Hook](../concepts/anatomy.md) |
| 02 | `state-counter` | `state`/`state_set` round-trip, counter in hook state | [Hook State](../data/state.md) |
| 03 | `hook-params` | `#[hook_param]`-configurable threshold, with a compiled-in default | [Hook and Transaction Parameters](../data/parameters.md) |
| 04 | `errors` | a meaningful `hook_errors!`-based rollback error-code system, matched to `HookReturnCode` | [Accept, Rollback, and Errors](../concepts/errors.md) |
| 05 | `firewall` | read `otxn_field(sfAccount)` + a hook parameter blacklist → `rollback` | [Reading the Originating Transaction](../data/otxn.md) |
| 06 | `guard-patterns` | `guard!`/`guard_m!` correctness, choosing `maxiter`, and the array-`==` memcmp-loop pitfall | [Guards and Loops](../concepts/guards.md) |
| 07 | `xfl-math` | reading `Amount` as XFL (`slot_float`/`sto_set`), `mulratio`, checked `Add`/`Sub`/`Mul`/`Div`/`Neg` operators, `.compare()`-family methods, and `XFLUnchecked`'s hot-path chain | [XFL: Decimal Floating Point](../data/xfl.md) |
| 08 | `slot-ledger` | the typed slot layer: `SlotObject::from_otxn()` → `.get(sfXxx)` → `.value()`, with no slot numbers in sight, measured against the raw numbered API it replaced | [Slots and Ledger Objects](../data/slots.md) |
| 09 | `state-foreign` | `state_foreign`: reading another (hook-parameter-configured) account's hook state | [Hook State](../data/state.md) |
| 10 | `emit-txn` | `etxn_reserve` + a `txn_template!`-declared Payment/`emit`, with a `cbak` | [Emitting Transactions](../emit/emitting.md) |
| 12 | `typed-data` | `#[derive(HookData)]`: composite (multi-field) state keys/values and `otxn_param`/`hook_param` structs, in place of hand-packed byte buffers | [Typed Data with Derives](../data/typed-data.md) |
| 13 | `keylets` | `rshooks::api::keylet`'s 26 typed `keylet_xxx` helpers (one per `KEYLET_*` constant), in place of the single untyped `util_keylet` | [Keylets](../data/keylets.md) |
| 14 | `account-id-macro` | `rshooks::account_id!`: compile-time r-address → `AccountId` decode, cross-checked against `hook_account`/`util_accid`/`util_raddr` | [Reading the Originating Transaction](../data/otxn.md) |
| 15 | `slot-objects` | the typed slot layer's live acceptance harness: account-root walk, native-amount drops round-trip, parent-clear/child-read, and two 300-iteration loops proving `take_*` recycling and leak-free `slot_path!` failures | [Slots and Ledger Objects](../data/slots.md) |
| 16 | `typed-results` | typed entry returns (`HookResult`): an idiomatic `?`/`Ok` entry with a `hook_errors!` message clause, alongside a raw `accept!`/`rollback!`-style entry in the same chain | [Accept, Rollback, and Errors](../concepts/errors.md#typed-entry-returns-hookresult) |
| 18 | `param-signature` | the Hook Parameter Signature Interface: `#[hook(..)]` fn arguments (`increment(account: AccountID, count: UInt16)`) as declared, typed, machine-readable Hook parameters | [Hook and Transaction Parameters](../data/parameters.md#signature-parameters-fn-arguments) |

There is no `11` in the numbering — the numbering follows the historical
example order, with gaps where an example was retired. `17_sto-writer`
exists in `examples/` (see its own README) but has no book chapter yet.

## 80+: production hooks in Rust

Unlike `01`–`16` (one concept each, in suggested reading order), the `80`+
series are behavior-equivalent Rust ports of real, deployed xahaud C hooks —
read them after `01`–`16`, not instead of them. Each has its own README with
a full behavior-equivalence table against its C source and a differences
table for any intentional deviation.

| # | example | ports | book chapter |
|---|---|---|---|
| 80 | `governance` | **One crate, two chain positions** — [`hook/genesis/govern.c`](https://raw.githubusercontent.com/Xahau/xahaud/dev/hook/genesis/govern.c) at `Hooks[0]` (the 20-seat L1/L2 round-table governance state machine) and [`hook/genesis/reward.c`](https://raw.githubusercontent.com/Xahau/xahaud/dev/hook/genesis/reward.c) at `Hooks[1]` (computes and emits a `GenesisMint` crediting `ClaimReward` claimants and active-validator L1 seats), sharing one `#[hooks]` struct's state schema | [Hook Chains](../concepts/chains.md) |

`80_governance` is also this book's worked example for the multi-Hook
chain model's real nesting-budget limit and its raw-API escape hatch — see
[Hook Chains](../concepts/chains.md#a-real-limit-typed-accessor-density-inside-one-entry)
and the example's own `README.md`.

## Building

Build every example (this is also the toolchain's own end-to-end test: each
one is built via `cargo run -p rshooks-build -- build ...` from the root
workspace, and the resulting `out/<name>.wasm` is re-validated with
`rshooks check`):

```sh
mise run build-examples
```

Build a single example directly:

```sh
cargo run -p rshooks-build -- build --manifest-path examples/02_state-counter/Cargo.toml
```

See [The rshooks CLI](../build/cli.md) for the CLI itself, and each
example's own README for its exact command.

## E2E tests

`e2e/` deploys the examples' `rshooks-build` output to a real, standalone
`xahaud` node (via `SetHook`) and asserts on the resulting transaction
metadata and ledger state — proof of runtime behavior, not just that the
binaries are SetHook-valid.
