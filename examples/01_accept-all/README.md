# accept-all

The minimal starter Hook: traces a short message (the `trace` feature is
enabled by default in this crate specifically to demonstrate that), then
unconditionally accepts the originating transaction.

No loop, no state, no emitted transactions — copy this crate as the
starting point for a new Hook.

## Build

```sh
rshooks build --manifest-path examples/01_accept-all/Cargo.toml
```

or, from the repo root:

```sh
cargo run -p rshooks-build -- build --manifest-path examples/01_accept-all/Cargo.toml
```

No extra flags needed — this example is guard-clean at the source level.
