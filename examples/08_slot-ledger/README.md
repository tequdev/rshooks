# slot-ledger

## What you'll learn

How to navigate a transaction's fields through the **typed Slot API**
(`SlotObject::from_otxn` → `.get(sfXxx)` → `.value()`) instead of
`otxn_field` directly — useful once a hook needs to reach into structure
`otxn_field` can't address on its own (arrays, nested objects), and a good
warm-up for those cases even though this example only reads two top-level
scalar fields.

## Code walkthrough

```rust
let txn = SlotObject::from_otxn()?;             // whole otxn → a slot
let dest_slot = txn.get(sfDestination)?;        // navigate to a field
let dest: AccountId = dest_slot.value()?;       // read it out
```

(the real code matches on each `Result` individually and rolls back with a
specific message per failure; shown compressed here with `?` for the
walkthrough.)

**No slot numbers appear anywhere.** `SlotObject::from_otxn()` asks the host
to load the whole originating transaction into an auto-assigned slot and
hands back a `SlotObject<STObject>` holding that number. `.get(key)` derives
a child slot the same way — and the key decides the child's type:
`sfDestination` is an `SField<AccountId>`, so `.get(sfDestination)` yields a
`SlotObject<AccountId>` and `.value()` needs no turbofish and no annotation.
An index (`.get(0u32)`) works on an array and yields a `SlotObject<STObject>`;
using the wrong key kind for the parent is a compile error, not a runtime
surprise.

The handle is **affine**: no `Copy`, no `Clone`, and every operation that
ends the slot's life takes `self`. `.value()` consumes the handle and
deliberately does *not* clear the slot — see "Why there are no `slot_clear`
calls" below.

This example does that twice from the same `txn` handle — `.get()` borrows,
so one parent yields several children: once for `sfDestination` (always
exactly 20 bytes — an `AccountId`), once for `sfAmount`. For `Amount`,
`.size()` is checked *before* reading anything out (`.size()` borrows, which
is exactly why it does — sizing must not spend the handle the read still
needs): it reports the serialized size (8 bytes for a
native amount, 48 for an IOU one) without copying any data, so the actual
read buffer only ever needs to be sized for the native case this example
supports (rejecting an IOU `Amount` as out of scope, rather than always
allocating room for the larger encoding just to check its length after the
fact). Every step returns a `Result`, each handled with its own
[`rshooks::hook_errors!`] rollback code and message.

## Why there are no `slot_clear` calls

`.value()` consumes its handle and leaves the slot loaded. The host frees
every slot when the hook returns, so for a hook that reads a few fields once
and exits, clearing is pure overhead — and it is exactly what the C idiom
costs: a C `slot_subfield` followed by a `slot()` read leaks the slot
identically. Making the typed read clear implicitly would bill every read
for a host call C never pays.

The case that *does* need clearing is a loop deriving a slot per iteration:
255 is the per-execution budget, and `take_value()`/`take_xfl()`/
`take_raw_exact()` read and release in one step for exactly that. A
multi-hop `slot_path!` clears its intermediates automatically.

## Typed vs raw: measured

This example was rewritten from the raw numbered API (`otxn_slot` →
`slot_subfield` → `slot_exact`) to the typed one, and four variants were
built through `rshooks-build` at this workspace's `opt-level = 3`: raw
and typed, each with and without slot clearing. Current values for the
committed variant live in [`metrics.json`](./metrics.json), refreshed by
`mise run record-example-metrics`.

**The no-clears pair is the zero-cost result**, and it is the only pair
that is apples-to-apples: the same host calls, in the same order, with the
same cleanup policy (none). Raw and typed measure **identical** — every
wrapper is `#[inline(always)]` over the same call, so the typed layer adds
no instructions and no bytes at all.

The clearing pair compares the *clearing* variants, and they are **not**
equivalent to each other: `take_*` clears on the failure path as well as the
success path, while the raw code's three `slot_clear` calls sit after every
rollback and so run only on success. The handful of extra instructions
buys that stronger cleanup; it is not overhead the typed layer imposes on
the same behavior. If you want raw's exact semantics through the typed API, read with
`value()`/`raw_exact()` and clear explicitly — that is the first two rows.

Why the committed version clears nothing at all is the previous section.

One more measured note, in the source as a comment: reading the amount with
`.value()` → `AmountBytes` instead of `raw_exact::<8>()` costs +12
instructions, because it reads into a 48-byte buffer (an IOU amount has to
fit) and branches on the length. That is a different operation, not layer
overhead — and it is the right one when the size *isn't* already known.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/08_slot-ledger/Cargo.toml
```

No extra flags needed: every comparison here is a scalar (`usize`)
length check, not a fixed-size array comparison, so there's no
compiler-generated `bcmp`-style loop to guard.

## Expected behavior

- Transaction has no `Destination` field (e.g. not a `Payment`) →
  rollback, code `2`.
- Transaction has a `Destination` but a non-native (IOU) `Amount` →
  rollback (`"unsupported (non-native) Amount"`, code `5`).
- Transaction has both a `Destination` and a native `Amount` → accept, with
  the accept code set to a combination of both fields' first bytes (a
  stand-in for "the values were actually read," not meaningful hook logic
  on its own).

## Error codes

`SlotLedgerError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback!` code for each failure this hook can exit with:

| variant | code | meaning |
|---|---|---|
| `OtxnSlotFailed` | 1 | loading the originating transaction into a slot failed |
| `NoDestinationField` | 2 | the originating transaction has no `Destination` field |
| `UnexpectedDestinationSize` | 3 | `Destination`'s slot didn't serialize to exactly 20 bytes |
| `NoAmountField` | 4 | the originating transaction has no `Amount` field |
| `UnsupportedAmount` | 5 | `Amount` isn't an 8-byte native (XRP/XAH) amount |
| `SlotSizeFailed` | 6 | reading the `Amount` slot's size failed |
| `AmountReadFailed` | 7 | reading `Amount` out of its slot failed after its size already reported the native-amount length |
