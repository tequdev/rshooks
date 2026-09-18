# txn-template-nested

## What you'll learn

`txn_template!`'s homogeneous indexed array form
(`array(sfX) [ Elem: object(sfY) { .. } ; N ]`) and `fixed_vl(sfX, N)`. See
[Emitting Transactions](../../book/src/emit/emitting.md) for the full
grammar, the generated `Elem`/accessor shape, and the shared
container-nesting rules — this crate is that chapter's own worked example.

This hook emits a Remit whose `sfAmounts` field is two back-to-back copies
of one issued `amount` entry, declared once and repeated, and an
`sfMemos` field holding one memo with two `fixed_vl` fields, with no
`StoWriter` call anywhere.

## Specific to this example

Contrast `examples/17_sto-writer`'s Remit, whose second `sfAmounts` entry
is only present *conditionally*, based on hook parameters supplied at
runtime — that shape isn't known at compile time, so it needs `StoWriter`.
`txn_template!`'s indexed arrays are for the opposite case: the element
*count* and every element's shape are fixed by the declaration; only the
per-element *values* are filled in at runtime.

`main` exercises both `amount` setters on the real `wasm32v1-none`
target — the 8-byte `_value` hot path by default, and the full 48-byte
setter (the same `[u8; 48]` build-and-copy `StoWriter::iou_amount` writes
on every call) when an `ISSUER` hook parameter is present — so `rshooks
build`/`check` catches a compiler-generated copy loop over that region
before it reaches a live node, not just the 8-byte path.

Both hook parameters (`DEST`, required; `ISSUER`, optional) are declared
on the chain struct the same way `examples/03_hook-params` does.

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

Two equivalent layouts (`tests/remit.rs`, and an in-crate `#[cfg(test)]`
module) exercise the real entry through `rshooks_testenv::TestEnv::invoke`
— no wasm build, no node — covering the accept-and-emit path, the
`DEST`-missing rollback, `cbak`, a byte-exact check of the `sfAmounts` and
`sfMemos` regions against hand-derived expected bytes, and the
`ISSUER`-parameter override. The in-crate module additionally cross-checks
the private `Remit` type directly against `rshooks::sto_writer::StoWriter`
building the same bytes independently, since that comparison is only
reachable from an in-crate test.

## Error codes

Rollback codes are declared on `TxnTemplateNestedError` in `src/lib.rs`
(`rshooks::hook_errors!`); the two "index out of range" variants are
unreachable by construction (both accessor calls use literal indexes
below the declared count) and are kept only because the accessor returns
`Option`.

## Cost

Current WCE, wasm size, and max nesting depth live in
[`metrics.json`](./metrics.json). `examples/17_sto-writer`'s `metrics.json`
is the natural comparison, but the two hooks do different work:
`StoWriter` pays bounds/duplicate checks on every field plus its
conditional issued-entry branch, while here every setter is a
fixed-offset store but `main` reads two hook parameters and computes each
value through `XFL::new`. Read the two files side by side rather than
expecting either to win on every axis.
