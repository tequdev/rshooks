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

## Code walkthrough

```rust
hook_errors! {
    pub enum DepositError {
        BadAmount = 1 => b"typed-results: bad AMT parameter",
        StateSetFailed = 2 => b"typed-results: state_set failed",
    }
}

#[inline(always)]
fn read_amount(t: &TypedResults) -> Result<u64, DepositError> {
    let bytes = t.amount.get_required().map_err(|_| DepositError::BadAmount)?;
    Ok(u64::from_be_bytes(bytes))
}

#[hook(0, name = "deposit", on = [Invoke])]
fn deposit(&self) -> HookResult {
    let amount = read_amount(self)?;
    let next = bump_counter(self, amount)?;
    Ok(Accept::new(b"typed-results: deposited", next as i64))
}
```

`hook_errors!`'s optional `= <code> => b"msg"` clause (this crate declares
one on every variant) generates `impl From<DepositError> for Rollback`,
whose message comes from the clause — so `?` alone carries both the code
and the message all the way out to the host `rollback` call, with no
`rollback!(msg, code)` written anywhere in `deposit`. `reset` shows the
alternative style: still `-> HookResult`, but calling `accept!`/`rollback!`
directly in the body — both macros diverge (`-> !`), so they coerce to
`HookResult` with no `Ok(..)`/`Err(..)` wrapping needed.

Two rules this example follows deliberately, both measured in
`.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md`'s T-1 probe (§5):

- **Every helper called on a `?` path is `#[inline(always)]`** (`D4`).
  `read_amount`/`bump_counter` both are. Probe `p2fix` measured that
  *without* forcing the inline, an otherwise-identical typed entry costs 5
  more worst-case instructions than its `accept!`/`rollback!` twin (an
  un-inlined call-boundary cost, not a `?`/`Result` cost); force-inlined,
  the same code measured *below* the twin.
- **Never `?` a raw Hook API `Result<_, HookError>` straight into a typed
  entry — `.map_err(..)` it first.** `read_amount`/`bump_counter` both
  discard the decoded `HookError` this way (`.map_err(|_| DepositError::X)`)
  rather than converting it through some `From<HookError>` chain. Probe P5
  measured why: `HookError::code()` is a 46-arm re-encode match, and a
  `?`-propagated two-hop `HookError → Rollback` conversion cost **3.1x** the
  worst-case instructions and **+67%** the size of the raw-code-check twin.
  `rshooks` does not even offer `From<HookError> for Rollback` (see
  [`rshooks::exit::Rollback`]'s doc comment) — this is the only
  supported shape for a fallible Hook API call inside a typed entry.

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
total persists across invocations), its two rollback paths (missing `AMT`,
forced `state_set` failure) with an assertion that `HookExit.msg` carries the
exact msg-clause bytes through the `?`/`From<DepositError> for Rollback`
conversion, and `reset`'s accept path. See `book/src/testing/unit-tests.md`
for the harness this builds on.

## Error codes

`DepositError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback`/`Rollback` code and message for each failure this chain's typed
entry can exit with (`reset` reuses `StateSetFailed` directly, via
`rollback!`):

| variant | code | message | meaning |
|---|---|---|---|
| `BadAmount` | 1 | `typed-results: bad AMT parameter` | `AMT` was missing, or not exactly 8 bytes |
| `StateSetFailed` | 2 | `typed-results: state_set failed` | the counter could not be persisted |

## Cost of the typed form, here

Measured (`rshooks build`/`check`, this workspace's `opt-level = 3`
profile):

| entry | form | worst-case instructions | size | max nesting depth |
|---|---|---:|---:|---:|
| `deposit` (index 0) | typed, idiomatic (`HookResult`, `?`, msg-clause `hook_errors!`) | 326 | 944 bytes | 1 |
| `reset` (index 1) | typed, raw (`HookResult`, `accept!`/`rollback!`) | 124 | 451 bytes | 1 |

Both are well within the 32-level nesting budget and the 65,535-byte
SetHook size limit — `deposit`'s higher numbers versus `reset` reflect it
doing strictly more work (a required-parameter read plus a state
read-modify-write, versus `reset`'s single unconditional write), not a cost
the typed form itself imposes; see the design doc's T-1 probe table for the
apples-to-apples comparison (`P1`/`p2fix`/`P3-typed` vs. their
`accept!`/`rollback!` twins at matched logic) that this example's numbers
corroborate rather than repeat.

[`rshooks::exit::Rollback`]: ../../crates/rshooks/src/exit.rs
