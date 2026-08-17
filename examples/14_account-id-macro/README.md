# account-id-macro

## What you'll learn

How `rshooks::account_id!("r...")` decodes a classic r-address into an
`AccountId` **entirely at compile time** — zero runtime cost, no
base58/checksum decode logic in the compiled hook at all — and how to prove
that compile-time result is correct against the live Hook API.

## Code walkthrough

```rust
const OWNER: AccountId = account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh");
```

`account_id!` runs entirely inside the proc-macro (host side, at `cargo
build` time): it base58-decodes the string (XRPL alphabet), verifies the
version byte and double-SHA256 checksum, and expands to a plain
`AccountId([0xB5, 0xF7, ...])` literal — see `rshooks::account_id`'s doc
comment for the full algorithm and `compile_fail` examples of what a
malformed address reports. Because the expansion is a bare literal, `OWNER`
works in `const` position, and the compiled wasm is byte-identical to
hand-writing the 20-byte array yourself (this crate's e2e test asserts
exactly that — see "Zero-cost, verified" below).

`rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh` is the Xahau/XRPL standalone-network
genesis/master account (seed `"masterpassphrase"`) — the same constant
`examples/80_governance` hand-hardcode as `GENESIS_ACCOUNT`.
`account_id!` replaces that hand-computation with the address string
directly.

The hook then checks this compile-time constant against three independent
runtime sources of truth:

```rust
// (1) hook_account(): the account this hook is installed on.
if !buf_eq_20(&installed_on, &OWNER) { rollback!(..., HookAccountMismatch) }

// (2) util_accid(): the host's own runtime r-address -> AccountID
// conversion of the same string.
if !buf_eq_20(&runtime_accid, &OWNER) { rollback!(..., UtilAccidMismatch) }

// (3) util_raddr(): converting OWNER back to text must round-trip.
if !buf_eq_34(&raddr_buf, OWNER_RADDR) { rollback!(..., UtilRaddrMismatch) }
```

Each comparison uses `buf_eq_20`/`buf_eq_34` (not `==`) for the same reason
`firewall` does — see `examples/05_firewall`'s README and
`docs/DESIGN.md`/the `hook-rust-build` skill for why array/slice `==` is
avoided in Hook code.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/14_account-id-macro/Cargo.toml
```

No extra flags needed: every comparison is a fixed-size buffer compared with
`buf_eq_20`/`buf_eq_34` (loop-free by construction, see
`crates/rshooks/src/buf_eq.rs`), and every buffer here is small enough
that no compiler-generated `memset`/`memcpy` loop appears either.

## Zero-cost, verified

`account_id!`'s whole point is that it costs the compiled hook nothing: the
e2e suite (`e2e/test/account-id-macro.test.ts`) asserts the built
`account_id_macro.wasm`'s reported worst-case instruction count against a
hand-written-array control, and the PR that introduced this example recorded
the built wasm bytes for both forms being identical — decoding happens once,
at `cargo build` time, never inside the wasm module.

## Expected behavior

Installed on the genesis/master account (`rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh`,
`@transia/hooks-toolkit`'s `testContext.master`) and invoked:

- `hook_account()` matches `OWNER`, `util_accid()` matches `OWNER`, and
  `util_raddr()` round-trips to `OWNER_RADDR` → accept, code `0`.

Installed on any other account, `hook_account()` would no longer match
`OWNER` → rollback, code `2` (not exercised by the e2e test, which always
installs on the genesis/master account specifically so all three checks are
meaningful).

## Error codes

`AccountIdMacroError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback!` code for each failure this hook can exit with:

| variant | code | meaning |
|---|---|---|
| `HookAccountFailed` | 1 | `hook_account()` itself failed |
| `HookAccountMismatch` | 2 | this hook isn't installed on `OWNER`'s account |
| `UtilAccidFailed` | 3 | `util_accid` failed to convert `OWNER_RADDR` |
| `UtilAccidMismatch` | 4 | `util_accid`'s runtime result disagrees with `account_id!`'s compile-time result |
| `UtilRaddrFailed` | 5 | `util_raddr` failed to convert `OWNER` back to text |
| `UtilRaddrLenMismatch` | 6 | `util_raddr` wrote an unexpected byte count |
| `UtilRaddrMismatch` | 7 | `util_raddr`'s output doesn't round-trip to `OWNER_RADDR` |
