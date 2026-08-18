# Macro Reference

A lookup table of every user-facing macro, derive, and attribute `rshooks`
exports. Each entry is a one-line purpose plus a minimal invocation sketch —
for the full grammar, worked examples, and edge cases, follow the link into
the tutorial chapter that covers it, or the macro's own rustdoc in
`crates/rshooks/src/lib.rs`.

## Chain declaration and entry points

| attribute | purpose | sketch |
|---|---|---|
| `#[hooks(description = "...")]` on a struct | Declares this crate's Hook chain — a container for shared `#[state]`/`#[hook_param]`/`#[otxn_param]` fields, no runtime instance. Exactly one per crate. | `#[hooks] pub struct MyHook;` |
| `#[hooks]` on an inherent `impl` | Declares this chain's entry points. Exactly one per `#[hooks]` struct, in the same module. | `#[hooks] impl MyHook { .. }` |
| `#[hook(<index>, ...)]` | Inside a `#[hooks]` impl: declares one Hook entry at the given chain position (`0..=9`, required). Turns a plain `fn name() -> i64` into that index's `hook` export. Named args: `name`, `on`/`on_incoming`+`on_outgoing`, `can_emit`, `description`. | `#[hook(0, name = "accept", on = [Invoke])] fn main() -> i64 { accept!() }` |
| `#[cbak(<index>)]` | Pairs with a `#[hook]` at the same index; exports `cbak` for that index — the optional callback invoked when a transaction this entry emitted later settles. Index only, no other arguments. | `#[cbak(0)] fn my_cbak() -> i64 { accept!() }` |

### Receivers on `#[hook]`/`#[cbak]` entries and impl helpers

An entry function, and a non-attributed helper declared inside the same
`#[hooks] impl`, accept exactly one optional receiver:

| receiver | accepted? |
|---|---|
| none (`fn main() -> i64`) | yes — the pre-existing, still-canonical form for a chain with nothing declared to read |
| `&self` (`fn main(&self) -> i64`) | yes — receives the chain's single zero-sized static by shared reference; reach its fields as `self.<field>` |
| `self` / `mut self` / `self: T` | **no** — diagnostic: "use `&self` — hook entrypoints receive the chain declaration by shared reference (it is zero-sized)" |
| `&mut self` / `&'a mut self` | **no** — diagnostic: "chain handles are zero-sized and immutable; ledger state is accessed through the handles, not by mutating the struct — use `&self`" |

