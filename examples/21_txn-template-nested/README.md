# txn-template-nested

## What you'll learn

`txn_template!`'s fixed-shape nested containers: `object(sfX) { .. }` for a
declared `STObject` and `array(sfX) [ .. ]` for a declared `STArray` (see
`crates/rshooks/src/txn.rs`'s `txn_template!` doc comment, "Nested
containers"). This hook emits a Remit whose `sfAmounts` field is a fixed,
two-entry array — one `native_amount` entry, one `amount` (48-byte issued)
entry with a baked currency/issuer default — declared entirely inside the
template, with no `StoWriter` call anywhere. Contrast
`examples/17_sto-writer`'s Remit, whose second `sfAmounts` entry is only
present *conditionally*, based on hook parameters supplied at runtime:
that shape isn't known at compile time, so it needs `StoWriter`.
`txn_template!`'s nested containers are for the opposite case — every
entry, and its shape, is fixed by the declaration alone. `main` also
exercises both `amount` setters on the real `wasm32v1-none` target — the
8-byte `_value` hot path by default, and the full 48-byte setter when an
`ISSUER` hook parameter is present — so `rshooks build`/`check` covers the
`[u8; 48]` build-and-copy path too, not just the 8-byte one.

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
        amounts: array(sfAmounts) [
            native: object(sfAmountEntry) {
                amount: native_amount(sfAmount) = 1,
            },
            usd: object(sfAmountEntry) {
                amount: amount(sfAmount) = (XFL::from_raw_bits(0), USD, USD_ISSUER),
            },
        ],
        emit_details: emit_details,
    }
}
```

Setter names splice the full declaration path: the native entry's amount
gets `set_amounts_native_amount(u64) -> Result<()>`, and the issued
entry's gets two setters — `set_amounts_usd_amount(xfl, &currency,
&issuer)` (all 48 bytes) and `set_amounts_usd_amount_value(xfl)` (just the
8-byte value, keeping the baked `USD`/`USD_ISSUER` default). `main` calls
both, on two different paths:

```rust
match hook_param_exact::<AccountId>(b"ISSUER") {
    Ok(issuer) => txn.set_amounts_usd_amount(XFL::one(), &USD, &issuer),
    Err(_) => txn.set_amounts_usd_amount_value(XFL::one()),
}
```

Without an `ISSUER` hook parameter, the currency and issuer stay at their
baked default and only the 8-byte value changes — a single store, no host
call. With one, the issued entry's currency and issuer are rewritten too,
through the full 48-byte setter — this is the same `[u8; 48]` build-and-copy
`rshooks::sto_writer::StoWriter`'s `iou_amount` writes on every call
(`examples/17_sto-writer`); exercising it here, on the real
`wasm32v1-none` target, is what lets `rshooks build`/`check` catch a
future compiler-generated copy loop over that region before it reaches a
live node.

`main` reserves one emission slot, reads the required `DEST` hook
parameter (a 20-byte `AccountId`; rolls back if absent), sets the
destination and both `sfAmounts` entries, then `prepare_for_emit()`/
`Prepared::emit()` — the same two-call lifecycle every `txn_template!`
type has (`examples/10_emit-txn`).

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
an in-crate `#[cfg(test)]` module at the bottom of `src/lib.rs`. Both cover
the accept-and-emit path, the `DEST`-missing rollback, `cbak`, and the
`ISSUER`-parameter override — asserting the emitted blob's issued
`sfAmountEntry` carries the overridden issuer while the currency stays the
baked `USD`. The in-crate module additionally exercises the private
`Remit` type directly, since it isn't reachable from an integration test:

- a byte-exact check of the whole `sfAmounts` region — headers (derived
  via `txn::codec::field_header`, not hardcoded), the native entry's
  1-drop value, and the issued entry's `XFL::one()` value bytes plus the
  baked `USD`/`USD_ISSUER` currency and issuer;
- a cross-check against `rshooks::sto_writer::StoWriter`: the identical
  fixed-prefix bytes, built once through `txn_template!`'s setters and
  once through `StoWriter::iou_amount` against a hand-written
  `float_sto` that assembles `STAmount`'s issued value component by
  component (sign, biased exponent, mantissa) the way xahaud's `float_sto`
  does, rather than through the bit-OR identity `txn::codec` relies on,
  asserted equal over `Remit::LEN - EMIT_DETAILS_MAX_LEN` bytes.

## Error codes

`TxnTemplateNestedError` (`rshooks::hook_errors!`, see `src/lib.rs`) is
the `rollback!` code for each failure this hook can exit with:

| variant | code | meaning |
|---|---|---|
| `ReserveFailed` | 1 | `etxn_reserve(1)` failed to reserve an emission slot |
| `MissingDestination` | 2 | the `DEST` hook parameter was missing or not a 20-byte `AccountId` |
| `BufferAlreadyTaken` | 3 | the static `Remit` template had already been `take()`n |
| `SetAmountFailed` | 4 | the native entry's amount setter failed (out of `u64` drops range) |
| `PrepareFailed` | 5 | `prepare_for_emit` failed to fill in the host-supplied fields |
| `EmitFailed` | 6 | the prepared transaction could not be emitted |

## Cost

Current WCE, wasm size, and max nesting depth live in
[`metrics.json`](./metrics.json), refreshed by `mise run
record-example-metrics`.

| | WCE (hook / cbak) | size | max nesting |
|---|---|---|---|
| `txn-template-nested` (this example) | 487 / 7 | 1812 bytes | 3 |
| `sto-writer` | 703 / 7 | 2224 bytes | 5 |

Below `examples/17_sto-writer`'s WCE and size, at a shallower nesting
depth, for an equivalent-shaped Remit — even with `main` reading a second
hook parameter (`ISSUER`) and branching between the 8-byte `_value` hot
path and the full 48-byte `amount` setter, both of which `StoWriter` still
has to beat on top of its own bounds/duplicate checks and conditional
issued-entry branch. The baked-issuer path alone (no `ISSUER` parameter,
so only `set_amounts_usd_amount_value` runs) costs 349 WCE / 1443 bytes /
nesting 2 — the `ISSUER`-parameter read plus the full 48-byte setter add
138 WCE and 369 bytes over that baked-only cost.
