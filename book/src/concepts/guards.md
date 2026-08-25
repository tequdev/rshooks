# Guards and Loops

The Hook host statically rejects any wasm module containing a loop it
cannot prove terminates. Every loop — every one, including loops the
compiler generates that never appear as a `loop` keyword in your Rust
source — must call the host's `_g` guard function at its top, declaring an
upper bound on its iteration count. This page covers `guard!` and
`guard_m!`, the compiler-generated-loop pitfall that catches most people
off guard the first time, and the two source-level idioms `rshooks` hooks
use to avoid it entirely.

## Why every loop needs a guard

The Hook API's static guard check exists so a malicious or buggy hook
can't wedge a validator in an infinite (or merely too-expensive) loop
during transaction processing. Before a Hook binary can be installed, the
host's guard checker walks every loop in the module and confirms it begins
with a call to `_g(guard_id, maxiter)` — a declaration of "this loop will
run at most `maxiter` times." At runtime, `_g` tracks each guard id's
actual iteration count as the hook executes, and the host aborts execution
with `GUARD_VIOLATION` if a loop ever exceeds the `maxiter` it declared.

`rshooks` exposes this through two macros that match the C `GUARD`/
`GUARDM` macros' id and iteration-count formulas exactly, so the `unsafe`
call to `_g` lives inside the macro expansion — hook code never writes
`unsafe` for this.

## `guard!` and `guard_m!`

`guard!(maxiter)` goes at the very top of a loop body:

```rust
use rshooks::guard;

let mut i = 0;
loop {
    guard!(10);
    if i >= 3 {
        break;
    }
    i += 1;
}
assert_eq!(i, 3);
```

`maxiter` is the largest number of times this loop can possibly execute —
`guard!` itself adds `+ 1` internally to match the C macro's exact formula,
so you supply the true iteration bound, not an off-by-one-adjusted value.
Choosing `maxiter` well means working from a bound you can actually justify
from the data's shape: a fixed array's length, a documented protocol
limit, or a value read from `hook_param` and validated before use — never
"a number that felt safe." From `examples/06_guard-patterns`:

```rust,ignore
fn accounts_equal(a: &AccountId, b: &AccountId) -> bool {
    let mut i: usize = 0;
    loop {
        guard!(ACC_ID_LEN as u32); // maxiter = 20, exact
        if i >= ACC_ID_LEN {
            break true;
        }
        if a.get(i) != b.get(i) {
            break false;
        }
        i = i.wrapping_add(1);
    }
}
```

`ACC_ID_LEN` is `20`, a compile-time fact about a fixed-size array — so
`maxiter = 20` is not just *a* safe bound, it's the exact worst case. A
smaller value would be wrong (this loop really can run 20 times, e.g. two
account IDs differing only in their last byte); a larger value would just
inflate the hook's reported worst-case instruction count for no benefit.

`guard_m!(maxiter, n)` is for the rare case where two textually distinct
loops share one physical source line — `guard!`'s id formula,
`(1 << 31) + line!()`, would otherwise collide for both. The extra `n`
disambiguates them:

```rust,ignore
let mut i: usize = 0; let mut sum_a: u32 = 0;
loop { guard_m!(8, 1); /* ... */ }
let mut j: usize = 0; let mut sum_b: u32 = 0;
loop { guard_m!(8, 2); /* ... */ }
```

In real (non-teaching) code this situation arises from *generated* code —
a macro like `rshooks::txn_template!` that expands to more than one loop
at a single call site — rather than from manually cramming code onto one
line.

**What `$n` does and doesn't protect against**, verified empirically by
`examples/06_guard-patterns`: giving both loops above the same `n` (so
they collide on one guard id) still passes `rshooks build`/`check`
without any error — the static checker only verifies loop *shape* (a guard
call at the top of every loop), never that ids are unique across the
module. The real hazard is a **runtime** one: `_g` tracks each guard id's
iteration count as the hook actually executes, so two unrelated loops
sharing an id share one counter — whichever runs first pushes it toward
the *other* loop's `maxiter`, risking a spurious on-ledger
`GUARD_VIOLATION` that no build-time tool catches. That's the actual
reason `$n` exists.

## The compiler-generated-loop pitfall

