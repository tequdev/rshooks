# XFL: Decimal Floating Point

Xahau amounts and rates are not `f64`. They are **XFL**, a 64-bit decimal
floating-point format the Hook API itself defines: a sign bit, an 8-bit
biased exponent, and a 54-bit mantissa normalized to exactly 16 significant
decimal digits. Every arithmetic operation on an XFL value is a host call —
`rshooks` never computes on the bit pattern itself — because only the host
knows how to normalize a result and detect overflow the same way the rest of
the ledger does. This page covers the `XFL` type, how to construct values
(including at compile time), the checked operators and comparison methods,
reading an `Amount` field as XFL, and `XFLUnchecked` for hot paths that can
defer validation to the end of a chain.

## Why not `f64`

`f64` cannot represent most decimal fractions exactly — `0.1` in binary
floating point is already an approximation. XFL exists precisely to avoid
that: every decimal value expressible in 16 significant digits has exactly
one correct XFL encoding, bit-for-bit. Any bridge from decimal text to XFL
that routes through `f64` reintroduces the very rounding error XFL is
designed to eliminate — which is why the `XFL!` literal macro below never
touches `f64` at all.

## The `XFL` type

`rshooks::xfl::XFL` wraps a raw XFL bit pattern. The inner value is private:
a host XFL call can return a negative value down the same `i64` channel used
for float results, so a public field would let an error code masquerade as a
value. Two explicit escape hatches cross that boundary:

```rust
use rshooks::xfl::XFL;

let one = XFL::one();
let bits = one.raw_bits();
let same = XFL::from_raw_bits(bits);
assert_eq!(same.raw_bits(), bits);
```

`XFL::one()` is the only constructor guaranteed not to fail. `XFL::new(exponent, mantissa)` builds a normalized value from its two components:

```rust,ignore
let min_share = XFL::new(-21, 1_000_000_000_000_000)?;
```

(from `examples/07_xfl-math`). XFL's mantissa is always normalized to 16
significant digits (`10^15` to `10^16 - 1`), so `0.000001` is not written as
exponent `-6` with mantissa `1` — it has to be mantissa
`1_000_000_000_000_000` (`1e15`) with exponent `-21`, since
`1e15 * 10^-21 == 10^-6`. Getting the mantissa/exponent split wrong is an
easy mistake, and `XFL::new` returning `Result` rather than silently
normalizing is what catches it.

## The `XFL!` compile-time literal macro

For a fixed constant, hand-computing that mantissa/exponent split — or the
raw bit pattern — is exactly the kind of arithmetic a macro should do
instead. `XFL!` takes a decimal literal and expands, at compile time, to
`XFL::from_raw_bits(<bits>i64)`:

```rust
use rshooks::XFL;
use rshooks::xfl::XFL as XflType;

const DEFAULT_REWARD_RATE: XflType = XFL!(0.003333333333333333);
assert_eq!(DEFAULT_REWARD_RATE.raw_bits(), 6_038_156_834_009_797_973);
```

Because the expansion is `XFL::from_raw_bits`, a `const fn`, the result works
directly in `const`/`static` position:

```rust
use rshooks::XFL;
use rshooks::xfl::XFL as XflType;

const ONE: XflType = XFL!(1);
static REWARD_DELAY: XflType = XFL!(2600000);
assert_eq!(ONE.raw_bits(), 6_089_866_696_204_910_592);
assert_eq!(REWARD_DELAY.raw_bits(), 6_199_553_087_261_802_496);
```

The macro parses the literal's text by hand — integer arithmetic on the
digit string — rather than going through `f64`, which is the whole point:
bit-exactness for every representable decimal, not an approximation.

**Grammar.** An optional leading `-`, then exactly one numeric literal
token: a plain integer (`123456789`, optionally with `_` separators like
`1_000_000`), a decimal (`0.1`, `1.`, `1.50`), or either with a decimal
exponent (`1e-5`, `2.6E6`, `1e+3`). Trailing zeros are normalized away, so
`1.50`, `1_000`, and `2600000` encode exactly as if written `1.5`, `1e3`,
and `2.6e6`.

**What gets rejected**, always as a `compile_error!`, never a panic:

