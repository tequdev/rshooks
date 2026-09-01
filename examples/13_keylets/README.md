# keylets

## What you'll learn

How to use `rshooks::api::keylet`'s 26 typed `keylet_xxx` helpers — one
per `rshooks_core::consts::KEYLET_*` constant — in place of the single
untyped `util_keylet`/`util_keylet_buf` (which takes a `keylet_type` plus
six same-typed `u32` components meaning something different per type), and
how 25 of those 26 results get independently verified end-to-end against
expected values recomputed in TypeScript (`e2e/test/keylets.test.ts`) — the
one exception, `KEYLET_TICKET`, is a real, live-tested host limitation, not
a gap in this crate; see "e2e verification scope" below.

## The hook

Reads the invoking transaction's `sfAccount` (`owner`) and `sfDestination`
(`dest`), computes 25 of the 26 `KEYLET_*` types from `owner`/`dest` plus a
handful of fixed test constants (`src/lib.rs`'s `TEST_HASH`/
`TEST_STATE_KEY`/`TEST_NAMESPACE`/`TEST_CURRENCY`/the `*_SEQ`/`*_HIGH`/
`*_LOW` constants), and writes every 34-byte result into this hook's own
state, keyed by [`KeyletKey`](src/lib.rs) — a `state_keys!` enum with one
unit variant per type, in the same numeric order as its `KEYLET_*`
constant (variant discriminant = constant value − 1; see the module doc
comment). `accept`s once all 25 are written. `KeyletKey::Ticket` stays
declared (so no other variant's discriminant shifts) but is never computed
or stored — see "e2e verification scope" below for why.

Every keylet is computed from inputs fixed at compile time (or read
directly off the invoking transaction) — no *other* ledger object needs to
exist first, so this hook can be invoked against a bare standalone node
with no setup transactions.

## Hook state

| Key (`KeyletKey` variant) | `KEYLET_*` | Arguments |
|---|---|---|
| `Hook` | `KEYLET_HOOK` (1) | `owner` |
| `HookState` | `KEYLET_HOOK_STATE` (2) | `owner`, `TEST_STATE_KEY`, `TEST_NAMESPACE` |
| `Account` | `KEYLET_ACCOUNT` (3) | `owner` |
| `Amendments` | `KEYLET_AMENDMENTS` (4) | — |
| `Child` | `KEYLET_CHILD` (5) | `TEST_HASH` |
| `Skip` | `KEYLET_SKIP` (6) | `None` (current skip list) |
| `Fees` | `KEYLET_FEES` (7) | — |
| `NegativeUnl` | `KEYLET_NEGATIVE_UNL` (8) | — |
| `Line` | `KEYLET_LINE` (9) | `owner`, `dest`, `TEST_CURRENCY` (`"USD"`) |
| `Offer` | `KEYLET_OFFER` (10) | `owner`, `OFFER_SEQ` (1) |
| `Quality` | `KEYLET_QUALITY` (11) | `TEST_DIR` (zeroed), `QUALITY_HIGH` (10), `QUALITY_LOW` (20) |
| `EmittedDir` | `KEYLET_EMITTED_DIR` (12) | — |
| `Ticket` | `KEYLET_TICKET` (13) | *not computed* — known host limitation, see below |
| `Signers` | `KEYLET_SIGNERS` (14) | `owner` |
| `Check` | `KEYLET_CHECK` (15) | `owner`, `CHECK_SEQ` (3) |
| `DepositPreauth` | `KEYLET_DEPOSIT_PREAUTH` (16) | `owner`, `dest` |
| `Unchecked` | `KEYLET_UNCHECKED` (17) | `TEST_HASH` |
| `OwnerDir` | `KEYLET_OWNER_DIR` (18) | `owner` |
| `Page` | `KEYLET_PAGE` (19) | `TEST_HASH`, `PAGE_INDEX_HIGH` (1), `PAGE_INDEX_LOW` (2) |
| `Escrow` | `KEYLET_ESCROW` (20) | `owner`, `ESCROW_SEQ` (2) |
| `Paychan` | `KEYLET_PAYCHAN` (21) | `owner`, `dest`, `PAYCHAN_SEQ` (5) |
| `Emitted` | `KEYLET_EMITTED` (22) | `TEST_HASH` |
| `NftOffer` | `KEYLET_NFT_OFFER` (23) | `owner`, `NFT_OFFER_SEQ` (6) |
| `HookDefinition` | `KEYLET_HOOK_DEFINITION` (24) | `TEST_HASH` |
| `HookStateDir` | `KEYLET_HOOK_STATE_DIR` (25) | `owner`, `TEST_NAMESPACE` |
| `Cron` | `KEYLET_CRON` (26) | `owner`, `CRON_START_TIME` (a raw ledger-time value, not a sequence — see `keylet_cron`'s own doc comment) |

## Hook parameters

None.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/13_keylets/Cargo.toml
```

No extra flags needed: `util_keylet_buf` (which every `keylet_xxx` helper
is built on) reads into an uninitialized scratch buffer rather than a
local zero-init, so the `wasm32v1-none` `memset`-lowering threshold
`docs/DESIGN.md`'s §2 C6 describes (for a hook-author-owned zero-init
buffer) never applies to it, at any `opt-level`.

Current WCE, wasm size, and max nesting depth (25 of the 26 `keylet_xxx`
calls actually exercised — see "e2e verification scope" below for why not
all 26) live in `metrics.json`.

## Expected behavior

- Any `Invoke` addressed to this hook's account succeeds (`accept!`) and
  writes 25 of the 26 keylets to state (every one but `Ticket` — see "e2e
  verification scope" below) — there is no rejection path besides the two
  field-missing/state-write-failure edge cases below (both unreachable in
  ordinary use).
- Missing `sfAccount`/`sfDestination` on the originating transaction (should
  never happen for a real `Invoke`) → rollback, codes `1`/`2`.
- A `state_set` failure (should never happen) → rollback, code `4`.
- A `keylet_xxx` compute failure (should never happen for the 25 this hook
  actually calls) → rollback, code `100 + KEYLET_*`'s own numeric value
  (`101`..`126`) — identifies exactly which type failed, see `compute`'s
  own doc comment in `src/lib.rs`. This is how the `Ticket` limitation
  below was actually found and isolated.

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
it) — only this example's hook, and this e2e suite, skip exercising it.
`KeyletKey::Ticket` stays declared in `src/lib.rs` (so no other variant's
`state_keys!` discriminant shifts) but is simply never computed or stored.

### The other 25: two-tier verification

`e2e/test/keylets.test.ts` independently recomputes each expected keylet
and compares it byte-for-byte against what this hook actually wrote to
state, for the 13 types (of the 25 actually computed) where an independent
computation is available with high confidence:

- **Directly via `xahau` npm's own exported hash helpers** (`hashes.
  hashAccountRoot`/`hashSignerListId`/`hashTrustline`/`hashOfferId`/
  `hashEscrow`/`hashPaymentChannel`/`hashCron`): `Account`, `Signers`,
  `Line`, `Offer`, `Escrow`, `Paychan`, `Cron`.
- **Via the same `sha512Half(ledgerSpace + args)` pattern those helpers
  use**, reusing the *same* ledger-space character table
  (`xahau`'s own `utils/hashes/ledgerSpaces.ts`) rather than a
  independently-recalled one: `Check` (`'C'`), `DepositPreauth` (`'p'`),
  `OwnerDir` (`'O'`), `Amendments` (`'f'`), `Fees` (`'e'`).
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

The remaining 12 types (`Hook`, `HookState`, `HookStateDir`,
`HookDefinition` — Xahau/Hooks-specific ledger-space extensions not in any
public rippled/xahau reference; `Child`, `Skip`, `Quality`, `Page`,
`EmittedDir`, `Emitted`, `NftOffer`, `NegativeUnl` — either a composite/
derived index shape or a ledger-space character this crate has no
independently-verified source for) only get a "well-formed" check: the
call succeeded, the stored value is exactly 34 bytes, and it is not the
all-zero placeholder. This is a deliberate scoping decision, not an
oversight — see the test file's own comment for the full reasoning per
type.
