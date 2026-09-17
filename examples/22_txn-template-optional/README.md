# txn-template-optional

## What you'll learn

`txn_template!`'s NOP-padded `optional` field kinds
(`docs/NOP_PADDING_DESIGN.md`), written entirely in the *inferred*
(bare-`sfX`) style `docs/TXN_TEMPLATE_FIELDS_DESIGN.md` describes:
`optional sfX` (any inferable scalar kind), and `any_amount` via the `=
AnyAmount()` default-shape marker (the one kind here that cannot infer
from its serialized type alone, since `STI_AMOUNT` also covers
`native_amount`/`amount`). Both keep their byte offset compile-time-fixed
regardless of whether they're present at runtime — absence is encoded as
`0x99` (`NOP`) bytes in field-header position, which xahaud's
`STObject`/`STArray` deserializer skips on the emit path (never on
`sto_*`).

The scenario: a Remit that sends one or two amounts, with an `optional`
`DestinationTag`. Every field is legal for a Remit per
`crates/rshooks-core/protocol_formats.json`. The other NOP-padded kinds
`docs/NOP_PADDING_DESIGN.md` introduces — `vl`/`optional vl`,
whole-container `optional` views, and a homogeneous array of `optional`
elements — don't fit this single, protocol-legal scenario alongside
`amounts` without reaching for an unrelated `sfcode`; they're covered by
`crates/rshooks/src/txn.rs`'s `mod tests` twinned fixtures and
`crates/rshooks/tests/ui/{pass,fail}` instead.

## Code walkthrough

```rust
txn_template! {
    struct Remit {
        transaction_type = ttREMIT,
        sequence: sfSequence = 0,
        destination_tag: optional sfDestinationTag,
        first_ledger_sequence: sfFirstLedgerSequence = 0,
        last_ledger_sequence: sfLastLedgerSequence = 0,
        fee: sfFee = NativeAmount(0),
        signing_pub_key: sfSigningPubKey = [],
        account: sfAccount,
        destination: sfDestination,
        amounts: sfAmounts [
            sfAmountEntry { amount: sfAmount = AnyAmount() },
            optional sfAmountEntry { amount: sfAmount = AnyAmount() },
        ],
        emit_details: emit_details,
    }
}
```

- `destination_tag: optional sfDestinationTag` — absent by default (its
  whole `header + 4-byte value` slot is `NOP`s); `DTAG` present routes
  through the generated `set_destination_tag(u32)`. Container charge: `5`
  (the slot's own byte count, well under the top level's 63-`NOP`
  budget).
- `amounts: sfAmounts [ sfAmountEntry { .. }, optional sfAmountEntry
  { .. } ]` — an array (not a homogeneous, indexed one) whose two
  elements are each numbered by their zero-based position: `amounts.0`'s
  `amount: sfAmount =
  AnyAmount()` is always present, flattened as
  `set_amounts_0_amount_native(u64) ->
  Result<()>`/`set_amounts_0_amount_iou(xfl, &currency, &issuer)`;
  `remit` always writes a real, constructible amount into it (`AMT1`, or
  `1` drop by default) rather than leaving it at `any_amount`'s raw
  native-zero encoding default — a required element, unlike an `optional`
  one, has no "absent" state to fall back to. `amounts.1` is a whole
  `optional` container with no view type: its own `amount` field is a
  plain `set_amounts_1_amount_native(u64) ->
  Result<()>`/`set_amounts_1_amount_iou(xfl, &currency, &issuer)` pair
  directly on `Remit`, exactly like `amounts.0`'s — calling either one
  makes `amounts.1` present as a side effect (`remit` calls one only when
  `AMT2` is supplied); `clear_amounts_1()`/`is_amounts_1_present()` round
  out the trio. A homogeneous array can't express "one required, one that
  may or may not be there" (its elements share one presence rule), and
  two fully `optional` `sfAmountEntry` elements (each `header(2) +
  any_amount(sfAmount)`'s 49-byte slot + terminator(1) = 52 bytes;
  `2 * 52 = 104 > 63`) wouldn't fit the array's own budget anyway — one
  required (`0` charge, always writes) plus one `optional` (`52 <= 63`)
  does.

Container budget: `amounts` itself charges the top level nothing (arrays
are always-present containers); `amounts.1`'s own charge (its whole
reserved slot, `52` bytes) lands against `amounts`'s own 63-NOP budget
instead, where it fits comfortably alone.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/22_txn-template-optional/Cargo.toml
```

One artifact: `0.remit.wasm` (`#[hook(0, ..)]`) — no `cbak` beyond a bare
accept, since the entry's own logic doesn't depend on the emitted
transaction's outcome.

## Unit tests

```sh
cargo test --manifest-path examples/Cargo.toml -p txn-template-optional
```

Two layers, split by what each needs to observe:

- `src/lib.rs`'s in-crate `#[cfg(test)]` module constructs `Remit`
  directly (a private type, so only reachable in-crate) and asserts its
  raw, pre-emit `bytes()` — the NOP-padding mechanism itself: an absent
  field's `NOP` bytes at the exact declared offset, and `amounts.0`'s
  native form leaving 40 trailing `NOP` bytes in its reserved 48-byte
  value region. No host backend is needed, since none of these calls
  reach `prepare_for_emit`/`emit`.
- `tests/remit.rs` drives the real `TxnTemplateOptional` chain through
  `rshooks_testenv::TestEnv::invoke` — no wasm build, no node — and
  asserts the *decoded* functional behavior against `env.emitted()`'s
  canonical (re-serialized, NOP-free) blob: `destination_tag` present
  with exactly the bytes written, absent by byte-count delta against the
  present-state blob (not by searching for the header's *absence* — a
  short header can coincidentally match part of a longer, unrelated one
  elsewhere in the blob, e.g. `EmitDetails`); `amounts`'s element count
  by counting the array's own `0xE1` terminators between `sfAmounts`'s
  header and its closing `0xF1` (one with `AMT2` absent, two once it
  enables `amounts.1`); both entries' issued form (`ISSUER` present)
  versus native form (absent); and a zero `AMT1`/`AMT2` rolling back
  rather than emitting.

Every expected region in both layers is built from
`rshooks::txn::codec::field_header`/`NOP` rather than hardcoded header
bytes, matching `examples/21_txn-template-nested`'s test style.

## Error codes

`RemitError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback!` code the entry can exit with:

| variant | code | meaning |
|---|---|---|
| `ReserveFailed` | 1 | `etxn_reserve(1)` failed to reserve an emission slot |
| `MissingDestination` | 2 | the `DEST` hook parameter was missing or not a 20-byte `AccountId` |
| `BufferAlreadyTaken` | 3 | the static template buffer had already been `take()`n |
| `ZeroAmount` | 4 | `AMT1`/`AMT2` was explicitly `0` — Remit rejects a zero amount |
| `AmountFailed` | 5 | an `any_amount` value was out of range |
| `PrepareFailed` | 6 | `prepare_for_emit` failed to fill in the host-supplied fields |
| `EmitFailed` | 7 | the prepared transaction could not be emitted |

## Cost

Current WCE, wasm size, and max nesting depth live in
[`metrics.json`](./metrics.json), refreshed by `mise run
record-example-metrics`.
