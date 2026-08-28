# state-foreign

## What you'll learn

How to read **another account's** Hook state with `state_foreign` — the
same hook installed on multiple accounts can use this to read a peer's
configuration, or one hook can gate its own behavior on a flag maintained
by a separate "registry"/"oracle" account.

## Code walkthrough

```rust
match self.state.enabled.get_foreign(None, Some(target.as_ref())) {
    Ok(Some(v)) => v,
    Ok(None) => rollback!(b"...", StateForeignError::NotConfiguredOnTarget),
    Err(_) => rollback!(b"...", StateForeignError::ReadFailed),
}
```

`enabled` is declared as a struct field (`#[state(key = &ENABLED_KEY)]
enabled: State<[u8; 1]>`), so its `.get_foreign(namespace, account)`
accessor is generated rather than hand-written. `namespace`/`account` are
`Option<&[u8]>`, both defaulting to "this hook's own" when `None` — same
convention as the underlying `rshooks::api::state::state_foreign`. Passing
`namespace = None` and `account = Some(target.as_ref())` reads the entry
keyed by [`ENABLED_KEY`] **in this hook's own namespace, but on `target`'s
account** — the natural shape for "the same hook code, installed on
account A and account B, where A wants to read a flag B's copy of the hook
maintains about itself." Reading a genuinely different hook's namespace on
a foreign account would need an actual `namespace` value too (out of
scope for this minimal example).

`target` itself comes from a required Hook parameter (`ACCT`), declared as
`#[hook_param(name = b"ACCT", required)] acct: HookParam<AccountId>` and
read with `self.hook_param.acct.get_required()` — the same "config via
`hook_param`" idiom as `examples/03_hook-params`, just requiring the
result to be exactly `AccountId`'s length (20 bytes, enforced by
`AccountId`'s `FixedRead` impl, no turbofish) instead of manually checking
a buffer's written length. `get_required()` folds "absent" and "present
but malformed" into the same `Err`, matching this hook's single
`AcctNotConfigured` rollback code. See that example's README for the
hex-encoding/`SetHook` details, which apply here unchanged (just a
different parameter name and a 20-byte `AccountId` payload instead of an
8-byte integer).

`enabled`'s `.get_foreign()` decodes through `[u8; 1]`'s `FromBytes` impl —
a lenient *prefix* decode: it reads only the entry's first byte, silently
ignoring any bytes beyond it. An oversized `enabled` entry on the target
account is not rejected as `ReadFailed`; it is read as if it were exactly
1 byte. `ENABLED_KEY` is declared via `#[state(key = &ENABLED_KEY)]`,
which re-uses the same `pad!`-based right-padded const, so the 32-byte key
computed is fixed and deterministic.

`get_foreign`'s `Ok(None)` (no entry at all — the common, expected "not
configured" case) is deliberately distinguished from every other `Err`
(e.g. an unexpected host failure), each rolling back with its own
[`rshooks::hook_errors!`] code and message — see `examples/04_errors` for
the same "give each failure a distinct outcome" idea, and the code table
below.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/09_state-foreign/Cargo.toml
```

No extra flags needed: every comparison here is a scalar (`usize`/`u8`)
comparison, not a fixed-size array comparison, so there's no
compiler-generated `bcmp`-style loop to guard.

## Configuring the target account and its flag

Set an `ACCT` Hook parameter (20 raw bytes, the account to read from) when
installing this Hook, the same way `hook-params`' README shows for `MIN`
(just a different parameter name and payload shape — 20 raw bytes instead
of 8). On the **target** account, this hook's namespace must have a
32-byte state entry keyed by `pad!(b"enabled")` (`"enabled"` followed by
zero bytes to 32 bytes total) whose first byte is nonzero — e.g. set with
`state-counter`'s `state_set` pattern, or any tooling that can write raw
hook state. Deployment/state-seeding tooling itself is out of scope for
this repo (see `docs/DESIGN.md` §1 non-goals).

## Expected behavior

- `ACCT` not configured (or not 20 bytes) → rollback, code `1`.
- `ACCT` configured, but the target account has no `enabled` entry in this
  hook's namespace → rollback (`"not configured on target account"`, code
  `2`).
- `ACCT` configured, `enabled` entry present but its first byte is `0` →
  rollback (`"target account's flag is off"`, code `4`).
- `ACCT` configured, `enabled` entry present with a nonzero first byte →
  accept.

## Error codes

`StateForeignError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback!` code for each failure this hook can exit with:

| variant | code | meaning |
|---|---|---|
| `AcctNotConfigured` | 1 | the `ACCT` Hook parameter isn't configured (or isn't a 20-byte `AccountId`) |
| `NotConfiguredOnTarget` | 2 | the target account has no `enabled` entry in this hook's namespace |
| `ReadFailed` | 3 | `state_foreign` failed for a reason other than "no entry" |
| `FlagOff` | 4 | the target account's `enabled` entry exists but its first byte is zero |
