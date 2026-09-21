# state-foreign

## What you'll learn

Reading **another account's** Hook state with `.get_foreign(namespace,
account)` — the same hook installed on multiple accounts can use this to
read a peer's configuration, or gate its own behavior on a flag maintained
by a separate "registry"/"oracle" account. See [Hook State](../../book/src/data/state.md#foreign-state-reading-another-accounts-entries)
for the full walkthrough of this exact hook.

## Configuring the target account and its flag

Set an `ACCT` Hook parameter (20 raw bytes, the account to read from) when
installing this Hook — see [Hook and Transaction
Parameters](../../book/src/data/parameters.md) for the `required`-field
pattern this uses. On the **target** account, this hook's namespace must
have a state entry keyed by `pad!(b"enabled")` that is **exactly 1 byte**,
nonzero (e.g. set with `state-counter`'s `state_set` pattern, or any tooling
that can write raw hook state — deployment/state-seeding tooling itself is
out of scope for this repo, see `docs/DESIGN.md` §1 non-goals).

`enabled` is declared `State<[u8; 1]>`, and a typed read requires the
stored entry to fit exactly — an entry longer than 1 byte fails the host
call with `TOO_SMALL` rather than decoding its first byte, so it surfaces
as `ReadFailed` here. (A stored entry can never be *shorter* than 1 byte
and still count as present: the Hook API deletes an entry by writing zero
bytes to it, so a 0-byte "entry" is absent, the same
`NotConfiguredOnTarget` case as no entry at all.)

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/09_state-foreign/Cargo.toml
```

No extra flags needed: every comparison here is a scalar (`usize`/`u8`)
comparison, not a fixed-size array comparison, so there's no
compiler-generated `bcmp`-style loop to guard.

## Expected behavior

- `ACCT` not configured (or not 20 bytes) → rollback.
- `ACCT` configured, but the target account has no `enabled` entry in this
  hook's namespace → rollback (`NotConfiguredOnTarget`).
- `ACCT` configured, `enabled` entry present but longer than 1 byte →
  rollback (`ReadFailed`).
- `ACCT` configured, `enabled` entry present, exactly 1 byte, value `0` →
  rollback (`FlagOff`).
- `ACCT` configured, `enabled` entry present, exactly 1 byte, nonzero →
  accept.

Failure/rollback codes are declared on `StateForeignError` in `src/lib.rs`
(`rshooks::hook_errors!`) — each variant's doc comment states which step
it corresponds to.
