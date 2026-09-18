# state-counter

## What you'll learn

The typed hook-state layer's smallest complete tutorial: a `#[state(key =
...)]` field on the `#[hooks]` struct, read-increment-write with no
hand-rolled buffer or byte-order code. See [Hook
State](../../book/src/data/state.md) ("Tier 3" and "The counter
walkthrough") for the full mechanism and generated accessor list.

## Specific to this example

The key sent to the host is exactly `counter`'s own 7 bytes, never locally
zero-padded to the fixed 32-byte key space (see `rshooks::state`'s module
doc comment, "Key length and padding") — the same on-ledger slot a C hook's
`state(&v, 8, "counter", 7)` would address.

For a hook this small (one `u64`, one key), a hand-rolled raw-buffer read
would cost about the same as the typed layer's read path (which still goes
through the generic scratch-buffer machinery `MAX_TYPED_STATE_LEN` sizes);
see [`metrics.json`](./metrics.json) for current numbers. This example uses
the typed layer anyway because its purpose is to be the smallest possible
tutorial for it — see `examples/12_typed-data` for the layer's actual
selling point, a composite multi-field key/value pair.

Failure/rollback codes are declared on `StateCounterError` in `src/lib.rs`.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/02_state-counter/Cargo.toml
```

No extra flags needed — this example is guard-clean at the source level.

## Unit tests

```sh
cargo test --manifest-path examples/Cargo.toml -p state-counter
```

`tests/counter.rs` drives the real `StateCounter` chain through
`rshooks_testenv::TestEnv::invoke` — no wasm build, no node: a first-invoke
assertion, persistence across two invocations, and a forced `state_set`
failure proving the rollback path leaves no trace. See
`book/src/testing/unit-tests.md` for what this harness does and does not
model.
