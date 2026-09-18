# account-id-macro

## What you'll learn

How `rshooks::account_id!("r...")` decodes a classic r-address into an
`AccountId` **entirely at compile time** — zero runtime cost, no
base58/checksum decode logic in the compiled hook at all — and how to prove
that compile-time result correct against the live Hook API. See
`rshooks::account_id`'s doc comment for the macro's full algorithm and
`compile_fail` examples of what a malformed address reports; see
[Reading the Originating Transaction](../../book/src/data/otxn.md) for
`hook_account`/`buf_eq_20`, the runtime APIs this hook cross-checks it
against.

## Code walkthrough

```rust
const OWNER: AccountId = account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh");
```

Because the expansion is a bare literal, `OWNER` works in `const` position,
and the compiled wasm is byte-identical to hand-writing the 20-byte array
yourself (see "Zero-cost, verified" below).
`rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh` is the Xahau/XRPL standalone-network
genesis/master account (seed `"masterpassphrase"`) — the same constant
`examples/80_governance` hand-hardcodes as `GENESIS_ACCOUNT`; `account_id!`
replaces that hand-computation with the address string directly.

The hook checks this compile-time constant against independent runtime
sources of truth: `hook_account()` (the account this hook is installed
on), `util_accid()` (the host's own runtime r-address decode of the same
string), and `util_raddr()` (converting `OWNER` back to text must
round-trip) — see `src/lib.rs` for the comparisons.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/14_account-id-macro/Cargo.toml
```

No extra flags needed: every comparison is a fixed-size buffer compared with
`buf_eq_20`/`buf_eq_34` (loop-free by construction), and every buffer here
is small enough that no compiler-generated `memset`/`memcpy` loop appears
either.

## Zero-cost, verified

`account_id!`'s whole point is that it costs the compiled hook nothing: the
e2e suite (`e2e/test/account-id-macro.test.ts`) asserts the built wasm's
reported worst-case instruction count against a hand-written-array control,
and the PR that introduced this example recorded the built wasm bytes for
both forms being identical — decoding happens once, at `cargo build` time,
never inside the wasm module.

## Expected behavior

Installed on the genesis/master account
(`@xahau/hooks-toolkit`'s `testContext.master`) and invoked, every check
matches `OWNER` → accept, code `0`. Installed on any other account,
`hook_account()` would not match `OWNER` → rollback (not exercised by the
e2e test, which always installs on the genesis/master account so every
check stays meaningful).

Failure/rollback codes are declared on `AccountIdMacroError` in
`src/lib.rs` (`rshooks::hook_errors!`) — each variant's doc comment states
which check it corresponds to.
