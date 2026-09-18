# errors

## What you'll learn

Giving a hook its own meaningful, stable rollback error-code system with
`rshooks::hook_errors!` — one code and message per rejection reason,
instead of every failure path sharing an undifferentiated `rollback!(msg,
-1)`. See [Accept, Rollback, and Errors](../../book/src/concepts/errors.md)
("Designing meaningful error codes with `hook_errors!`") — this crate is
that page's own worked example, end to end.

## Specific to this example

`RejectReason`'s codes (`-101..=-104`) are chosen well outside the Hook
API's own `-1..=-45`/`-10024` range (`rshooks::error::HookError`), so an
application-defined `HookReturnCode` is unambiguous at a glance against a
`HookError` that leaked through instead — see `firewall`/`state-counter`/
`emit-txn` for the same `hook_errors!` idiom applied to smaller error sets.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/04_errors/Cargo.toml
```

No extra flags needed: `SourceTag`/`Amount` comparisons here are all
between plain integers (`u64`), never fixed-size arrays, so there's no
compiler-generated comparison loop to guard (contrast with `firewall`).

## Expected behavior

- Sender unreadable, blocked `SourceTag`, a non-native `Amount`, or a
  native `Amount` over the policy limit → rollback with that reason's own
  `RejectReason` code.
- Otherwise → accept.
