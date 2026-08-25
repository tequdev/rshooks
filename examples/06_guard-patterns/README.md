# guard-patterns

A teaching example, not a realistic policy on its own: it exists to show
correct `guard!`/`guard_m!` usage, how to reason about `maxiter`, and the
`[u8; N] == [u8; N]` compiler-generated-loop pitfall — with real, measured
numbers from this repo's own toolchain, not just prose.

## What you'll learn

1. How to write a hand-guarded loop with a **provably exact** `maxiter`.
2. Why comparing fixed-size byte arrays with `==` is a trap on
   `wasm32v1-none`, and how to avoid it entirely (rather than reach for
   `--auto-guard` after the fact).
3. What `guard_m!`'s `$n` disambiguator is actually protecting against —
   verified by deliberately breaking it and observing what does (and does
   not) catch the mistake.
4. How a loop's `maxiter` shows up in `rshooks-build`'s reported worst-case
   instruction count (WCE), with this crate's own measured numbers.

## The hook, briefly

Same shape as `firewall`: read the otxn sender, read a `BL` Hook parameter
(the blocked account), reject on a match. The interesting part is *how*
the comparison and a couple of extra demonstration loops are written.

## 1. A hand-written loop with an exact `maxiter`

```rust
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

`ACC_ID_LEN` is `20` — a compile-time fact about a fixed-size array, not a
runtime-dependent bound. That makes `maxiter = 20` not just *a safe
bound* but *the exact worst case*: this loop can never run more than 20
times, and choosing anything smaller would be wrong (it could run exactly
20 times, e.g. two accounts that differ only in their last byte), while
choosing anything larger would just inflate the reported worst-case
instruction count for no benefit. **How to choose `maxiter`, in general:**
work from a bound you can actually justify from the data's shape (a fixed
array's length, a documented protocol limit, a value read from
`hook_param` and validated) — never from "a number that felt safe," which
is exactly the trap `--auto-guard`'s default of 16 sets (see §2).

## 2. Why not `==`

It's tempting to write `*a == *b` instead of the loop above. That's
actually fine today: `a`/`b` are `&AccountId`, and `AccountId`'s own
`PartialEq` is hand-written to delegate to `buf_eq_20` internally, so
`*a == *b` is already loop-free. The pitfall this section teaches is about
the **bare `[u8; 20]` buffer** underneath — exactly what `accounts_equal`'s
hand-written loop above compares via `a.get(i)`/`b.get(i)` instead of
`==`. On `wasm32v1-none` (WASM MVP only — no bulk-memory instructions),
LLVM at `opt-level = "z"` lowers a bare `[u8; 20]` equality check to a call
into a `compiler_builtins` `bcmp`-style function containing a real,
unguarded loop — one that never appears as a `loop` keyword anywhere in
the crate's own source. `firewall` (`examples/05_firewall`) hit exactly
this in an earlier version that wrote `sender == blocked` directly, and as
a result needed:

```sh
cargo run -p rshooks-build -- build --manifest-path examples/05_firewall/Cargo.toml \
  --auto-guard --default-maxiter 24
