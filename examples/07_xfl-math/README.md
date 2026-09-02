# xfl-math

## What you'll learn

How to read a transaction's `Amount` as an **XFL** (Xahau's decimal
floating-point type) regardless of whether it's a native (XRP/XAH) or IOU
amount, do a ratio computation (`mulratio`) on it, and compare the result —
handling every step's `Result` explicitly, since every XFL host call is
fallible. Also: rshooks's XFL **operator** API end to end — the checked
`Add`/`Sub`/`Mul`/`Div`/`Neg` operators (all fallible host round trips), the
`.eq()`/`.lt()`/`.gt()`/`.compare()` comparison methods, the `==`/`<`/`>`
(`PartialEq`/`PartialOrd`) comparison *operators* on plain `XFL` (used
deliberately for exactly one, carefully-reasoned comparison here — see
below for why not more, and why that one is safe), and `XFLUnchecked`'s
poison-propagating hot-path chain.

## Code walkthrough

### Reading the Amount as XFL, via a slot

```rust
let txn_slot = otxn_slot(0)?;                       // load the otxn into a slot
let amount_slot = slot_subfield(txn_slot, sfAmount, 0)?;  // navigate to Amount
let amount = XFL::from_slot(amount_slot)?;           // decode as XFL
```

(the real code above matches on each `Result` and rolls back with a
specific message per failure, rather than using `?` — shown here
compressed for the walkthrough).

`otxn_slot(0)` loads the originating transaction into a new, auto-assigned
slot (`0` for `slot_into` means "auto-assign", the same convention used
throughout the Slot API). `slot_subfield(parent, field_id, 0)` then
extracts one field's own slot from it. `XFL::from_slot` (`slot_float` under
the hood) reads whatever's in that slot as an XFL — **this works
identically for an 8-byte native amount or a 48-byte IOU amount**, which is
the main advantage over parsing the raw bytes by hand (compare
`hook-params`/`errors`, which only understand the native case and reject
IOU amounts outright).

An equivalent, non-slot route to the same `XFL` exists too (see the
in-source comment): read the raw Amount bytes with `otxn_field`, then
`XFL::sto_set(&buf[..n])`. Either works; this example uses the slot route
because it's also a chance to show `otxn_slot`/`slot_subfield` (see
`examples/08_slot-ledger` for more on slot navigation specifically).

This hook stays on the **raw, numbered** slot API deliberately: it manages
the two slot numbers itself and calls `slot_clear` on both at the end. Those
functions are no longer in the prelude — mixing them with the typed
`SlotObject` layer would silently corrupt handles, since both address the
same 255 registers — so the source names them explicitly
(`rshooks::api::slot::{slot_clear, slot_subfield}`,
`rshooks::api::otxn::otxn_slot`). The typed equivalent of the walkthrough
above is one line shorter and needs no slot numbers at all:

```rust
let amount = SlotObject::from_otxn()?.get(sfAmount)?.as_xfl()?;
```

`examples/08_slot-ledger` is written that way, with a measured comparison.

### `mulratio` and the `.lt()` comparison

```rust
let share = amount.mulratio(false, 1, 100)?;   // 1% of `amount`, rounding down
// ...
match share.lt(min_share) {
    Ok(true) => rollback!(...),
    Ok(false) => {}
    Err(_) => rollback!(...),
}
```

`mulratio(round_up, num, den)` computes `self * (num / den)` — used here to
take 1% of the transaction amount; it takes two extra scale parameters
beyond `self`/`rhs`, so it stays a named method (no operator shape fits
it). Comparison is a fallible `float_compare` host round trip. `XFL` does
have `PartialEq`/`PartialOrd` (`==`/`<`/`>`/...) — but this hook uses
`.lt()` here instead, deliberately: those two traits' methods return a
bare `bool`/`Option<Ordering>`, with no room for an `Err` case, so they
fall back to `false`/`None` on a `float_compare` failure rather than
propagating it (see `rshooks::xfl`'s module doc comment, and this
README's "Migrating from the pre-operator method API" section below, for
why that's a reasonable choice for the operators in general but the wrong
one for a comparison that gates whether this hook rolls back). `.lt()`
returns `Result<bool>` and is matched three ways (`Ok(true)`/`Ok(false)`/
`Err(_)`) like every other fallible call here, so a `float_compare`
failure gets its own explicit rollback instead of silently falling through
as "not below the minimum."

### The checked `Sub` operator

```rust
let remaining = match amount - share {
    Ok(x) => x,
    Err(_) => rollback!(...),
};
match remaining.compare(XFL::from_raw_bits(0), COMPARE_LESS | COMPARE_EQUAL) {
    Ok(true) => rollback!(...),
    Ok(false) => {}
    Err(_) => rollback!(...),
}
```

