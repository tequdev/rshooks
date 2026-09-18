# typed-data

## What you'll learn

Declaring a composite hook-state key/value pair and a composite Hook API
parameter name/value pair against a `#[hooks]` struct's `State`/
`HookParam`/`OtxnParam` fields, backed by `#[derive(HookKey)]`/
`#[derive(HookData)]`/`#[derive(ParamName)]`/`#[derive(ParamValue)]` —
instead of hand-packing each into a raw byte buffer. See [Typed Data with
Derives](../../book/src/data/typed-data.md) for those derives, [Hook
State](../../book/src/data/state.md) for `#[state(key_by = ..)]`, and [Hook
and Transaction Parameters](../../book/src/data/parameters.md) for
`#[hook_param(name_by = ..)]` — this README covers only what those pages
don't: this hook's own business rules and wire encoding.

## The hook

A per-account deposit ledger, invoked via `Invoke`. Each call attaches its
own instruction as an `#[otxn_param(..)]` field (`INS`), distinct from the
hook's own installed configuration (`CFG`, an `#[hook_param(..)]` field):

- `deposit` (`action = 1`): rejects if the deposited amount is below the
  configured minimum; otherwise adds it to the sender's balance and
  (re)starts a lock window ending `lock_ledgers` ledgers from now.
- `withdraw` (`action = 2`): rejects if the sender has no outstanding
  deposit, or if the lock window hasn't elapsed yet; otherwise **deletes**
  the sender's record, refunding the owner reserve it was holding.

Each sender's record is looked up by a composite key (a tag byte plus their
`AccountId`, `DepositKey`) and stored as a composite value (an amount, a
deadline ledger sequence, and a flags byte, `DepositValue`) — see
`src/lib.rs` for both struct definitions.

An operator-controlled pause switch (`AdminName`/`PauseSwitch`, a
`#[hook_param(name_by = ..)]` field) can block new deposits without
touching the hook's own code — see [Hook and Transaction
Parameters](../../book/src/data/parameters.md#composite-names-deriveparamname-and-name_by)
for how a composite (struct-shaped) parameter name works.

## Hook parameter hex encoding

`CFG`/`INS`/the pause switch's name all decode as `#[derive(ParamValue)]`/
`#[derive(ParamName)]` structs, so their wire layout is "every field, in
declaration order, little-endian, back-to-back."

`Config { min_amount: u64, lock_ledgers: u32 }` — 12 bytes. For
`min_amount = 5,000,000` drops (5 XAH), `lock_ledgers = 20`:

```json
{ "HookParameter": { "HookParameterName": "434647", "HookParameterValue": "404B4C000000000014000000" } }
```

(`434647` is `CFG` in ASCII hex.) Omitting `CFG` falls back to the
compiled-in default (1 XAH minimum, a 10-ledger lock).

`Instruction { action: u8, amount: u64 }` — 9 bytes, attached to the
`Invoke` transaction's own `HookParameters` (not the `SetHook`'s). For a
`deposit` of 6,000,000 drops (6 XAH):

```json
{ "HookParameter": { "HookParameterName": "494E53", "HookParameterValue": "01808D5B0000000000" } }
```

A `withdraw` needs no meaningful `amount` but the field must still be
present (`02` + 8 zero bytes).

`AdminName { section: 0, field: 0 }` — 2 bytes — pairs with `PauseSwitch {
paused: u8 }` — 1 byte. Installed at `SetHook` time (an administrative
control, not a per-transaction instruction):

```json
{ "HookParameter": { "HookParameterName": "0000", "HookParameterValue": "01" } }
```

Omitting this parameter (or `HookParameterValue: "00"`) leaves deposits
unpaused; a *present* value of the wrong size is a decode failure, not
"unpaused" (see "Expected behavior").

## Zero-cost, measured

[Typed Data with Derives](../../book/src/data/typed-data.md#the-zero-cost-claim-measured-not-assumed)
covers the derived-vs-hand-packed measurement for this hook's core
deposit-ledger logic; current numbers live in [`metrics.json`](./metrics.json).
The `AdminName` composite parameter name (as opposed to a plain
byte-string tag like `CFG`/`INS`) costs strictly more than the plain `CFG`
tag used elsewhere in this same hook, since a composite name has to
actually run its `write()` at runtime — see [Hook and Transaction
Parameters](../../book/src/data/parameters.md#composite-names-deriveparamname-and-name_by)
for the measured difference.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/12_typed-data/Cargo.toml --out examples/12_typed-data/out
```

No extra flags needed — every derive-generated accessor is guard-clean at
the source level by construction.

## Expected behavior

- No `INS` parameter (or the wrong size) on the `Invoke` → rollback.
- `deposit` below the configured (or default) minimum → rollback.
- `deposit` at or above the minimum → accept; the account's stored balance
  increases by the deposited amount and the lock window resets.
- `withdraw` with no outstanding deposit → rollback.
- `withdraw` before the lock window elapses → rollback.
- `withdraw` after the lock window elapses → accept; the account's record
  is **deleted** (not zeroed in place), refunding its owner reserve. A
  subsequent `withdraw` then rolls back as "nothing to withdraw."
- `action` anything other than `1`/`2` → rollback.
- `CFG` present but not exactly 12 bytes → rollback — never silently falls
  back to the compiled-in default.
- `deposit` while the pause switch is set → rollback.
- `deposit` while the pause switch is present but not exactly 1 byte →
  rollback — never silently treated as unpaused.
- `deposit`/lock-window arithmetic that would overflow → rollback.

Failure/rollback codes are declared on `TypedDataError` in `src/lib.rs`
(`rshooks::hook_errors!`) — each variant's doc comment states which step
it corresponds to.
