# typed-results

## What you'll learn

The typed entry-return form (`.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md`):
every `#[hook]`/`#[cbak]` entry returns `HookResult` (a
`Result<Accept, Rollback>` alias), and `?` can propagate failures out of
ordinary helper functions — in place of a hand-written `accept!`/`rollback!`
call at every failure point. This crate declares one chain with both styles
side by side, both `-> HookResult`: `deposit` uses the idiomatic `?`/`Ok`
form, and `reset` uses the raw `accept!`/`rollback!` escape hatch directly
inside its `HookResult`-returning body — proving that style stays
first-class within the typed signature, not just the `?`-based one.
`deposit`'s own `amount` is a declared **signature parameter**
(`docs/PARAM_SIGNATURE_DESIGN.md`) — an extra argument on the entry fn
itself — rather than a hand-rolled `#[otxn_param(..)]` field; see
"Signature parameters: `amount`" below for how its own failure path (a
generated rollback, before the body runs at all) sits alongside the `?`
form this example is really about.

## Code walkthrough

```rust
hook_errors! {
    pub enum DepositError {
        StateSetFailed = 2 => b"typed-results: state_set failed",
    }
}

#[inline(always)]
fn bump_counter(t: &TypedResults, amount: u64) -> Result<u64, DepositError> {
    let count = t.counter.get().unwrap_or(Some(0)).unwrap_or(0);
    let next = count.wrapping_add(amount);
    t.counter.set(&next).map_err(|_| DepositError::StateSetFailed)?;
    Ok(next)
}

#[hook(0, name = "deposit", on = [Invoke])]
fn deposit(&self, amount: u64) -> HookResult {
    let next = bump_counter(self, amount)?;
    Ok(Accept::new(b"typed-results: deposited", next as i64))
}
```

`hook_errors!`'s optional `= <code> => b"msg"` clause (this crate declares
one on its only variant) generates `impl From<DepositError> for Rollback`,
whose message comes from the clause — so `?` alone carries both the code
and the message all the way out to the host `rollback` call, with no
`rollback!(msg, code)` written anywhere in `deposit`. `reset` shows the
alternative style: still `-> HookResult`, but calling `accept!`/`rollback!`
directly in the body — both macros diverge (`-> !`), so they coerce to
`HookResult` with no `Ok(..)`/`Err(..)` wrapping needed.

Two rules this example follows deliberately, both measured in
`.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md`'s T-1 probe (§5):

- **Every helper called on a `?` path is `#[inline(always)]`** (`D4`).
  `bump_counter` is. Probe `p2fix` measured that *without* forcing the
  inline, an otherwise-identical typed entry costs 5 more worst-case
  instructions than its `accept!`/`rollback!` twin (an un-inlined
  call-boundary cost, not a `?`/`Result` cost); force-inlined, the same
  code measured *below* the twin.