Both accepted forms measure byte-identical wasm; the choice is style, not
performance. Code outside the annotated `impl` (a free function, another
module) has no `self` to borrow and reaches the same static by the
struct's own name instead (`MyHook.some_field`) — see [Anatomy of a
Hook](../concepts/anatomy.md#the-struct-has-no-runtime-instance-but-entries-may-borrow-it).

See [Anatomy of a Hook](../concepts/anatomy.md), [Hook
Chains](../concepts/chains.md), [Per-Hook
Attributes](../build/metadata.md), and [Emitting
Transactions](../emit/emitting.md).

## Control flow & exit

| macro | purpose | sketch |
|---|---|---|
| `accept!` | Terminate successfully, optionally with a message and code. | `accept!()` / `accept!(b"ok", 0)` |
| `rollback!` | Terminate with failure, rolling back state changes. | `rollback!(b"blocked", FirewallError::BlockedAccount)` |
| `guard!` | Bound a loop's iteration count for the host's static guard check. | `loop { guard!(10); .. }` |
| `guard_m!` | Like `guard!`, for multiple loops sharing one source line (`$n` disambiguates). | `guard_m!(10, 0);` |
| `hook_errors!` | Declare a `#[repr(i64)]` error enum usable directly as a `rollback!`/`accept!` code. | `hook_errors! { pub enum E { BlockedAccount = 1 } }` |
| `exit_on_err!` | Unwrap a `Result<T, E: Into<i64>>`, rolling back on `Err`. | `let v = exit_on_err!(b"failed", check());` |

See [Accept, Rollback, and Errors](../concepts/errors.md) and [Guards and
Loops](../concepts/guards.md).

## Data & typing

| macro/attribute | purpose | sketch |
|---|---|---|
| `#[derive(HookData)]` | Encode/decode a fixed-size, named-field struct as a **hook-state value** (or a parameter value, or a nested field). | `#[derive(HookData)] struct Deposit { amount: u64 }` |
| `#[derive(HookKey)]` | Encode a fixed-size, named-field struct as a **hook-state key** (encode-only, 32-byte bound checked at derive time). | `#[derive(HookKey)] struct DepositKey { tag: u8, owner: AccountId }` |
| `#[derive(ParamName)]` | Encode a fixed-size, named-field struct as a **Hook API parameter name** (encode-only, 1–32-byte bound checked at derive time). | `#[derive(ParamName)] struct SeatParamName { topic: u8, seat: u8 }` |
| `#[derive(ParamValue)]` | Decode a fixed-size, named-field struct as a **Hook API parameter value** (decode-only). | `#[derive(ParamValue)] struct Config { min_amount: u64 }` |
| `#[state(key = ...)]` / `#[state(key_by = ...)]` | On a `#[hooks]` struct field of type `State<V>`: declares a hook-state entity — key + value pairing, with `.get()`/`.set()`/`.update()`/`.delete()` (and `.at(args)` for `key_by`). | `#[state(key_by = DepositKey)] deposits: State<Deposit>,` |
| `state_keys!` | Declare an enum of hook-state keys, each variant its own real byte length. | `state_keys! { enum DataKey { Counter, Balance(AccountId) } }` |
| `#[hook_param(name = ...)]` / `#[hook_param(name_by = ...)]` | On a `#[hooks]` struct field of type `HookParam<V>`: declares a Hook parameter (this hook's own installed parameters) — name + value pairing, `.get()`/`.get_or_default()`/`.get_required()` (and `.at(args)` for `name_by`). | `#[hook_param(name = b"CFG", default = Config::default())] config: HookParam<Config>,` |
| `#[otxn_param(name = ...)]` / `#[otxn_param(name_by = ...)]` | Identical grammar to `#[hook_param]`, but reads the *originating transaction's* parameters. | `#[otxn_param(name = b"INS", required)] instruction: OtxnParam<Instruction>,` |

See [Hook State](../data/state.md), [Hook and Transaction
Parameters](../data/parameters.md), and [Typed Data with
Derives](../data/typed-data.md).

## Compile-time literals

| macro | purpose | sketch |
|---|---|---|
| `XFL!` | Encode a decimal numeric literal into a bit-exact `xfl::XFL` value at compile time (never via `f64`). | `const RATE: XFL = XFL!(0.003333333333333333);` |
| `account_id!` | Decode a classic r-address into an `AccountId` at compile time. | `const OWNER: AccountId = account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh");` |

See [XFL: Decimal Floating Point](../data/xfl.md) and
[Keylets](../data/keylets.md) (which covers `account_id!`).

## Transactions

| macro | purpose | sketch |
|---|---|---|
| `txn_template!` | Declare a typed, byte-exact emitted-transaction template: field list in, `new()`/setters/`prepare_for_emit()`/`emit()` out. | `txn_template! { struct Payment { transaction_type = ttPAYMENT, .. } }` |

See [Emitting Transactions](../emit/emitting.md).

## Slots

| macro | purpose | sketch |
|---|---|---|
| `slot_path!` | Walk a multi-hop `SlotObject` path, clearing each intermediate handle as soon as its child exists — no `?`-chain slot leaks. | `slot_path!(root[sfSigners][0][sfAccount])` |

See [Slots and Ledger Objects](../data/slots.md).

## Tracing & buffers

| macro | purpose | sketch |
|---|---|---|
| `trace!` | Emit a debug trace message (and optional byte payload). Compiles to nothing unless the `trace` feature is enabled. | `trace!(b"checkpoint");` |
| `trace_num!` | Emit a trace message followed by an integer. | `trace_num!(b"count", count);` |
| `trace_float!` | Emit a trace message followed by an XFL value. | `trace_float!(b"rate", rate);` |
| `pad!` | Zero-pad a constant byte string to a fixed-size array at compile time, `src` at the front. | `const KEY: StateKey = StateKey(pad!(b"counter"));` |
| `pad_left!` | Same as `pad!`, but `src` at the end (zero bytes first). | `const KEY: StateKey = StateKey(pad_left!(b"counter"));` |

See [Tracing and Debugging](../concepts/tracing.md) and [Hook
State](../data/state.md).

## Build metadata

There's no separate metadata macro in v0.2 — a chain's descriptive and
SetHook-facing metadata is the `#[hooks(description = ...)]` struct
attribute plus each entry's `#[hook(<index>, name = ..., on = ...,
can_emit = ..., description = ...)]` arguments, listed under "Chain
declaration and entry points" above. `rshooks build` extracts them into a
per-entry sidecar JSON and a `SetHook` template — see [Per-Hook
Attributes](../build/metadata.md).
