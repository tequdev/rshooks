# emit-txn

## What you'll learn

Reserving an emission slot, building a `txn_template!`-declared Payment,
`emit()`ing it, and reacting to the outcome in a paired `#[cbak]`. See
[Emitting Transactions](../../book/src/emit/emitting.md) — this exact hook
is that page's worked example end to end, including the static-buffer
idiom the `Payment` template uses and why.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/10_emit-txn/Cargo.toml
```

No extra flags needed — the static-buffer idiom leaves no
compiler-generated loop to guard.

## Unit tests

```sh
cargo test --manifest-path examples/Cargo.toml -p emit-txn
```

Two equivalent layouts exercise the real `EmitTxn` entry through
`rshooks_testenv::TestEnv::invoke` — no wasm build, no node: `tests/emit.rs`
(an integration test against the crate as a library) and an in-crate
`#[cfg(test)]` module at the bottom of `src/lib.rs` (made possible by
`#![cfg_attr(not(test), no_std)]` — `std` is only available under the test
harness, never in the shipped wasm build). Both assert the accept exit and
inspect the captured emission (`env.emitted()`'s length, transaction type,
and blob). See `book/src/testing/unit-tests.md` for the full walkthrough.

Failure/rollback codes are declared on `EmitTxnError` in `src/lib.rs`
(`rshooks::hook_errors!`) — each variant's doc comment states which step
it corresponds to.