The trap that catches most people writing Rust hooks for the first time:
some Rust operations lower to a call into a `compiler_builtins` function
containing a real, unguarded loop, *even though no loop appears in your
source at all*. On `wasm32v1-none` (the WASM MVP target, with no
bulk-memory instructions), this happens for:

- **Fixed-size array/slice equality** — `[u8; N] == [u8; N]` lowers to a
  `bcmp`-style byte-compare loop. (The protocol newtypes in
  `rshooks::types` — `AccountId`, `Hash`, `Keylet`, and the rest — are the
  exception: their `PartialEq` is hand-written to call the matching
  `buf_eq_*` internally, so `==` between two of them is already loop-free.
  This pitfall is specifically about comparing bare `[u8; N]` arrays.)
- **Large buffer zero-init or copy** — a big stack-local `[0u8; N]`, or a
  large `memcpy`-shaped copy, lowers to a `memset`/`memcpy`-style loop.

`examples/05_firewall` used to hit exactly this: an earlier version of its
account comparison, written as `sender == blocked`, needed to be built
with:

```sh
cargo run -p rshooks-build -- build --manifest-path examples/05_firewall/Cargo.toml \
  --auto-guard --default-maxiter 24
```

`rshooks build` defaults to treating an unguarded loop as a hard
build error — missing a `guard!` in your own code is a bug, not something
to silently paper over. `--auto-guard` is the escape hatch for loops the
guard checker finds that your source never wrote. `examples/05_firewall`'s
current source (`examples/05_firewall/src/lib.rs`) avoids all of this: it
compares with `buf_eq_20` explicitly and needs no extra flags — and today,
`AccountId`'s own `==` would itself already be loop-free too (see the
callout above), since its `PartialEq` delegates to `buf_eq_20`
internally.

### The two idioms that avoid it

Rather than reach for `--auto-guard` after the fact, two source-level
idioms sidestep the compiler-generated loop entirely, and are preferred
wherever they apply:

**Fixed-size buffer equality** — `rshooks::buf_eq_8`/`_20`/`_32`/`_33`/
`_34`/`_40`/`_48`/`_64` compare a buffer as a fixed sequence of word-sized
(`u64`, with a narrower tail word where the size isn't a multiple of 8)
chunks, built from source-level literal byte indices. The comparison is
genuinely straight-line code — there is nothing for LLVM to lower into a
loop:

```rust,ignore
if buf_eq_20(&sender, &blocked) {
    rollback!(
        b"guard-patterns: blocked account",
        GuardPatternsError::BlockedAccount
    );
}
```

Historically, switching `firewall`'s `sender == blocked` from a derived
array comparison to `buf_eq_20` removed both the loop and the
`--auto-guard` flag entirely, and the word-at-a-time comparison further
dropped its worst-case instruction count from 419 to 122. `buf_eq_20` is
still what the example calls today, and the measurement still holds — it's
just no longer the *only* loop-free option for two `AccountId`s, since the
type's own `==` now delegates to `buf_eq_20` as well (see the callout
above).