- anything that is not a single numeric literal token — a string/char/byte
  literal, a hex/octal/binary integer (`0x..`/`0o..`/`0b..`), missing input,
  or extra tokens
- a numeric type suffix (`1i64`, `1.0f64`) — `XFL!` always produces its own
  `i64` expansion, so a suffix can only be a mistake
- more than 16 significant decimal digits after trailing-zero normalization
  — XFL's mantissa cannot hold them, and the macro never silently rounds
- a magnitude outside XFL's representable range, roughly `1e-81` to `1e96`
  (unbiased exponent bounds `-96..=80`) — reported as a distinct "too small"
  or "too large" message

```rust,compile_fail
// More than 16 significant digits.
rshooks::XFL!(1.2345678901234567);
```

```rust,compile_fail
// Magnitude too large to represent.
rshooks::XFL!(1e96);
```

## Checked arithmetic

`XFL` implements `Add`, `Sub`, `Mul`, `Div`, and `Neg` — but every one of
these has `Output = Result<XFL, HookError>`, not a bare `XFL`. There is no
local arithmetic: `self + rhs` issues a `float_sum` host call,
`self * rhs` issues `float_multiply`, and so on. `Sub` is built from `Neg`
plus `float_sum` (there is no dedicated `float_subtract` host function), and
`Neg` is a real `float_negate` round trip, never a local sign-bit flip.

```rust,ignore
let remaining = match amount - share {
    Ok(x) => x,
    Err(_) => rollback!(b"xfl-math: amount - share failed", ...),
};
```

(from `examples/07_xfl-math`). Because every operator's `Output` is a
`Result`, `rshooks` also implements the mixed combinations
`Result<XFL> op XFL` and `XFL op Result<XFL>`, so a chain that alternates a
plain `XFL` in on each side short-circuits on the first error without an
explicit `?` between every step. (Rust's orphan rules forbid `Result` on
*both* sides of one of these impls, so an independently-fallible value on
each side still needs a `?` first.)

`mulratio(round_up, num, den)` computes `self * (num / den)` in one host
call — used for percentage-style scaling:

```rust,ignore
let share = amount.mulratio(false, 1, 100)?; // 1% of `amount`, rounding down
```

It takes two extra scale parameters beyond a plain `rhs`, so it stays a
named method rather than trying to fit an operator shape.

## Comparison: methods and operators

