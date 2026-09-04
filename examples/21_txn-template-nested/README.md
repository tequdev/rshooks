# txn-template-nested

## What you'll learn

`txn_template!`'s homogeneous indexed array form: `array(sfX) [ Elem:
object(sfY) { .. } ; N ]` for a fixed-count `STArray` whose elements all
share one declared shape (see `crates/rshooks/src/txn.rs`'s
`txn_template!` doc comment, "Homogeneous arrays", and
`docs/TXN_TEMPLATE_FIELDS_DESIGN.md` §2.5), plus `fixed_vl(sfX, N)` for a
`VL` field whose length — and so its rippled length prefix — is fixed at
declaration time (§2.6). This hook emits a Remit whose `sfAmounts` field
is two back-to-back copies of one issued `amount` entry, declared once
and repeated, and an `sfMemos` field holding one memo with two `fixed_vl`
fields — with no `StoWriter` call anywhere. Contrast
`examples/17_sto-writer`'s Remit, whose second `sfAmounts` entry is only
present *conditionally*, based on hook parameters supplied at runtime:
that shape isn't known at compile time, so it needs `StoWriter`.
`txn_template!`'s indexed arrays are for the opposite case — the element
*count*, and every element's shape, are both fixed by the declaration
alone; only the per-element *values* are filled in at runtime, through a
runtime-indexed accessor. `main` also exercises both `amount` setters on
the real `wasm32v1-none` target — the 8-byte `_value` hot path by
default, and the full 48-byte setter when an `ISSUER` hook parameter is
present — so `rshooks build`/`check` covers the `[u8; 48]` build-and-copy
path too, not just the 8-byte one.

## Code walkthrough

```rust
txn_template! {
    struct Remit {
        transaction_type = ttREMIT,
        flags: u32_field(sfFlags) = tfCANONICAL,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        destination: account_id(sfDestination),
        memos: array(sfMemos) [
            Memo: object(sfMemo) {
                memo_type: fixed_vl(sfMemoType, 4) = *b"note",
                memo_data: fixed_vl(sfMemoData, 8),
            }; 1
        ],
        amounts: array(sfAmounts) [
            AmountEntry: object(sfAmountEntry) {
                amount: amount(sfAmount) = (XFL::from_raw_bits(0), USD, USD_ISSUER),
            }; 2
        ],
        emit_details: emit_details,
    }
}
```

`sfMemos` (canonical code `(15, 9)`) sorts before `sfAmounts` (`(15, 92)`),
so it's declared first — canonical order is checked per container, same as
every other kind. `fixed_vl(sfMemoType, 4) = *b"note"` bakes both the VL
length prefix (a single byte, since `4 <= 192`) and the four-byte payload
into `Memo::TEMPLATE`; `memo_data`'s declaration has no `=`, so its 8-byte
payload defaults to zero and only its length (and so its prefix) is fixed.
`empty_vl` stays the one spelling for an *empty* blob — `signing_pub_key`
above is `empty_vl(sfSigningPubKey)`, not `fixed_vl(sfSigningPubKey, 0)`,
which the macro rejects (`N` must be at least 1).

The `; 2` after the element's field list is the whole of the indexed-array
syntax: it declares the element shape once (`AmountEntry`, an issued
`amount` field with the baked `USD`/`USD_ISSUER` default) and reserves two
back-to-back copies of it. This generates a standalone view type
`AmountEntry<'a>` — with the *same* `set_amount(xfl, &currency, &issuer)`
(all 48 bytes) / `set_amount_value(xfl)` (just the 8-byte value, keeping
the baked default) setters a plain field of that shape would get — plus a
runtime-indexed accessor on `Remit`: `fn amounts(&mut self, index: usize)
-> Option<AmountEntry<'_>>`, `None` for `index >= 2`.

`main` fills the two entries one after the other, each through the
accessor:

```rust
let Some(mut first) = txn.amounts(0) else { /* rollback */ };
let Ok(one) = XFL::new(0, 1) else { /* rollback */ };
match issuer {
    Some(iss) => first.set_amount(one, &USD, &iss),
    None => first.set_amount_value(one),
}

let Some(mut second) = txn.amounts(1) else { /* rollback */ };
let Ok(two) = XFL::new(0, 2) else { /* rollback */ };
match issuer {
    Some(iss) => second.set_amount(two, &USD, &iss),
    None => second.set_amount_value(two),
}
```

`XFL::new(0, 1)`/`XFL::new(0, 2)` compute `1.0`/`2.0` via the host
`float_set` call. Without an `ISSUER` hook parameter, each entry's
currency and issuer stay at their baked default and only the 8-byte value
changes — a single store, no host call. With one, each entry's currency
and issuer are rewritten too, through the full 48-byte setter — this is
the same `[u8; 48]` build-and-copy `rshooks::sto_writer::StoWriter`'s
`iou_amount` writes on every call (`examples/17_sto-writer`); exercising
it here, on the real `wasm32v1-none` target, is what lets `rshooks
build`/`check` catch a compiler-generated copy loop over that region
before it reaches a live node. The accessor takes a runtime index, so a
larger array can equally be filled from a `guard!`-bounded loop (see
[Emitting Transactions](../../book/src/emit/emitting.md)); with two
entries, straight-line code is the simpler read.