`Sub`'s `Output` is `Result<XFL, HookError>` — implemented as `self + (-rhs)?`:
one `float_negate` host call (via the `Neg` operator) plus one `float_sum`
host call. There is no dedicated `float_subtract` host function, and `Neg`
is *not* a local sign-bit flip — see `rshooks::xfl`'s module doc comment,
which covers both why `Neg` still has to be a host round trip and why its
`Output` can be `Result<XFL, HookError>` even though `PartialEq`/
`PartialOrd` can't (unlike those two, `Neg`'s `Output` type isn't fixed by
the trait). Handled explicitly here exactly like every other fallible step
in this hook — the operator changes *how* the call is spelled
(`amount - share` vs. the equivalent `amount.sub(share)`), not whether the
`Result` gets checked. `XFL::from_raw_bits(0)` constructs canonical zero
with no host call at all (the all-zero bit pattern is always valid);
`COMPARE_LESS | COMPARE_EQUAL` is `.compare()`'s bitmask spelling of `<=`
(there's no dedicated `le`/`ge` convenience method — `.eq()`/`.lt()`/
`.gt()` cover the common cases, `.compare()` covers the rest).

### `XFLUnchecked`: a hot-path chain

```rust
let compounded_raw =
    share.unchecked() * growth.unchecked() * growth.unchecked() * growth.unchecked();
let compounded = match compounded_raw.validate() {
    Ok(x) => x,
    Err(_) => rollback!(...),
};
```

`XFLUnchecked` (`rshooks::xfl_unchecked`) is the poison-propagating
counterpart to `XFL`: every one of its operators is still a host round
trip (there is no local-only fast path — see its module doc comment for
why `Neg` in particular has to be `float_negate`, not a bit flip), but with
**no guest-side `Result` branch** between steps — the raw `i64` passes
straight from one host call into the next — then `validate()` turns the
final value into a real `Result<XFL, HookError>` with one last host round
trip. The performance win is entirely about *when* validation happens
(once, at the end, not once per step), not about skipping any host calls a
correct implementation actually needs. The three-multiply compounding
chain here is purely illustrative — it's nowhere near where per-step
`Result` handling would actually be the measured bottleneck worth
optimizing away — included solely to show the pattern's shape; see
`rshooks::xfl_unchecked`'s module doc comment for the full soundness
argument (why a poisoned/invalid operand can never produce a
spuriously-valid result from any of these operators) and its audit table.

### The `==`/`<`/`>` comparison operators — where they're actually safe

```rust
if compounded > remaining {
    rollback!(...);
}
```