**Statics for templates and large buffers** — covered in
[Anatomy of a Hook](anatomy.md#statics-for-templates-and-large-buffers):
`HookStatic` moves a template or large buffer into a data segment or BSS
instead of runtime store chains or a `memset` loop, which removes the
`memcpy`/`memset`-shaped compiler-generated loop the same way `buf_eq_*`
removes the `bcmp`-shaped one. Applying this idiom to `emit-txn` removed
its only compiler-generated loops entirely and cut its worst-case
instruction count by an order of magnitude (6798 → 331, at this
toolchain's `opt-level = 3` default — exact numbers drift a little
between compiler versions).

## When `--auto-guard --default-maxiter` is the last resort

`--auto-guard` remains available for cases neither idiom above covers, but
treat it as a last resort, not a default habit — it is a real footgun for
one specific reason: **the CLI only validates guard shape, not that
`maxiter` covers the loop's true runtime bound.** An under-sized
`--default-maxiter` builds clean — the guard checker sees a syntactically
valid guard call at the top of the loop and is satisfied — and then fails
with `GUARD_VIOLATION` only later, on a live node, the first time the
loop's actual input pushes it past the value you guessed.

`firewall`'s own README works through this concretely: `--auto-guard`'s
own default (`--default-maxiter 16`) would build successfully for its
20-byte account comparison, yet risks a real on-ledger `GUARD_VIOLATION`,
since the compare can run up to 20 iterations — four more than 16 covers.
Getting to a safe `24` there meant reasoning about the loop's true
worst-case bound from first principles, not trusting the flag's default.
If you do reach for `--auto-guard`, size `--default-maxiter` from the
loop's true worst-case iteration count — found via disassembly, not
guessed — every time.

## A nested-guarded-loop pitfall: unrolling that duplicates the inner loop

The worst-case-instruction-count model a nested `guard!` relies on assumes
each `guard!` call site compiles to exactly one physical loop in the final
module: an inner loop's cost is meant to be amortized across every
iteration of its outer loop (the outer loop's own `guard!` already bounds
how many times that can happen), so the checker only has to charge that
inner loop's cost once, at the multiplier its `maxiter` implies relative
to its parent's.

At `opt-level = 3`, that assumption can quietly break. LLVM routinely
fully unrolls a small, provably-bounded outer loop (2 or 3 iterations is a
typical threshold) whenever it judges duplicating the body worthwhile —
independent of whether that body is written inline or behind a function
call, since Guard-type hooks force-inline every reachable function into
`hook()`/`cbak()` regardless (`docs/DESIGN.md` §6.2b), so unrolling and
inlining compound. When the outer loop wraps a `guard!`-protected inner
loop, unrolling physically duplicates that inner loop once per outer
iteration. The checker walks the *compiled* bytecode, not the source, so
it then counts the inner loop's full worst-case cost once per duplicate
instead of once total — silently **multiplying**, not amortizing, its
contribution to the worst-case instruction count.

The actual driver is what happens to the *parent* loop, not the duplicate
count by itself: the checker's multiplier for a loop is its own `maxiter`
divided by whatever its immediate parent's iteration bound is, so
duplicated copies that stay nested under a real parent loop are still
charged at that parent-divided rate (three copies nested under a
`guard!(2)` outer loop each cost `66/2`, the same total as one copy would)
— no penalty from duplication alone. The penalty shows up specifically
when unrolling removes the parent loop entirely: each duplicate then sits
at the top level, with no parent to divide by, so its multiplier jumps
from `66/2` to the loop's full, undivided `66/1` — once per duplicate.

`examples/80_governance` hit this directly: an outer 2-iteration table
loop wrapping a `guard!(66)`-bounded 32-topic scan got fully unrolled,
doubling the inner loop's measured cost — worth roughly a third of the
whole `govern` entry's worst-case instruction count. `rshooks::no_unroll`
fixes it by routing the outer loop's induction variable through
`core::hint::black_box` at its comparison, which makes the trip count
opaque to the optimizer and keeps the loop as one real `loop` construct:

```rust,ignore
use rshooks::{guard, no_unroll};

let mut tbl = 1u8;
while no_unroll(tbl) <= 2 {
    guard!(2);
    // ... the inner guard!(66)-protected loop goes here, once ...
}
```

This is the mirror image of the compiler-generated-loop pitfall above:
that section is about the compiler turning *no* loop into one that needs a
guard; this one is about the compiler turning *one* guarded loop into
several, silently. Only reach for `no_unroll` at a call site actually
exhibiting this shape — an outer loop, itself small enough to be a
plausible full-unroll candidate, wrapping further `guard!`-protected work
— applying it to every guarded loop by default would regress the common
case, where full unrolling is *cheaper* (straight-line code has no loop
overhead and no worst-case-padding waste). See `no_unroll`'s own doc
comment (`crates/rshooks/src/macros.rs`) for the full reasoning, including
its failure mode if a future toolchain ever stopped honoring
`black_box`'s optimization-barrier hint.

## Where to go next

- [Anatomy of a Hook](anatomy.md) covers the `HookStatic` idiom in full,
  including the safety argument for its take-once exclusivity.
- [Accept, Rollback, and Errors](errors.md) covers how a hook actually
  terminates once its checks — guarded loops included — are done.
- [The rshooks CLI](../build/cli.md) covers `--auto-guard` and
  `--default-maxiter` as build flags in full.