- **Never `?` a raw Hook API `Result<_, HookError>` straight into a typed
  entry — `.map_err(..)` it first.** `bump_counter` discards the decoded
  `HookError` this way (`.map_err(|_| DepositError::StateSetFailed)`)
  rather than converting it through some `From<HookError>` chain. Probe P5
  measured why: `HookError::code()` is a 46-arm re-encode match, and a
  `?`-propagated two-hop `HookError → Rollback` conversion cost **3.1x** the
  worst-case instructions and **+67%** the size of the raw-code-check twin.
  `rshooks` does not even offer `From<HookError> for Rollback` (see
  [`rshooks::exit::Rollback`]'s doc comment) — this is the only
  supported shape for a fallible Hook API call inside a typed entry.

## Signature parameters: `amount`

`amount: u64` on `deposit`'s own signature is a declared Hook Parameter
Signature Interface parameter (`docs/PARAM_SIGNATURE_DESIGN.md` §1) —
index `0`, type byte `0x03` (`STI_UINT64`), display name `amount`. The
`#[hooks]`-generated prologue reads and big-endian-decodes it from the
originating transaction's own Hook parameters before `deposit`'s body ever
runs; a missing or wrong-length value rolls back right there, with
`b"rshooks: bad sig param 'amount'"` and code `0` (the argument's own
index) — a *different* failure path from `DepositError::StateSetFailed`
above, which `deposit`'s own body still `?`-propagates through the typed
`HookResult` form once `amount` is already a plain `u64`. This is why
`deposit` no longer needs its own `BadAmount` variant or a `read_amount`
helper: the interface's own auto-rollback replaces both.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/16_typed-results/Cargo.toml
```

No extra flags needed — both entries are guard-clean without `--auto-guard`.

## Unit tests

```sh
cargo test --manifest-path examples/Cargo.toml -p typed-results
```

`tests/deposit.rs` drives both entries through `rshooks_testenv::TestEnv::invoke`
— no wasm build, no node: `deposit`'s accept path (including that the running
total persists across invocations), three rollback paths — a missing `amount`
signature parameter and a wrong-length one (both from the generated prologue
directly, never reaching `DepositError`/`Rollback` at all), and a forced
`state_set` failure (the one path that *does* go through
`?`/`From<DepositError> for Rollback`, with an assertion that `HookExit.msg`
carries the exact msg-clause bytes through that conversion) — and `reset`'s
accept path. See `book/src/testing/unit-tests.md` for the harness this
builds on.

## Error codes

A missing or wrong-length `amount` signature parameter rolls back from the
generated prologue, before `deposit`'s body runs at all — with
`b"rshooks: bad sig param 'amount'"` and code `0` (see "Signature
parameters" above), not from `DepositError`. `DepositError`
(`rshooks::hook_errors!`, see `src/lib.rs`) is the `rollback`/`Rollback`
code and message for every failure `deposit`'s/`reset`'s own bodies can
exit with (`reset` reuses `StateSetFailed` directly, via `rollback!`):

| variant | code | message | meaning |
|---|---|---|---|
| `StateSetFailed` | 2 | `typed-results: state_set failed` | the counter could not be persisted |

## Cost of the typed form, here

Measured (`rshooks build`/`check`, this workspace's `opt-level = 3`
profile), after the conversion to a declared `amount` signature parameter
(`docs/PARAM_SIGNATURE_DESIGN.md`):

| entry | form | worst-case instructions | size | max nesting depth |
|---|---|---:|---:|---:|
| `deposit` (index 0) | typed, idiomatic (`HookResult`, `?`, msg-clause `hook_errors!`, `amount` signature parameter) | 306 | 912 bytes | 1 |
| `reset` (index 1) | typed, raw (`HookResult`, `accept!`/`rollback!`) | 124 | 451 bytes | 1 |

Before the signature-parameter conversion (`AMT` `#[otxn_param(..)]` field
plus the `read_amount` helper, at the same crates, and with
`crates/rshooks-build`'s unreachable-tail DCE pass already in place —
`#76`), `deposit` measured 307 / 906 bytes / nesting 1 — converting to a
signature parameter cost *nothing*: WCE actually moved one instruction
*lower* (307 → 306), size grew a modest 6 bytes, and nesting was unchanged.
`reset` (no signature parameters) is byte-identical before and after.

Both are well within the 32-level nesting budget and the 65,535-byte
SetHook size limit — `deposit`'s higher numbers versus `reset` reflect it
doing strictly more work (a required-parameter read plus a state
read-modify-write, versus `reset`'s single unconditional write), not a cost
the typed form itself imposes; see the design doc's T-1 probe table for the
apples-to-apples comparison (`P1`/`p2fix`/`P3-typed` vs. their
`accept!`/`rollback!` twins at matched logic) that this example's numbers
corroborate rather than repeat.

[`rshooks::exit::Rollback`]: ../../crates/rshooks/src/exit.rs