`compounded` and `remaining` are both already-validated `XFL` values at
this point in the hook — `compounded` only exists because
`compounded_raw.validate()` returned `Ok`, and `remaining` only exists
because `amount - share` returned `Ok`. `PartialOrd::gt`'s `false`-on-
`float_compare`-failure fallback (see the section above) is a real risk
when either operand might still be unvalidated, exactly like `share`/
`min_share` were at the `.lt()` call site earlier in this hook — but by
the time execution reaches this `>`, there is no realistic path from two
already-`Ok`, host-validated XFLs to a `float_compare` failure: neither
value's bits have been touched since the host itself produced and
certified them, and `float_compare`'s own validation gate
(`RETURN_IF_INVALID_FLOAT`, per xahaud's `applyHook.cpp`) re-derives the
same mantissa/exponent from the same bits every host call sees them, so it
re-passes deterministically. That is the situation this crate's module
doc comment calls out as reasonable
for the operators: a comparison failure and a genuine
"not-greater-than" both mean "don't roll back" here, and that's the
correct behavior either way, so falling back to `false` costs nothing.
`compounded` (~1.0303% of `amount`, i.e. `share` compounded three times at
1%) is expected to be far smaller than `remaining` (~99% of `amount`) for
any realistic `Amount` — `>` here is a pure sanity check that would only
trip on a logic bug earlier in this hook, same spirit as the
`CompoundNotIncreasing` check above it.

### Constructing a fixed XFL constant

```rust
let min_share = XFL::new(-21, 1_000_000_000_000_000)?;
```

XFL's mantissa is normalized to 16 significant digits (`10^15` to
`10^16 - 1`, per `rshooks::xfl`'s module doc comment on the bit layout),
so `0.000001` (1e-6) is written as mantissa `1_000_000_000_000_000` (1e15)
with exponent `-21` (`1e15 * 10^-21 == 10^-6`) — not exponent `-6`, which
with that mantissa would be `1e9`. Getting this wrong is an easy mistake;
`XFL::new` returning `Result` (rather than silently normalizing or
truncating) is what surfaces it if the exponent/mantissa combination is
out of the valid range. The growth-factor constant (`1.01`) later in the
hook is constructed the same way: mantissa `1_010_000_000_000_000`,
exponent `-15`.

## Migrating from the pre-operator method API

`rshooks::xfl::XFL` used to expose `mul`/`add`/`div`/`neg` as named
methods; `eq`/`lt`/`gt`/`compare` stay named methods (see below for why).
The four arithmetic methods are now gone, replaced by operators (this is
the breaking change this example demonstrates end to end):

| Old method | New spelling |
|---|---|
| `a.mul(b)` | `a * b` (`Output = Result<XFL, HookError>`) |
| `a.add(b)` | `a + b` (`Output = Result<XFL, HookError>`) |
| `a.div(b)` | `a / b` (`Output = Result<XFL, HookError>`) |
| `a.neg()` | `-a` (`Output = Result<XFL, HookError>` — a `float_negate` host round trip, same as before) |

`a.eq(b)`/`a.lt(b)`/`a.gt(b)`/`a.compare(b, mode)` are **unchanged** — still
named methods returning `Result<bool>`, still backed by `float_compare`.
`XFL` *also* now implements `PartialEq`/`PartialOrd` (`==`/`<`/`>`/...),
forwarding to those same methods. This example uses **both**, deliberately
choosing per call site: the `Result`-returning methods, matched three ways
(`Ok(true)`/`Ok(false)`/`Err(_)`), for every comparison that gates a
rollback decision on a value that hasn't been separately validated yet
(`share`/`min_share` at the `.lt()` call site, `remaining`/zero and
`compounded`/`share` at the two `.compare()` call sites); the `>` operator
for exactly one comparison, `compounded > remaining`, where both operands
are already-validated `XFL`s with no realistic path to a `float_compare`
failure (see the "`==`/`<`/`>` comparison operators" section above for the
full reasoning). The difference matters because `PartialEq`/`PartialOrd`'s
fixed `bool`/`Option<Ordering>` return types can't express a
`float_compare` failure, so on failure they fall back to `false`/`None`
(the same convention `f64` uses for `NaN`) — which for `share < min_share`
would mean silently treating "the comparison failed" the same as "the
share is not below the minimum," i.e. silently accepting a transaction
this hook could not actually validate. `==`/`<`/`>` are a reasonable
choice specifically when a comparison failure and a genuine
inequality/incomparable both deserve the same handling (see
`rshooks::xfl`'s module doc comment's "Comparison: both methods and
operators, both via `float_compare`" section) — true for
`compounded > remaining` here, not true for any of the other three
comparisons in this hook.

`a - b` (`Sub`) is new — there was no `sub`/`subtract` method before, since
there's no dedicated `float_subtract` host function; it's built from `Neg`
plus `float_sum` (two host calls total). Chains that mix a plain `XFL` with
an already-`Result<XFL, HookError>` value on either side (`a + b + c`) work
without an explicit `?` between steps; see `rshooks::xfl`'s module doc
comment for exactly which combinations are (and, for one specific
combination that Rust's orphan rules make impossible, are not) supported.
For hot paths where even that per-step `Result` handling is the measured
cost problem, there's the new `XFLUnchecked` (`rshooks::xfl_unchecked`),
demonstrated above.

## Handling XFL's failure modes

Every fallible step here — `otxn_slot`, `slot_subfield`, `XFL::from_slot`,
`mulratio`, `XFL::new`, `.lt()`/`.compare()`, the `Sub` operator,
`XFLUnchecked::validate` — is matched explicitly and rolls back with a
distinct message on `Err`, rather than being unwrapped. Concretely, the
kinds of `HookError` these can surface include:

| Call | Example failure |
|---|---|
| `slot_subfield` | `DOESNT_EXIST` — no `Amount` field on this transaction type |
| `XFL::from_slot` | `NOT_AN_AMOUNT` — the field isn't an Amount-shaped object |
| `mulratio` | `XFL_OVERFLOW` — the scaled result doesn't fit |
| `XFL::new` | `MANTISSA_OVERSIZED`/`MANTISSA_UNDERSIZED`/`EXPONENT_OVERSIZED`/`EXPONENT_UNDERSIZED` — out-of-range inputs |
| `.lt()`/`.compare()` | `INVALID_FLOAT` — either operand isn't a valid XFL bit pattern |
| `Sub` (`amount - share`) | `INVALID_FLOAT` — either operand isn't a valid XFL bit pattern (via `Neg` or `float_sum`) |
| `XFLUnchecked::validate` | `INVALID_FLOAT` — the chain's final raw value (or any poisoned value it passed through) didn't validate |

`compounded > remaining` is the one comparison in this hook *not* in that
list: `PartialOrd::gt`'s `Result`-free signature has no `Err` case to
surface in the first place, by design — see the "`==`/`<`/`>` comparison
operators" section above for why that's fine specifically at this call
site.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/07_xfl-math/Cargo.toml
```

No extra flags needed: every operation here is either a host call
(`otxn_slot`, `slot_subfield`, the `float_*` family) or a scalar match on
the resulting `bool`/`i64` values — no fixed-size array comparison, so no
compiler-generated `bcmp`-style loop to guard.

## Zero-cost check: operators vs. the old method API

The mulratio-and-`.lt()` logic is **unchanged, byte-for-byte, from the
pre-operator method API** — isolated on its own, it reproduces the
pre-operator worst-case instruction count exactly. The `Add`/`Mul`/
`Div` arithmetic operators are a pure syntax change over the pre-operator
`.add()`/`.mul()`/`.div()` methods (same single host call each) and so are
zero-cost by construction; `Sub`/`Neg` are the one place cost actually
differs, because `Neg` is a real `float_negate` host round trip, not a
local bit flip. The full version in this crate — with the `Sub`,
`XFLUnchecked`, and `==`/`<`/`>` operator sections added on top purely for
demonstration — measures higher, purely from those added sections (see
below for where that comes from); current values live in
[`metrics.json`](./metrics.json).

Chained-operator benchmark against the actual shipped types
(`rshooks::xfl`/`rshooks::xfl_unchecked`, N=1/4/8 chained ops,
`opt-level = "z"`, `lto = "fat"`):

| chain | N=1 | N=4 | N=8 | marginal cost/op |
|---|---|---|---|---|
| raw `float_multiply` (baseline) | 27 | 36 | 48 | +3 |
| `XFLUnchecked` `Mul` chain | 29 | 38 | 50 | +3 (matches raw exactly) |
| checked `Result`-chain `Mul` | 27 | 69 | 125 | +14 |
| raw `float_negate`+`float_sum` (baseline) | 29 | 44 | 64 | +5 |
| `XFLUnchecked` `Sub` chain | 31 | 46 | 66 | +5 (matches raw exactly) |
| checked `Result`-chain `Sub` | 40 | 121 | 229 | +27 |

`XFLUnchecked`'s marginal cost matches a hand-written raw host-call chain
exactly, for both `Mul` (single host call per step) and `Sub` (two host
calls per step, since `Neg` isn't free) — its performance win over the
checked operators is real and comes entirely from skipping the per-step
`Result` branch, not from skipping any host validation a correct
implementation actually needs.

## Expected behavior

- 1% of the transaction `Amount` is at least `0.000001` → accept (subject
  to the `Sub`/`XFLUnchecked`/`==`/`<`/`>` sanity checks below, which
  should never actually trip for a valid positive `Amount`).
- 1% of the transaction `Amount` is below `0.000001` → rollback
  (`"xfl-math: computed share below minimum"`, code `7`).
- Any of the intermediate steps fails (missing `Amount` field, overflow,
  ...) → rollback with that step's specific message and code (see below).

## Error codes

`XflMathError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback!` code for each failure this hook can exit with:

| variant | code | meaning |
|---|---|---|
| `OtxnSlotFailed` | 1 | `otxn_slot` failed to load the originating transaction into a slot |
| `NoAmountField` | 2 | `slot_subfield` found no `Amount` field on the originating transaction |
| `InvalidAmount` | 3 | `XFL::from_slot` could not decode the `Amount` slot as a valid XFL amount |
| `MulratioFailed` | 4 | `mulratio` failed (e.g. overflow) computing the percentage share |
| `MinShareConstructFailed` | 5 | `XFL::new` failed to construct the fixed minimum-share constant |
| `ComparisonFailed` | 6 | the `.lt()` comparison between the computed share and the minimum failed |
| `BelowMinimum` | 7 | the computed share fell below the fixed minimum |
| `RemainingComputeFailed` | 8 | `amount - share` (the checked `Sub` operator) failed |
| `RemainingComparisonFailed` | 9 | the `remaining <= 0` comparison (`.compare()`) failed |
| `NotEnoughRemaining` | 10 | `amount - share` was not strictly positive |
| `GrowthConstructFailed` | 11 | `XFL::new` failed to construct the fixed growth-factor constant |
| `CompoundValidationFailed` | 12 | the `XFLUnchecked` compounding chain's final `validate()` call failed |
| `CompoundComparisonFailed` | 13 | the `compounded <= share` comparison (`.compare()`) failed |
| `CompoundNotIncreasing` | 14 | the compounded value did not come out strictly greater than `share` |
| `CompoundExceedsRemaining` | 15 | `compounded > remaining` (the `>` operator) — the compounded projection exceeded the transaction amount minus its own share |
