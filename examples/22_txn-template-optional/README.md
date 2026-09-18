# txn-template-optional

## What you'll learn

`txn_template!`'s NOP-padded `optional` field kinds, written entirely in
the inferred (bare-`sfX`) style: `optional sfX` (any inferable scalar
kind), and `any_amount` via the `= AnyAmount()` default-shape marker (the
one kind here that cannot infer from its serialized type alone). See
[Emitting Transactions](../../book/src/emit/emitting.md#optional-and-variable-length-fields)
for the full NOP-padding mechanism, the per-container 63-`NOP` budget, and
this crate's own `amounts` field as that section's worked example.

## Specific to this example

The scenario: a Remit that sends one or two amounts, with an `optional`
`DestinationTag` — every field legal for a Remit per
`crates/rshooks-core/protocol_formats.json`. The other NOP-padded kinds
`docs/NOP_PADDING_DESIGN.md` introduces — `vl`/`optional vl`,
whole-container `optional` views, and a homogeneous array of `optional`
elements — don't fit this single, protocol-legal scenario alongside
`amounts` without reaching for an unrelated `sfcode`; they're covered by
`crates/rshooks/src/txn.rs`'s `mod tests` twinned fixtures and
`crates/rshooks/tests/ui/{pass,fail}` instead.

`Remit::amounts` is an array with one required and one `optional` element
(both numbered by position, not the homogeneous indexed form) because a
homogeneous array can't express "one required, one that may or may not be
there" — its elements share one presence rule — and two fully `optional`
elements wouldn't fit the array's own NOP budget anyway.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/22_txn-template-optional/Cargo.toml
```

One artifact: `0.remit.wasm` — no `cbak` beyond a bare accept, since the
entry's own logic doesn't depend on the emitted transaction's outcome.

## Unit tests

```sh
cargo test --manifest-path examples/Cargo.toml -p txn-template-optional
```

Two layers: `src/lib.rs`'s in-crate `#[cfg(test)]` module constructs
`Remit` directly and asserts its raw, pre-emit `bytes()` (the NOP-padding
mechanism itself, no host backend needed); `tests/remit.rs` drives the
real chain through `rshooks_testenv::TestEnv::invoke` and asserts the
*decoded* functional behavior against `env.emitted()`'s canonical
(re-serialized, NOP-free) blob — presence/absence of `destination_tag`,
`amounts`'s element count, both entries' issued vs. native form, and a
zero amount rolling back. Every expected region in both layers is built
from `rshooks::txn::codec::field_header`/`NOP` rather than hardcoded
header bytes, matching `examples/21_txn-template-nested`'s test style.

## Error codes

Rollback codes are declared on `RemitError` in `src/lib.rs`
(`rshooks::hook_errors!`).

## Cost

Current WCE, wasm size, and max nesting depth live in
[`metrics.json`](./metrics.json).
