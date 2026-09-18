# slot-ledger

## What you'll learn

Navigating a transaction's fields through the **typed Slot API**
(`SlotObject::from_otxn()` → `.get(sfXxx)` → `.value()`) instead of
`otxn_field` directly, with no slot numbers anywhere in hook source. See
[Slots and Ledger Objects](../../book/src/data/slots.md) for the type
itself, why it costs nothing over the raw numbered API, and why the raw
numbered functions are kept out of the prelude.

## Specific to this example

This hook reads `sfDestination` and `sfAmount` off the originating
transaction and rejects a non-native (IOU) `Amount` as out of scope. Before
reading `Amount` out, it checks `.size()` — which borrows rather than
consuming the handle — so the read buffer only ever needs to be sized for
the native case this example supports, rather than always allocating room
for the larger IOU encoding just to check its length after the fact.

The committed hook clears no slots at all (the host frees every slot when
the hook returns anyway); see [Slots and Ledger Objects](../../book/src/data/slots.md#measured-typed-vs-raw)
for the measured typed-vs-raw-vs-clearing comparison this example was
built four ways to produce.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/08_slot-ledger/Cargo.toml
```

No extra flags needed: every comparison here is a scalar (`usize`) length
check, not a fixed-size array comparison, so there's no
compiler-generated `bcmp`-style loop to guard.

## Expected behavior

- Transaction has no `Destination` field (e.g. not a `Payment`) →
  rollback.
- Transaction has a `Destination` but a non-native (IOU) `Amount` →
  rollback.
- Transaction has both a `Destination` and a native `Amount` → accept, with
  the accept code set to a combination of both fields' first bytes (a
  stand-in for "the values were actually read," not meaningful hook logic
  on its own).

Failure/rollback codes are declared on `SlotLedgerError` in `src/lib.rs`
(`rshooks::hook_errors!`) — each variant's doc comment states which step
it corresponds to.
