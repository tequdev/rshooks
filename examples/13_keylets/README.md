# keylets

## What you'll learn

Using `rshooks::api::keylet`'s 26 typed `keylet_xxx` helpers — one per
`KEYLET_*` constant — in place of the single untyped `util_keylet`. See
[Keylets](../../book/src/data/keylets.md) for the type-safety rationale,
the full helper table, and `account_id!`/`CurrencyCode::from_iso`. This
README covers what that page only summarizes: how each computed result
gets independently verified end-to-end, except `KEYLET_TICKET`, which
doesn't.

## The hook

Reads the invoking transaction's `sfAccount` (`owner`) and `sfDestination`
(`dest`), computes every `KEYLET_*` type but `Ticket` from `owner`/`dest`
plus a handful of fixed test constants (`src/lib.rs`), and writes every
34-byte result into this hook's own state, keyed by `KeyletKey` (a
`state_keys!` enum, variant discriminant = constant value − 1). `accept`s
once they're all written. `KeyletKey::Ticket` stays declared (so no other
variant's discriminant shifts) but is never computed or stored — see "e2e
verification scope" below.

Every keylet is computed from inputs fixed at compile time or read
directly off the invoking transaction — no other ledger object needs to
exist first, so this hook can be invoked against a bare standalone node
with no setup transactions.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/13_keylets/Cargo.toml
```

No extra flags needed: `util_keylet_buf` (which every `keylet_xxx` helper
is built on) reads into an uninitialized scratch buffer rather than a
local zero-init, so the `wasm32v1-none` `memset`-lowering threshold never
applies to it, at any `opt-level`.

## Expected behavior

- Any `Invoke` addressed to this hook's account succeeds (`accept!`) and
  writes every keylet but `Ticket` to state — there is no rejection path
  besides the field-missing/state-write-failure edge cases below (both
  unreachable in ordinary use).
- Missing `sfAccount`/`sfDestination` on the originating transaction →
  rollback.
- A `state_set` failure → rollback.
- A `keylet_xxx` compute failure (should never happen for any type this hook
  actually calls) → rollback with a code identifying exactly which type
  failed — see `compute`'s own doc comment in `src/lib.rs`. This is how the
  `Ticket` limitation below was actually found and isolated.

Failure/rollback codes are declared on `KeyletsError` in `src/lib.rs`
(`rshooks::hook_errors!`).

## e2e verification scope

### `KEYLET_TICKET`: a known host limitation, not exercised at all

Live testing against this exact node build (standalone `xahaud
2026.6.21-release+3350`) found `keylet_ticket` reliably fails at runtime —
`util_keylet` returns an error for `KEYLET_TICKET` regardless of
`ticket_seq`'s value (tried `4`, `12345`, and the invoking account's own
current `Sequence`, all rejected identically) — even though:

- The identical `account`/`ticket_seq` shape (as `{account, ticket_seq}`,
  confirmed over the node's own WebSocket RPC — the `xahau` npm package's
  own `ledger_entry` TypeScript types call the fields `owner`/
  `ticket_sequence`, which this node's RPC actually rejects as malformed)
  is accepted by that same node's `ledger_entry` RPC, which computes the
  same index through a different code path and correctly returns
  `entryNotFound` (no such ticket really exists) rather than any
  input-validation error.
- Every structurally identical type — `keylet_offer`/`keylet_escrow`/
  `keylet_check`/`keylet_signers`, each isolated the same way in a
  throwaway single-call probe hook — succeeds without incident.

This looks like a genuine gap in this specific `xahaud` build's
`util_keylet` implementation for `KEYLET_TICKET` specifically, not a bug
in `rshooks::api::keylet::keylet_ticket`'s argument marshaling. The
helper stays in `rshooks::api::keylet` regardless (it matches the
documented argument shape, and a different/future host build may support
it) — only this example's hook, and its e2e suite, skip exercising it.

### The rest: two-tier verification

`e2e/test/keylets.test.ts` independently recomputes each expected keylet
and compares it byte-for-byte against what this hook actually wrote to
state, for the types where an independent computation is available with
high confidence:

- **Directly via `xahau` npm's own exported hash helpers** (`hashes.
  hashAccountRoot`/`hashSignerListId`/`hashTrustline`/`hashOfferId`/
  `hashEscrow`/`hashPaymentChannel`/`hashCron`): `Account`, `Signers`,
  `Line`, `Offer`, `Escrow`, `Paychan`, `Cron`.
- **Via the same `sha512Half(ledgerSpace + args)` pattern those helpers
  use**, reusing the *same* ledger-space character table (`xahau`'s own
  `utils/hashes/ledgerSpaces.ts`) rather than an independently-recalled
  one: `Check` (`'C'`), `DepositPreauth` (`'p'`), `OwnerDir` (`'O'`),
  `Amendments` (`'f'`), `Fees` (`'e'`).
- **By construction**: `Unchecked` is documented as `ltANY` (`0`) plus the
  raw hash verbatim, with no hashing at all — checked byte-for-byte
  against the fixed `TEST_HASH`.

A ledger-space character is *only* the byte fed into the sha512Half hash
that produces a type's 32-byte index — for most types it also happens to
equal the resulting `Keylet`'s own 2-byte `ltXXX` type code, but not
always. Live testing found `Cron`/`OwnerDir`/`Fees`'s real type codes
(`0x0041`/`0x0064`/`0x0073`) differ from their hash-space characters
(`'L'`/`'O'`/`'e'`) — the 32-byte index still matched exactly either way,
proving the hash formula itself was right; only the assumption "type code
== hash space" was wrong for these three. `e2e/test/keylets.test.ts`'s
`typedKeyletHex` helper keeps the two independent, with the type codes
confirmed against this exact node's own output rather than assumed.

The remaining types (`Hook`, `HookState`, `HookStateDir`,
`HookDefinition` — Xahau/Hooks-specific ledger-space extensions not in any
public rippled/xahau reference; `Child`, `Skip`, `Quality`, `Page`,
`EmittedDir`, `Emitted`, `NftOffer`, `NegativeUnl` — either a composite/
derived index shape or a ledger-space character this crate has no
independently-verified source for) only get a "well-formed" check: the
call succeeded, the stored value is exactly 34 bytes, and it is not the
all-zero placeholder. This is a deliberate scoping decision, not an
oversight — see the test file's own comment for the full reasoning per
type.