```

`--auto-guard`'s own default (`--default-maxiter 16`) would build
successfully for that same loop yet risks a real on-ledger
`GUARD_VIOLATION`: the compare can run up to 20 iterations, one more than
16 covers. `firewall`'s README works through the exact reasoning for `24`
— though `firewall`'s current source needs none of this, since it compares
with `buf_eq_20` explicitly (and its `AccountId`s' own `==` would be
loop-free too, same as here). This example's `accounts_equal` sidesteps
the whole problem another way: because the loop is hand-written, its
`guard!` is present in the source and its `maxiter` is exact —
`rshooks build` needs **no extra flags at all** for this function.

## 3. `guard_m!`: what it actually protects against

```rust
let mut i: usize = 0; let mut sum_a: u32 = 0;
loop { guard_m!(8, 1); /* ... */ }
let mut j: usize = 0; let mut sum_b: u32 = 0;
loop { guard_m!(8, 2); /* ... */ }
```

These two loops are deliberately written on **one physical source line**
each — not realistic formatting, but the clearest way to show exactly when
`guard_m!` (rather than plain `guard!`) is needed: `guard!`'s id formula is
`(1 << 31) + line!()`, so two textually-distinct loops sharing one source
line would otherwise collide on the same id. `guard_m!`'s formula,
`(1 << 31) + (line!() << 16) + n`, folds the extra `n` in to keep them
apart. In real (non-teaching) code this situation comes from *generated*
code — e.g. a macro like `rshooks::txn_template!` that expands to more
than one loop at a single call site — not from manually cramming code onto
one line.

**What actually happens if you get this wrong** was checked empirically,
not assumed: changing one of the two `guard_m!(8, 1)`/`guard_m!(8, 2)`
calls above so both loops share id `1` still passes `rshooks build`
(and `check`) without any error — the static checker only verifies loop
*shape* (a guard call at the top of every loop), never that ids are
unique across the module. The real hazard is a **runtime** one: `_g`
tracks each guard id's iteration count as the hook actually executes, so
two unrelated loops sharing an id would share one counter — whichever runs
first could push it toward the *other* loop's `maxiter`, risking a
spurious on-ledger `GUARD_VIOLATION` with no build-time tool able to catch
it. That's the actual reason `$n` exists, and why it's worth getting right
even though nothing here will stop you if you don't.

## Measured worst-case instruction count

Numbers from this repo's own `rshooks build` output (they'll drift a
little across compiler and `rshooks-build` pipeline versions — these are
current-toolchain measurements, not a guarantee). The `#[hooks]` per-index
build pipeline compiles each entry in isolation via a dedicated
`--cfg`-selected build, at this workspace's `opt-level = 3` default
(`examples/Cargo.toml`, see `docs/DESIGN.md` §2 C6):

| Build | worst-case instructions (`hook=`) | size |
|---|---:|---:|
| `accounts_equal` only (the two `guard_m!` loops removed) | 230 | 746 bytes |
| Full hook (`accounts_equal` + both `guard_m!` loops) | **352** | **1015 bytes** |

The two `maxiter = 8` demonstration loops together add **122**
instructions to the worst case (`352 - 230`) at this optimization level.
The reason is `opt-level = 3` itself: LLVM unrolls a small, compile-time-bounded loop like these
(`maxiter = 8`) into straight-line code rather than leaving it as a real
`loop` construct, so the guard checker's worst-case analysis no longer has
to multiply the loop body's cost by `maxiter` — it just counts the
unrolled instructions once. The underlying lesson from §1 still holds
**whenever a loop isn't unrolled** (which `guard!`'s `maxiter` still
governs directly, and which is exactly what happens for `accounts_equal`'s
own `ACC_ID_LEN`-bounded loop here, and for any loop LLVM doesn't choose to
unroll): **WCE scales with `maxiter`** for a loop that stays a loop, because
the checker's worst-case count has to assume every guarded loop actually
runs its full `maxiter` iterations. A needlessly large `maxiter` still
risks hiding real bugs regardless of whether the loop ends up unrolled —
only its effect on the *reported number* changes with optimization level,
not the underlying correctness argument for choosing `maxiter` precisely.

**One nuance to this file's own "unrolling drops WCE" observation**: that
holds for a small loop unrolled *on its own*. If the loop being unrolled
*wraps* another `guard!`-protected loop, unrolling instead physically
duplicates that inner loop once per outer iteration, and the checker then
charges its full cost per duplicate — silently multiplying, not
amortizing, the inner loop's contribution. `examples/80_governance` hit
exactly this; see the book's
[Guards and Loops](../../book/src/concepts/guards.md#a-nested-guarded-loop-pitfall-unrolling-that-duplicates-the-inner-loop)
page and `rshooks::no_unroll`'s doc comment for the full mechanism and the
fix.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/06_guard-patterns/Cargo.toml
```

No extra flags needed: both `accounts_equal` and the two `guard_m!` demo
loops are hand-written and guarded in the source; there is no
compiler-generated loop anywhere in this crate to auto-guard.

## Expected behavior

- `BL` not configured (or not 20 bytes) → accept (nothing to block).
- `BL` configured and matches the otxn sender → rollback
  (`"guard-patterns: blocked account"`, code `2`).
- `BL` configured and doesn't match → accept, with the accept code set to
  the sum of the two demonstration loops' outputs (not meaningful hook
  logic on its own — see the code comment).

## Error codes

`GuardPatternsError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback!` code for each failure this hook can exit with:

| variant | code | meaning |
|---|---|---|
| `CouldNotReadSender` | 1 | `otxn_field(sfAccount)` did not return a 20-byte `AccountId` |
| `BlockedAccount` | 2 | the sender matched the `BL`-configured blacklist account |
