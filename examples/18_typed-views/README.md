# typed-views

## What you'll learn

`rshooks::views`: generated, type-checked read views over the originating
transaction and over ledger objects. This hook gates incoming IOU payments
using three of them — `views::tx::Payment`, `views::ledger::RippleState`
and `views::ledger::AccountRoot`. See [Typed Views](../../book/src/data/views.md)
for the full walkthrough of this crate's policy, the low/high trust-line
side determination, the "fewer host calls is not the same as fewer
instructions" measurement, and the per-accessor cost table.

## Specific to this example

`hook_is_low_side` and `issuer_charges_fee` are `#[inline(never)]`: both
are only reached on rejection paths, and keeping their `match` ladders out
of line keeps them out of `hook()`'s own nesting budget — the same escape
hatch `examples/15_slot-objects` and `examples/80_governance` use.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/18_typed-views/Cargo.toml
```

No extra flags are required.

## Unit tests

```sh
cargo test --manifest-path examples/Cargo.toml -p typed-views
```

`tests/typed_views.rs` drives the real `TypedViews` entry through
`rshooks_testenv::TestEnv::invoke` — no wasm build, no node — against a
seeded trust line and issuer account, covering every branch: the native
short-circuit, the missing tag, the missing line, a wrong-typed object at
the line's keylet, both freeze directions from both sides, an absent /
unit / non-unit transfer rate, an unfunded issuer, and a non-`Payment`
originating transaction.

Keylets are recomputed independently in the test (`sha512Half(ledgerSpace
++ args)`) rather than called through `rshooks::api::keylet`, which needs a
live backend — the same two-tier pattern `examples/13_keylets` and
`examples/15_slot-objects` use.

## Error codes

`ViewError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the `rollback!`
code for each failure this hook can exit with:

| variant | code | meaning |
|---|---|---|
| `NotAPayment` | 1 | the originating transaction is not a `Payment` |
| `MissingAmount` | 2 | `sfAmount` could not be read (malformed transaction) |
| `MissingDestinationTag` | 3 | an IOU payment whose `sfDestinationTag` is missing or unreadable |
| `NoHookAccount` | 4 | `hook_account` failed |
| `KeyletFailed` | 5 | a keylet could not be built |
| `NoTrustLine` | 6 | no usable `RippleState` between this account and the issuer: absent, not an `ltRIPPLE_STATE`, or its type field unreadable |
| `NoLineFlags` | 7 | the line's `sfFlags` could not be read |
| `FrozenByUs` | 8 | this account froze the line |
| `FrozenByCounterparty` | 9 | the counterparty froze the line |
| `NoIssuerAccount` | 10 | no usable `AccountRoot` for the issuer: unfunded, not an `ltACCOUNT_ROOT`, or its type field unreadable |
| `IssuerChargesFee` | 11 | the issuer's `sfTransferRate` is above 1.0 |
