# state-interface

## What you'll learn

The Hook State Interface: `#[state_interface(id = .., key(..), value(..))]`
chain-struct fields expose a Hook's state layout as a machine-readable,
typed key/value schema. See [Hook State](../../book/src/data/state.md#state-interface-typed-on-ledger-schema)
for the declaration grammar, supported types, the generated key/value byte
layout, and the design doc's spec vector (this crate's own worked
example).

This crate's `rshooks` dependency enables the `unstable-state-interface`
feature (see this example's `Cargo.toml`) — without it,
`#[state_interface(..)]` is a compile error.

## Specific to this example

Declares one keyed entry (`balances`, keyed by `account`/`token`) and one
singleton (`config`), the design doc's own worked example. Every `Invoke`
credits the sender's balance by 1 (`token` fixed to `0` — one balance per
sender) and bumps `updated` by 1, persisting across invocations.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/20_state-interface/Cargo.toml --out examples/20_state-interface/out
```

No extra flags. Current worst-case instruction count, size, and max
nesting depth live in [`metrics.json`](./metrics.json).

## Expected behavior

Every `Invoke` accepts with the new balance as the accept code
(`"state-interface: credited"`). A `state_set` failure (forced, e.g., by
capping the environment's max state value length in a test) rolls back
(`"state-interface: state_set failed"`).

Rollback codes are declared on `StateInterfaceError` in `src/lib.rs`
(`rshooks::hook_errors!`).