`.eq(rhs)`, `.lt(rhs)`, `.gt(rhs)`, and `.compare(rhs, mode)` all return
`Result<bool>`, backed by the fallible `float_compare` host call.
`.compare()` takes a bitmask (`COMPARE_EQUAL`, `COMPARE_LESS`,
`COMPARE_GREATER`, freely combined — e.g. `COMPARE_LESS | COMPARE_EQUAL` for
`<=`, since there's no dedicated `le`/`ge` method).

`XFL` also implements `PartialEq`/`PartialOrd` (`==`, `<`, `>`, ...),
forwarding to those same methods — but these traits have a fixed
`bool`/`Option<Ordering>` return type with no room for an `Err`, so on a
`float_compare` failure they fall back to `false`/`None`, the same
convention `f64` uses for `NaN`. That is the wrong choice whenever a
comparison gates a rollback decision on an operand that hasn't been
separately validated — silently treating "the comparison failed" the same
as "not below the minimum" would mean accepting a transaction the hook
never actually validated. Prefer the `Result`-returning methods, matched
three ways, for exactly those cases:

```rust,ignore
match share.lt(min_share) {
    Ok(true) => rollback!(b"xfl-math: computed share below minimum", ...),
    Ok(false) => {}
    Err(_) => rollback!(b"xfl-math: comparison failed", ...),
}
```

The operators are reasonable specifically when both operands are already
host-validated `XFL` values with no realistic path to a `float_compare`
failure — a pure sanity check where "incomparable" and "not greater than"
deserve identical handling:

```rust,ignore
if compounded > remaining {
    rollback!(b"xfl-math: compounded share unexpectedly exceeds remaining amount", ...);
}
```

(`compounded` and `remaining` here only exist because two earlier `Result`s
already returned `Ok` — see `examples/07_xfl-math`'s README for the full
reasoning on when each style applies.)

## Reading an Amount field as XFL

The typed slot layer (see [Slots and Ledger Objects](slots.md)) reads an
`Amount` field as XFL directly, working identically whether the amount is
native (XRP/XAH) or an IOU:

```rust,ignore
let txn = SlotObject::from_otxn()?;
let amount: XFL = txn.get(sfAmount)?.as_xfl()?;
```

`SlotObject<Amount>::as_xfl` is a direct `slot_float` call. For a **native**
amount, the result comes back in **XAH units**, not drops — the host builds
it from the drop count as mantissa with exponent `-6`, then normalizes.
Recover the drop count with `xfl.to_int(6, false)`.

An equivalent route using the raw numbered slot API, reading the same field
without the typed layer:

```rust,ignore
let txn_slot = otxn_slot(0)?;
let amount_slot = slot_subfield(txn_slot, sfAmount, 0)?;
let amount = XFL::from_slot(amount_slot)?;
```

Both call the same host function under the hood; the typed form just needs
no slot numbers. `XFL::sto`/`XFL::sto_set` are the corresponding
encode/decode pair for a serialized `Amount` buffer that isn't already in a
slot (e.g. building a transaction's own `Amount` field by hand).

When the amount already came back as an `AmountBytes::Iou` — from
`otxn_field_typed(sfAmount)` or a `views::tx` accessor (see [Typed
Views](views.md)) — `IouAmount::xfl()` decodes its value straight from those
bytes, with no slot at all:

```rust,ignore
let AmountBytes::Iou(iou) = otxn_field_typed(sfAmount)? else {
    accept!(); // native, not handled here
};
let value: XFL = iou.xfl()?;
```

It hands the host exactly the amount's 8-byte value component via
`XFL::sto_set`, never the full 48 bytes and never a local bit-reinterpret —
the wire value component sets an always-on "not native" flag bit a real XFL
never sets, so either shortcut would produce a wrong result.

## `XFLUnchecked` for hot paths

`rshooks::xfl_unchecked::XFLUnchecked` is the deferred-validation
counterpart to `XFL`: every operator is still a real host round trip — there
is no local fast path — but with **no guest-side `Result` branch** between
steps. A poisoned or invalid operand propagates through the host calls (an
invalid input is rejected by the host and the result is `INVALID_FLOAT`'s
own raw bits, itself a valid poison value to keep propagating), and a single
`.validate()` call at the end turns the final raw value into a real
`Result<XFL, HookError>`:

```rust,ignore
let compounded_raw =
    share.unchecked() * growth.unchecked() * growth.unchecked() * growth.unchecked();
let compounded = match compounded_raw.validate() {
    Ok(x) => x,
    Err(_) => rollback!(b"xfl-math: compounded share failed to validate", ...),
};
```

(from `examples/07_xfl-math`.) `XFL::unchecked()` is a zero-cost
reinterpretation into `XFLUnchecked` — no host call — and the two types mix
freely at a chain's boundary (`XFLUnchecked op XFL` and `XFL op
XFLUnchecked` are both implemented, treating the `XFL` side as implicitly
unchecked), so the usual shape is: start from a known-valid `XFL`, run the
hot loop in `XFLUnchecked`, validate once at the end.

`validate()` is implemented as `float_sum(self, 0)` — a real host round
trip, not a guest-side range check — specifically so it reuses the host's
own validation gate instead of re-deriving XFL's mantissa/exponent rules
locally and risking drift.

This is only worth reaching for on a measured hot path. Benchmarked against
a checked `Result`-chain of the same operations, `XFLUnchecked`'s marginal
cost per chained multiply matches a hand-written raw host-call chain
exactly (`+3` instructions/op vs. the checked chain's `+14`); for `Sub`
(two host calls per step, since `Neg` isn't free), it's `+5` vs. `+27`. The
win is entirely about *when* validation happens — once, at the end, instead
of once per step — never about skipping any host validation a correct
implementation actually needs. Use `XFL`'s checked operators by default,
and reach for `XFLUnchecked` only once a chain like this is the measured
bottleneck.
