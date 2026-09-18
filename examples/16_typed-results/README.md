# typed-results

## What you'll learn

The typed entry-return form: every `#[hook]`/`#[cbak]` entry returns
`HookResult` (a `Result<Accept, Rollback>` alias), and `?` can propagate
failures out of ordinary helper functions in place of a hand-written
`accept!`/`rollback!` call at every failure point. See
[Accept, Rollback, and Errors](../../book/src/concepts/errors.md#typed-entry-returns-hookresult)
for the full walkthrough of this crate's `deposit`/`read_amount`/
`bump_counter` code, the `#[inline(always)]` helper convention, and why
`rshooks` has no `From<HookError> for Rollback` impl.

## Specific to this example

This crate declares one chain with both entry styles side by side, both
`-> HookResult`: `deposit` uses the idiomatic `?`/`Ok` form (covered in the
book), and `reset` uses the raw `accept!`/`rollback!` escape hatch directly
inside its `HookResult`-returning body — proving that style stays
first-class within the typed signature, not just the `?`-based one.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/16_typed-results/Cargo.toml
```

No extra flags needed — both entries are guard-clean at the source level.

## Unit tests

```sh
cargo test --manifest-path examples/Cargo.toml -p typed-results
```

`tests/deposit.rs` drives both entries through `rshooks_testenv::TestEnv::invoke`
— no wasm build, no node: `deposit`'s accept path (including that the running
total persists across invocations), its two rollback paths (missing `AMT`,
forced `state_set` failure) with an assertion that `HookExit.msg` carries the
exact msg-clause bytes through the `?`/`From<DepositError> for Rollback`
conversion, and `reset`'s accept path. See `book/src/testing/unit-tests.md`
for the harness this builds on.

## Error codes

`DepositError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback`/`Rollback` code and message for each failure `deposit` can exit
with (`reset` reuses `StateSetFailed` directly, via `rollback!`) — each
variant's doc comment states its meaning.

## Cost of the typed form, here

Current WCE, wasm size, and max nesting depth for each entry live in
[`metrics.json`](./metrics.json). Both entries stay well within the
nesting and size budgets; `deposit`'s higher numbers versus `reset` reflect
it doing strictly more work (a required-parameter read plus a state
read-modify-write, versus `reset`'s single unconditional write), not a
cost the typed form itself imposes — see the book chapter's
`#[inline(always)]`-convention section for the apples-to-apples probe this
example's numbers corroborate rather than repeat.
