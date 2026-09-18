# hook-params

## What you'll learn

Making a hook's behavior configurable at install time via a **Hook
parameter** (`#[hook_param(...)]`), with a compiled-in default when the
operator doesn't set one. See [Hook and Transaction
Parameters](../../book/src/data/parameters.md) ("`default`: a compiled-in
fallback") for the field-attribute grammar and what "absent" vs.
"present-but-malformed" each resolve to.

## The hook

Rolls back the originating transaction if its native (XRP/XAH) `Amount` is
below a minimum threshold; accepts otherwise. The threshold comes from a
Hook parameter named `MIN` (a little-endian `u64` drops value, wrapped in a
one-field `MinDrops` via `#[derive(ParamValue)]` so its meaning travels
with its type), falling back to a baked-in default (1 XAH) via `MinDrops`'s
own `Default` impl when `MIN` isn't configured.

This example intentionally only supports native amounts — rejecting an IOU
`Amount` outright — using the same `otxn_field_typed`/`AmountBytes` match
[Reading the Originating Transaction](../../book/src/data/otxn.md) covers.
Reading *any* `Amount` kind uniformly is what `examples/07_xfl-math` is
for.

## Hook parameter hex encoding

`MIN` must be exactly 8 bytes, little-endian. For a threshold of
`5,000,000` drops (5 XAH):

```
decimal:  5000000
hex (u64, little-endian): 40 4B 4C 00 00 00 00 00
```

In a `SetHook` transaction's `HookParameters` array, this becomes one
`HookParameter` entry:

```json
{
  "HookParameter": {
    "HookParameterName": "4D494E",
    "HookParameterValue": "404B4C0000000000"
  }
}
```

`HookParameterName` is the hex encoding of the ASCII parameter name (`MIN`
→ `4D494E`); `HookParameterValue` is the hex encoding of the 8 raw bytes
above. Omitting the `MIN` entry entirely falls back to the compiled-in
1 XAH default.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/03_hook-params/Cargo.toml
```

No extra flags needed: every comparison here is between plain integers
(`u64`), not fixed-size arrays, so there's no compiler-generated
`bcmp`-style loop to worry about (contrast with `firewall`, which compares
two `[u8; 20]`s and avoids that loop with `buf_eq_20`).

## Expected behavior

- `MIN` unset, `Amount` = 1 XAH or more → accept.
- `MIN` unset, `Amount` below 1 XAH → rollback.
- `MIN` set, `Amount` at or above it → accept.
- `MIN` set, `Amount` below it → rollback.
- `MIN` present but not exactly 8 bytes → rollback, regardless of `Amount`.
- `Amount` is an IOU (not native XRP/XAH) → rollback, regardless of `MIN`.

Failure/rollback codes are declared on `HookParamsError` in `src/lib.rs`.
