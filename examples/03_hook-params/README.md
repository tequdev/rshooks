# hook-params

## What you'll learn

How to make a Hook's behavior configurable at install time via a **Hook
parameter** (`#[hook_param]`), with a sensible compiled-in default when the
operator doesn't set one.

## The hook

Rolls back the originating transaction if its native (XRP/XAH) `Amount` is
below a minimum threshold; accepts otherwise. The threshold comes from a
Hook parameter named `MIN` — 8 raw bytes, a little-endian `u64` drops
value — falling back to a baked-in default (`1,000,000` drops = 1 XAH) if
`MIN` isn't configured.

## Code walkthrough

```rust
#[derive(ParamValue)]
struct MinDrops {
    drops: u64,
}

impl Default for MinDrops {
    fn default() -> Self {
        Self {
            drops: DEFAULT_MIN_DROPS,
        }
    }
}

fn min_drops() -> u64 {
    HookParams
        .hook_param
        .min
        .get_or_default()
        .unwrap_or_default()
        .drops
}

#[hooks]
pub struct HookParams {
    #[hook_param(name = b"MIN", default = MinDrops::default())]
    min: HookParam<MinDrops>,
}
```

The `min` field declares the `MIN` Hook parameter permanently paired with
its value shape, closing the gap the loose `hook_param_exact::<T>(name)`
accessor leaves open (a typo or copy-paste error can pair the right name
with the wrong type, or vice versa, and both compile fine). `MinDrops`
wraps the `u64` value in a one-field `ParamValue` struct so `MIN`'s meaning
travels with its type instead of reading a bare `[u8; 8]` (`FixedRead` is
implemented for `[u8; N]`, `rshooks::types` newtypes, `XFL`, and
`#[derive(ParamValue)]`/`#[derive(HookData)]` structs — not for a bare
`u64`) — the same house idiom `examples/12_typed-data` uses for its
`Config`/`Instruction` fields. The compiled-in fallback is single-sourced
through `MinDrops`'s `Default` impl: the attribute's `default =
MinDrops::default()` covers the absent case inside
`HookParam<V>::get_or_default()` (which returns `Ok(<the field's
default>)` when `MIN` is absent, and `Err` when `MIN` is present but the
wrong number of bytes for `MinDrops` (8)), and `.unwrap_or_default()`
masks that `Err` back to the very same value — both paths land on the
default, not just the absent case. `MinDrops`'s `drops: u64` field decodes via this crate's
little-endian `FromBytes` trait (`rshooks::convert::FromBytes for u64`).
A Hook parameter like `MIN` carries no protocol-mandated endianness of its
own — its byte convention is whatever the operator who set it wrote — so
it's the declared-field tier itself that fixes `MIN` to little-endian,
matching `examples/12_typed-data`'s `CFG`. Contrast the originating
transaction's `Amount`, read below: a genuine protocol field decoded
through `otxn_field_exact` stays big-endian per Xahau Binary's own wire
format, and the `u64::from_be_bytes(n.0)` line below applies that same
convention by hand to the raw bytes `otxn_field_typed` hands back.

The originating transaction's `Amount` is read via
`otxn_field_typed(sfAmount)`, which classifies the field by its wire length
and hands back an `AmountBytes` — `Native([u8; 8])` for an 8-byte native
amount, `Iou(_)` for a 48-byte IOU amount — so only the `Native` arm is
accepted; `Iou` (and any read error) falls to the same "unsupported" arm.
The top two bits of a serialized native amount are format flags, not
part of the drops value (`0x80` = "not an IOU", `0xC0`'s low bit = sign,
always set since XRP/XAH amounts are never negative) — see
`rshooks::txn::codec::encode_native_amount_const`'s doc comment for the
same bit layout used in the other direction (encoding a drops value for an
emitted transaction). Masking `NATIVE_AMOUNT_FLAG_BITS` off recovers the
plain drops magnitude.

This example intentionally only supports native amounts — reading *any*
`Amount` kind (native or IOU) uniformly is what `examples/07_xfl-math` is for.

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
two `[u8; 20]`s and needs `--auto-guard`).

## Expected behavior

- `MIN` unset, `Amount` = 1 XAH or more → accept.
- `MIN` unset, `Amount` below 1 XAH → rollback (`"hook-params: amount below
  configured minimum"`, code `2`).
- `MIN` set to some threshold, `Amount` at or above it → accept.
- `MIN` set, `Amount` below it → rollback, code `2`.
- `Amount` is an IOU (not native XRP/XAH) → rollback (`"hook-params:
  unsupported (non-native) Amount"`, code `1`), regardless of `MIN`.

## Error codes

`HookParamsError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback!` code for each failure this hook can exit with:

| variant | code | meaning |
|---|---|---|
| `UnsupportedAmount` | 1 | the originating transaction's `Amount` isn't an 8-byte native (XRP/XAH) amount |
| `BelowMinimum` | 2 | the native `Amount` fell below the configured (or default) minimum |