The single `sfMemos` entry is filled the same way, through
`Remit::memos`'s accessor:

```rust
let Some(mut memo) = txn.memos(0) else { /* rollback */ };
memo.set_memo_data(b"rshooks!");
```

`memo_type` is never touched at runtime — it stays at the `*b"note"`
default baked into `Memo::TEMPLATE`. `set_memo_data` is `fixed_vl`'s
setter: `fn set_x(&mut self, value: &[u8; N])`, an infallible fixed-size
store at a compile-time-proven offset, no different in spirit from
`account_id`'s or `hash256`'s setters — the VL length prefix (`0x08`
here) isn't writable at all, since the field's length can't change.

Both hook parameters are declared on the chain struct (`#[hook_param(name
= b"DEST", required)]` / `#[hook_param(name = b"ISSUER")]`, see
`examples/03_hook-params`) and read through `self.hook_param`.

`main` reserves one emission slot, reads the required `DEST` hook
parameter (a 20-byte `AccountId`; rolls back if absent), sets the
destination, both `sfAmounts` entries, and the `sfMemos` entry's
`memo_data`, then `prepare_for_emit()`/`Prepared::emit()` — the same
two-call lifecycle every `txn_template!` type has (`examples/10_emit-txn`).

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/21_txn-template-nested/Cargo.toml
```

No extra flags needed. `TXN` (the reusable `Remit` template) is a
`HookStatic`, the same static-template idiom `10_emit-txn`'s README
describes.

## Unit tests

```sh
cargo test --manifest-path examples/Cargo.toml -p txn-template-nested
```

Two equivalent layouts exercise the real `TxnTemplateNested` entry through
`rshooks_testenv::TestEnv::invoke` — no wasm build, no node:
`tests/remit.rs` (an integration test against the crate as a library) and
an in-crate `#[cfg(test)]` module at the bottom of `src/lib.rs`. Both
cover:

- the accept-and-emit path, the `DEST`-missing rollback, and `cbak`;
- a byte-exact check of the whole `sfAmounts` region — headers (derived
  via `txn::codec::field_header`, not hardcoded) and both entries' value
  bytes (`XFL::new(0, 1)`/`XFL::new(0, 2)`, hand-derived from the XFL bit
  layout) against the baked `USD`/`USD_ISSUER` currency and issuer;
- a byte-exact check of the whole `sfMemos` region — `memo_type`'s baked
  `*b"note"` default and `memo_data`'s runtime-written `b"rshooks!"`,
  each with its `fixed_vl` length prefix, headers again via
  `txn::codec::field_header`;
- the `ISSUER`-parameter override — asserting **both** emitted
  `sfAmountEntry` images carry the overridden issuer while the currency
  stays the baked `USD`.

The in-crate module additionally cross-checks the private `Remit` type
directly against `rshooks::sto_writer::StoWriter`, since that's only
reachable from an in-crate test: the identical fixed-prefix bytes, built
once through `txn_template!`'s setters and once through
`StoWriter::iou_amount`/`StoWriter::vl` against a hand-written `float_sto`
that assembles `STAmount`'s issued value component by component (sign,
biased exponent, mantissa) the way xahaud's `float_sto` does, rather than
through the bit-OR identity `txn::codec` relies on — plus a hand-written
`float_set` reimplementing XFL's integer-mantissa normalization
independently, so `XFL::new` inside that same test resolves through the
mock rather than the host stub — asserted equal over `Remit::LEN -
EMIT_DETAILS_MAX_LEN` bytes.

## Error codes

`TxnTemplateNestedError` (`rshooks::hook_errors!`, see `src/lib.rs`) is
the `rollback!` code for each failure this hook can exit with:

| variant | code | meaning |
|---|---|---|
| `ReserveFailed` | 1 | `etxn_reserve(1)` failed to reserve an emission slot |
| `MissingDestination` | 2 | the `DEST` hook parameter was missing or not a 20-byte `AccountId` |
| `BufferAlreadyTaken` | 3 | the static `Remit` template had already been `take()`n |
| `AmountsIndexOutOfRange` | 4 | an `sfAmounts` index was out of range — unreachable by construction (both indexes are literals below the declared count), kept only because the accessor returns `Option` |
| `AmountValueFailed` | 5 | `XFL::new` failed to normalize an entry's value |
| `MemosIndexOutOfRange` | 6 | the `sfMemos` index was out of range — unreachable by construction (the index is a literal below the declared count), kept only because the accessor returns `Option` |
| `PrepareFailed` | 7 | `prepare_for_emit` failed to fill in the host-supplied fields |
| `EmitFailed` | 8 | the prepared transaction could not be emitted |

## Cost

Current WCE, wasm size, and max nesting depth live in
[`metrics.json`](./metrics.json), refreshed by `mise run
record-example-metrics`. `examples/17_sto-writer`'s `metrics.json` is the
natural comparison, but the two hooks do different work: `StoWriter` pays
bounds/duplicate checks on every field plus its conditional issued-entry
branch, while here every setter is a fixed-offset store but `main` reads
two hook parameters, computes each value through `XFL::new`, and carries
a rollback branch per accessor and per value. Read the two files side by
side rather than expecting either to win on every axis.
