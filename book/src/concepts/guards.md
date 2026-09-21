# Guards and Loops

The Hook host statically rejects any wasm module containing a loop it
cannot prove terminates. Every loop — every one, including loops the
compiler generates that never appear as a `loop` keyword in your Rust
source — must call the host's `_g` guard function at its top, declaring an
upper bound on its iteration count. This page covers `guard!` and
`guard_m!`, the loop-rotation pitfall that can move a correctly written
guard away from the top of the compiled loop, the compiler-generated-loop
pitfall that catches most people off guard the first time, and the
source-level idioms `rshooks` hooks use to avoid all of it.

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
"a number that felt safe." From `examples/80_governance`:

```rust,ignore
let mut i = 0u8;
while i < member_count {
    guard!(u32::from(SEAT_COUNT)); // maxiter = 20, exact
    let this_seat = i;
    i = i.wrapping_add(1);
    // ... reads the `IS<seat>` hook parameter for this seat ...
}
```

`SEAT_COUNT` is `20`, a compile-time governance constant, and an earlier
check already rejects any `member_count` greater than it — so
`maxiter = u32::from(SEAT_COUNT)` is not just *a* safe bound, it's the
exact worst case this loop can ever reach. A smaller value would be wrong
(a chain can configure up to 20 seats); a larger value would just inflate
the hook's reported worst-case instruction count for no benefit.

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
`GUARD_VIOLATION` that no build-time tool catches; `rshooks-testenv`
reports it at unit-test time instead, since its own `_g` enforces the
same cumulative per-id budget. That's the actual reason `$n` exists.

## wasm-opt block-wrapping and LLVM loop rotation

A guard written correctly at a loop's top in Rust source can still end up
somewhere other than the very first instruction after the compiled `loop`
opcode. Distinct compiler behaviors cause this, at different stages of
`rshooks build`'s pipeline: `rshooks build` compensates for one of them
automatically; the other needs a source-level idiom.

### 1. `wasm-opt -Oz` wraps the loop body in a `block`

`rshooks build` runs Binaryen's `wasm-opt -Oz` size optimization first, on
each entry's raw per-entry wasm, before cleaning (`optimizer.rs`). For a
loop whose body contains an internal early-exit branch — a `continue`, or
an `if`/`?` check on a `Result` a Hook API call returned — `wasm-opt` often
restructures it into `loop { block { <guard>; .. } }`: the break/continue
logic is wrapped in a `block` so a `br`/`br_if` can jump to its end, and
that `block` lands first inside the `loop`, ahead of the guard call that
was the loop body's first statement in source. The guard itself is
untouched — still unconditional, still the first *real* instruction the
loop runs every iteration — only its position relative to `loop` moved,
which the checker's exact `loop; i32.const; i32.const; call $_g` prologue
match doesn't tolerate on its own.

`rshooks build` runs a guard-hoist pass (`crates/rshooks-build/src/
guard_hoist.rs`) after unnesting and before the guard check specifically to
undo this: it moves a guard prologue found just inside one or more leading
empty `block`s back out to sit directly after `loop`, which is always
semantically identical (the prologue is stack-neutral and no label inside
the `block`s targets it). This runs automatically, for every `rshooks
build`/`rshooks clean`, with no source change required — a hook author
writing an ordinary `loop { guard!(N); if !cond { break } body }` or `while
cond { guard!(N); body }` never needs to think about this case.

### 2. LLVM loop rotation moves the guard to the loop's latch

Separately, at `opt-level = 3`, LLVM's own loop-rotation pass can turn
either guard-writing form shown above into a do-while: it duplicates
the loop's header block (the condition check) into the preheader ahead of
the loop, and moves the original header to the loop's latch, after the
body, just before the branch back. Which source form stays "guard-first"
after this depends on which block LLVM treats as the header — an LLVM
decision the source doesn't control:

- `loop { guard!(N); if !cond { break } body }`'s guard is the first
  statement of the loop body, so it *is* the header. Rotation duplicates it
  into the preheader and moves the original past `body`, into the latch —
  the compiled `loop` opcode is then followed by `body`, not by the guard,
  and the checker rejects it as missing a guard.
