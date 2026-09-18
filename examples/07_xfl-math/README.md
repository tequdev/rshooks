# xfl-math

## What you'll learn

Reading a transaction's `Amount` as an **XFL** (regardless of whether it's
native XRP/XAH or an IOU), `mulratio` for percentage scaling, the checked
`Add`/`Sub`/`Mul`/`Div`/`Neg` operators, and the `.eq()`/`.lt()`/`.gt()`/
`.compare()` comparison methods vs. the `==`/`<`/`>` operators. See
[XFL: Decimal Floating Point](../../book/src/data/xfl.md) for the type
itself and `rshooks::xfl`'s module doc comment for the full API.

## Specific to this example

This hook stays on the **raw, numbered** slot API deliberately (managing
its own slot numbers and calling `slot_clear` at the end) rather than the
typed `SlotObject` layer — see `examples/08_slot-ledger` for the typed
equivalent, one line shorter and with no slot numbers in sight.

It also demonstrates a real choice between the `Result`-returning
comparison methods and the `PartialEq`/`PartialOrd` operators: `.lt()` for
every comparison that gates a rollback on a value not yet independently
validated, and the `>` operator for exactly one comparison
(`compounded > remaining`) where both operands are already host-validated
XFLs with no realistic path to a `float_compare` failure. See the
in-source comments at each call site for the reasoning.

Failure/rollback codes are declared on `XflMathError` in `src/lib.rs`
(`rshooks::hook_errors!`) — each variant's doc comment states which step
it corresponds to.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/07_xfl-math/Cargo.toml
```

No extra flags needed: every operation here is either a host call or a
scalar match on the resulting `bool`/`i64` — no fixed-size array
comparison, so no compiler-generated `bcmp`-style loop to guard.

## Chained-operator benchmark: `XFLUnchecked` vs. checked `Result` chains

One-off probe against the shipped types (`rshooks::xfl`/
`rshooks::xfl_unchecked`, N=1/4/8 chained ops, `opt-level = "z"`,
`lto = "fat"`); frozen at the time it was recorded, not part of
`metrics.json` and not CI-checked:

| chain | N=1 | N=4 | N=8 | marginal cost/op |
|---|---|---|---|---|
| raw `float_multiply` (baseline) | 27 | 36 | 48 | +3 |
| `XFLUnchecked` `Mul` chain | 29 | 38 | 50 | +3 (matches raw exactly) |
| checked `Result`-chain `Mul` | 27 | 69 | 125 | +14 |
| raw `float_negate`+`float_sum` (baseline) | 29 | 44 | 64 | +5 |
| `XFLUnchecked` `Sub` chain | 31 | 46 | 66 | +5 (matches raw exactly) |
| checked `Result`-chain `Sub` | 40 | 121 | 229 | +27 |

`XFLUnchecked`'s marginal cost matches a hand-written raw host-call chain
exactly — its win over the checked operators comes entirely from skipping
the per-step `Result` branch, not from skipping any host validation a
correct implementation actually needs.

## Expected behavior

- 1% of the transaction `Amount` is at least `0.000001` → accept (subject
  to the `Sub`/`XFLUnchecked`/`==`/`<`/`>` sanity checks, which should
  never trip for a valid positive `Amount`).
- 1% of the transaction `Amount` is below `0.000001` → rollback.
- Any intermediate step fails (missing `Amount` field, overflow, ...) →
  rollback with that step's specific `XflMathError` code.
