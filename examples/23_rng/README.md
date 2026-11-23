# rng

## What you'll learn

Drawing randomness from the consensus RNG with `dice` and `util_random`,
and treating `TooLittleEntropy` as a defined outcome rather than an
unexpected failure. See [Consensus Randomness](../../book/src/data/random.md).

## Specific to this example

On an `Invoke`, the hook rolls `dice(6)`, draws an 8-byte buffer with
`util_random`, stores the roll and the bytes in hook state, and accepts with
the roll.

`TooLittleEntropy` means the ledger has no usable entropy, which a hook
cannot distinguish from the amendment being disabled. The hook rolls back
with `RngError::NoEntropy` and writes nothing; any other draw failure rolls
back with `RngError::DrawFailed`. `dice` and `util_random` share a draw
budget per execution, and a rollback does not undo draws already made.

The e2e node has no validator keys configured, so no entropy transaction is
ever built and the end-to-end test (`e2e/test/rng.test.ts`) asserts the
`NoEntropy` rollback. The unit tests
seed entropy with `TestEnv::rng_seed`.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/23_rng/Cargo.toml
```

No extra flags needed — this example is guard-clean at the source level.

## Unit tests

```sh
cargo test --manifest-path examples/Cargo.toml -p rng
```

`tests/rng.rs` drives the real `Rng` chain through
`rshooks_testenv::TestEnv::invoke`: a seeded roll stays in range and is
stored, consecutive invocations advance the entropy, and an unseeded ledger
or a thin validator contribution rolls back with `NoEntropy`.

## Error codes

Rollback codes are declared on `RngError` in `src/lib.rs`
(`rshooks::hook_errors!`).

## Cost

Current WCE, wasm size, and max nesting depth live in
[`metrics.json`](./metrics.json).