- `while cond { guard!(N); body }`'s condition is the header, with the
  guard as the body's first statement. Rotation moves the condition to the
  latch, so the guard ends up leading the rotated loop — this passes.

But rotation only fires when LLVM judges the header "small" (a cheap
condition check); a large or expensive header (many arithmetic/memory
operations) is left un-rotated, and then it's the `while` form that fails
instead. Neither fixed source form is guard-first under both outcomes, and
nothing in the source indicates which outcome a given loop will get — the
guard-hoist pass above can't help here either, since there's no leading
`block` to hoist out of: the guard is simply absent from the top of the
compiled loop, moved to its latch.

`rshooks::guarded_while!(maxiter, cond, { body })` sidesteps the question
by placing a guard in both positions — the condition block and the top of
the body — so whichever one rotation leaves leading the compiled loop, that
block already starts with a guard call. The cost is one extra `_g` call per
iteration. See its rustdoc (`crates/rshooks/src/macros.rs`) for the full
mechanism and the guard-id convention it uses (`guard_m!` ids `1` and `2`
on the macro's own invocation line). `continue` inside its body jumps back
to the condition block, which is guarded, so it stays covered too.

### Diagnosing which one you're looking at

`rshooks build`'s unguarded-loop error names this shape directly when it
recognizes it — a `_g` call inside the loop's body that isn't at its head.
Since the build pipeline's guard-hoist pass already runs before this check,
a report reaching you from `rshooks build` is case 2 (rotation): the block
case was already fixed automatically. `rshooks check` on an already-built
file calls the validator directly, with no hoist pass, so either cause is
still possible there.

To tell them apart by hand:

- `rshooks build --no-optimize` skips `wasm-opt` entirely. If the same loop
  now passes, the failure was case 1 (`wasm-opt` block-wrapping) — LLVM's
  own output was already guard-first. If it still fails, it's case 2
  (rotation).
- `wasm-tools print <entry>.wasm` on the raw per-entry wasm (see below for
  how to obtain it) shows the signature directly: a guard id (`i32.const
  <id>`) appearing **twice** — once immediately before the `loop` opcode
  (the duplicated preheader copy) and again partway through the loop's
  body, at its latch — is rotation (case 2). A single `block` opener
  immediately after `loop`, with the guard as the block's first
  instruction, is the `wasm-opt` shape (case 1) — already fixed by the time
  `rshooks build` reports anything, so this is only visible with
  `--no-optimize` off and inspecting an intermediate stage, or by disabling
  the hoist pass.

The raw per-entry wasm isn't kept by default; rebuild it directly with the
`cargo rustc` invocation `rshooks build` itself uses, e.g.
`cargo rustc --cfg rshooks_entry="0" --check-cfg
'cfg(rshooks_entry,values("0","1","2","3","4","5","6","7","8","9"))'
--target wasm32v1-none --release -- -C link-arg=-zstack-size=<bytes>`
(`crates/rshooks-build/src/chain_build.rs`'s `selected_rustc_args`/
`cargo_args` print the exact, current flags).

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

`rshooks build` treats an unguarded loop as a hard build error — missing
a `guard!` in your own code is a bug, not something to silently paper
over — so a compiler-generated loop like this needs a source-level fix,
not a build flag. `examples/05_firewall/src/lib.rs` compares accounts with
`buf_eq_20` explicitly for exactly this reason, and `AccountId`'s own `==`
is itself already loop-free too (see the callout above), since its
`PartialEq` delegates to `buf_eq_20` internally.

### The two idioms that avoid it

Two source-level idioms sidestep the compiler-generated loop entirely, and
are preferred wherever they apply:

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

`firewall`'s account comparison calls `buf_eq_20` for this reason: it
removes the compiler-generated loop entirely, and the word-at-a-time
comparison keeps its worst-case instruction count well below a derived
array comparison's. `AccountId`'s own `==` delegates to `buf_eq_20` as
well (see the callout above), so either spelling is loop-free.

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
- [The rshooks CLI](../build/cli.md) covers the full `build`/`clean`/`check`
  flag reference.
