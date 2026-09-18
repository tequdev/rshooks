# guard-patterns

A teaching example, not a realistic policy on its own: same shape as
`firewall` (block an account via a `BL` Hook parameter), written to
exercise `guard!`/`guard_m!` correctness and the `[u8; N] == [u8; N]`
compiler-generated-loop pitfall. See [Guards and
Loops](../../book/src/concepts/guards.md), which quotes this crate's
`accounts_equal` and `guard_m!` demonstration loops directly, for the full
mechanism: how to choose an exact `maxiter`, what `guard_m!`'s `$n`
disambiguator actually protects against (verified here by deliberately
colliding two ids), and the nested-loop-unrolling pitfall
`examples/80_governance` hit.

## Specific to this example

`accounts_equal`'s hand-written loop (`maxiter = 20`, exact, from
`AccountId`'s fixed length) and the two `guard_m!(8, 1)`/`guard_m!(8, 2)`
demonstration loops exist purely to make the guard book chapter's claims
checkable against this repo's own toolchain — not idiomatic hook style.
The accept code on the "not blocked" path is the sum of the two
demonstration loops' outputs, not meaningful hook logic on its own (see
the in-source comment at the return site).

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/06_guard-patterns/Cargo.toml
```

No extra flags needed: every loop in this crate is hand-written and
guarded in the source; there is no compiler-generated loop anywhere in it.

## Expected behavior

- `BL` not configured → accept (nothing to block).
- `BL` configured but not a 20-byte `AccountId` → rollback.
- `BL` configured and matches the otxn sender → rollback.
- `BL` configured and doesn't match → accept, with the accept code set to
  the sum of the two demonstration loops' outputs.

Failure/rollback codes are declared on `GuardPatternsError` in
`src/lib.rs`. Current WCE/size/nesting live in
[`metrics.json`](./metrics.json).
