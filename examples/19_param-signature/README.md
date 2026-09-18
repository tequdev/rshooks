# param-signature

## What you'll learn

The Hook Parameter Signature Interface: extra arguments after `&self` on a
`#[hook(..)]` fn declare typed, machine-readable Hook parameters, decoded
before the body runs. See [Hook and Transaction
Parameters](../../book/src/data/parameters.md#signature-parameters-fn-arguments)
for the wire format, supported types, the generated rollback, and the
escape hatch (`rshooks::sig`).

This crate's `rshooks` dependency enables the `unstable-param-sig-interface`
feature (see this example's `Cargo.toml`) — without it, an extra
`#[hook(..)]` fn argument is a compile error.

## Specific to this example

This is the interface draft's own worked example,
`increment(account: AccountID, count: UInt16)`, kept deliberately small so
its generated `sethook.template.json` declarations are easy to read in
full. `crates/rshooks-testenv/tests/sig_params.rs` drives the same
declared signature end-to-end through `TestEnv::invoke`, including both
rollback paths and a successful invocation.

The generated `HookParameters` block in `sethook.template.json` (see [Per-Hook
Attributes and the SetHook Template](../../book/src/build/metadata.md)) holds
**declaration** entries — `HookParameterValue` is always the placeholder
`00`. Contrast an **invocation** entry, attached to the `Invoke` transaction
that actually triggers this hook, with real, typed values:

```json
{
  "TransactionType": "Invoke",
  "Account": "...",
  "Destination": "...",
  "HookParameters": [
    { "HookParameter": { "HookParameterName": "5F5053000008076163636F756E74", "HookParameterValue": "AABBCCDDEEFF00112233445566778899AABBCCDD" } },
    { "HookParameter": { "HookParameterName": "5F505300010105636F756E74", "HookParameterValue": "0007" } }
  ]
}
```

Same `HookParameterName`s as the declaration (index/type/name always
agree), but real `HookParameterValue`s: `account`'s 20 raw bytes, `count`'s
`0007` (`7` big-endian).

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/19_param-signature/Cargo.toml --out examples/19_param-signature/out
```

No extra flags — this hook has no compiler-generated loop at this
optimization level. Current worst-case instruction count, size, and max
nesting depth live in [`metrics.json`](./metrics.json).

## Expected behavior

- Either `account` or `count` missing, or the wrong length, on the
  `Invoke` → rollback from the generated prologue (`increment`'s body
  never runs).
- Both present and well-formed → accept, with the account's new counter
  total (`current + count`, wrapping) as the accept code. The counter
  persists across invocations, keyed per `account`.

Rollback codes for `increment`'s own body are declared on
`ParamSignatureError` in `src/lib.rs` (`rshooks::hook_errors!`), numbered
from `16` per the `>= 16` convention described in [Hook and Transaction
Parameters](../../book/src/data/parameters.md#signature-parameters-fn-arguments)
so they never collide with a signature-parameter argument index.
