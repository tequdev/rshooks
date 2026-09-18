# firewall

## What you'll learn

Reading the originating transaction's sender and comparing it against a
Hook-parameter-configured blocklist — the typed field path end to end. See
[Reading the Originating Transaction](../../book/src/data/otxn.md) ("A
worked example: the firewall pattern"), which quotes this crate's `main`
in full, and [Guards and Loops](../../book/src/concepts/guards.md) (the
compiler-generated-loop pitfall section) for why the account comparison is
spelled `buf_eq_20` rather than bare `==`.

## Configuring the blacklist

Set a `BL` Hook parameter (20 raw bytes, the blocked `AccountId`) when
installing this Hook via `SetHook`. Deployment/`SetHook` tooling is out of
scope for this repo (see `docs/DESIGN.md` §1 non-goals).

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/05_firewall/Cargo.toml
```

No extra flags needed. Straight-line code: no loop is written in the
source, and the account comparison is loop-free by construction.

Failure/rollback codes are declared on `FirewallError` in `src/lib.rs`.
