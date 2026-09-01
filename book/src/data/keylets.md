# Keylets

A **keylet** is a 34-byte locator for a ledger object: a type prefix plus
the 32-byte hash-derived index the protocol uses to find that object in the
ledger's state map. Almost every ledger read that isn't the originating
transaction itself starts by computing a keylet, then loading the object it
points at into a slot. This page covers `rshooks`'s 26 typed `keylet_xxx`
helpers (each with a `keylet_xxx_into` out-param twin — see "Why typed
helpers" below), a worked example that computes and stores them, and
`account_id!`, the companion macro for building compile-time r-address
constants.

## Why typed helpers, not one untyped function

The host exposes a single `util_keylet` function that takes a
`keylet_type` plus up to six same-typed `u32` components (`a`..`f`). Which
of those six are used, how many, and what each one *means* — a raw value
like a sequence number, or a pointer into the hook's own linear memory for
an account ID or hash — all depend silently on `keylet_type`. Nothing at
the type level stops passing an account pointer where a sequence number was
expected, and getting it wrong is either a runtime `NO_SUCH_KEYLET`/
`INVALID_ARGUMENT`, or worse, a keylet that silently resolves to the wrong
object.

`rshooks::api::keylet` has one function per `KEYLET_*` constant instead,
each taking exactly the arguments its own type needs as the real
`rshooks::types` newtypes — `keylet_account` takes only an `&AccountId`,
`keylet_line` takes two `&AccountId`s and a `&CurrencyCode`, `keylet_offer`
takes an `&AccountId` and a `u32` sequence. Each has a `keylet_xxx_into`
out-param twin — a thin `#[inline(always)]` pass-through to the same
underlying host call, costing nothing beyond the raw call itself — that the
by-value `keylet_xxx` form delegates to (zero-init a local `Keylet`, call
the twin, return it). Reach for `_into` directly when the result is about
to be borrowed into another buffer-taking call right away, as the worked
example below does: writing straight into the caller's own storage avoids
a copy the by-value form's own scratch buffer would otherwise need.

## The 26 typed helpers

| function | `KEYLET_*` | ledger object addressed |
|---|---|---|
| `keylet_hook(account)` | `KEYLET_HOOK` (1) | `account`'s installed hook chain |
| `keylet_hook_state(account, key, namespace)` | `KEYLET_HOOK_STATE` (2) | one hook-state entry |
| `keylet_account(account)` | `KEYLET_ACCOUNT` (3) | `account`'s `AccountRoot` |
| `keylet_amendments()` | `KEYLET_AMENDMENTS` (4) | the ledger's singleton `Amendments` object |
| `keylet_child(parent)` | `KEYLET_CHILD` (5) | a derived pseudo-account keyed one level below `parent` |
| `keylet_skip(ledger_index)` | `KEYLET_SKIP` (6) | a `SkipList` object (current, or as of a historical ledger) |
| `keylet_fees()` | `KEYLET_FEES` (7) | the ledger's singleton `FeeSettings` |
| `keylet_negative_unl()` | `KEYLET_NEGATIVE_UNL` (8) | the ledger's singleton `NegativeUNL` |
| `keylet_line(a, b, currency)` | `KEYLET_LINE` (9) | the trust line (`RippleState`) between two accounts |
| `keylet_offer(account, seq)` | `KEYLET_OFFER` (10) | `account`'s `Offer` created at sequence `seq` |
| `keylet_quality(dir, high, low)` | `KEYLET_QUALITY` (11) | the order-book directory page at a given exchange rate |
| `keylet_emitted_dir()` | `KEYLET_EMITTED_DIR` (12) | the singleton directory of outstanding emitted transactions |
| `keylet_ticket(account, seq)` | `KEYLET_TICKET` (13) | `account`'s `Ticket` at `seq` — see the note below |
| `keylet_signers(account)` | `KEYLET_SIGNERS` (14) | `account`'s `SignerList` |
| `keylet_check(account, seq)` | `KEYLET_CHECK` (15) | `account`'s `Check` created at sequence `seq` |
| `keylet_deposit_preauth(owner, authorized)` | `KEYLET_DEPOSIT_PREAUTH` (16) | a recorded deposit preauthorization |
| `keylet_unchecked(hash)` | `KEYLET_UNCHECKED` (17) | `hash` reinterpreted directly as a keylet index, unvalidated |
| `keylet_owner_dir(account)` | `KEYLET_OWNER_DIR` (18) | `account`'s owner directory root |
| `keylet_page(root, high, low)` | `KEYLET_PAGE` (19) | directory page `high`/`low` under directory `root` |
| `keylet_escrow(account, seq)` | `KEYLET_ESCROW` (20) | `account`'s `Escrow` created at sequence `seq` |
| `keylet_paychan(src, dst, seq)` | `KEYLET_PAYCHAN` (21) | the `PayChannel` from `src` to `dst` created at `seq` |
| `keylet_emitted(hash)` | `KEYLET_EMITTED` (22) | the `EmittedTxn` bookkeeping entry for `hash` |
| `keylet_nft_offer(account, seq)` | `KEYLET_NFT_OFFER` (23) | `account`'s `NFTokenOffer` created at sequence `seq` |
| `keylet_hook_definition(hash)` | `KEYLET_HOOK_DEFINITION` (24) | the account-independent `HookDefinition` for wasm hash `hash` |
| `keylet_hook_state_dir(account, namespace)` | `KEYLET_HOOK_STATE_DIR` (25) | the directory of `account`'s hook-state entries under `namespace` |
| `keylet_cron(account, start_time)` | `KEYLET_CRON` (26) | `account`'s `Cron` entry firing at `start_time` |

`keylet_line_for_asset(account, &asset)` is a convenience wrapper over
`keylet_line` for when the currency/issuer pair is already an `IssuedAsset`
(the type `IouAmount::asset()` produces — see [Slots and Ledger
Objects](slots.md)) rather than two separate arguments: the trust line
between `account` and `asset.issuer` in `asset.currency`.

Every function returns `Result<Keylet>`. `keylet_hook` addresses the
*account's* installed hook chain; `keylet_hook_definition` addresses a
single hook's own account-independent definition — the two are easy to
conflate but key different objects. Likewise `keylet_owner_dir` (an
account's own directory root) is distinct from `keylet_page`, which
addresses one page of *any* directory once you already have that
directory's root index.

`keylet_ticket` has a known host limitation on the tested xahaud build: the
host's `util_keylet` rejects `KEYLET_TICKET` regardless of `ticket_seq`,
even though the identical account/sequence shape works through the
`ledger_entry` RPC and every structurally similar type (`keylet_offer`,
`keylet_escrow`, `keylet_check`, `keylet_signers`) succeeds. The helper
stays in `rshooks` — it matches the documented argument shape and a future
host build may support it — but treat it as untested until your target
node confirms otherwise.

## A worked example

`examples/13_keylets` computes 25 of the 26 keylet types (everything but
`keylet_ticket`, for the reason above) from the invoking transaction's
`sfAccount`/`sfDestination` plus a handful of fixed test inputs, and writes
every 34-byte result into hook state:

```rust,ignore
let Ok(owner) = otxn_field_typed(sfAccount) else {
    rollback!(b"keylets: sfAccount missing from the originating transaction", ...)
};
let Ok(dest) = otxn_field_typed(sfDestination) else {
    rollback!(b"keylets: sfDestination missing from the originating transaction", ...)
};

let mut keylet = Keylet::default();
check(KEYLET_ACCOUNT, keylet_account_into(&mut keylet, &owner));
store(&KeyletKey::Account, &keylet);
```

(condensed from `examples/13_keylets/src/lib.rs`, which repeats this shape
once per keylet type against one `keylet` local declared once and reused
for all 25, using a small `check`/`store` helper pair — `check` rolls back
with `100 + keylet_type` on failure, `store` writes the already-filled
`keylet` to state). It reaches for `keylet_account_into` rather than
`keylet_account` precisely because `keylet` is about to be borrowed into
`state_set` right away, and reusing one buffer across every call — instead
of zero-initializing a fresh one per type — is what collapses this hook's
WCE (see the example's own README). Every keylet here is computed entirely
from inputs already available at compile time or read directly off the
invoking transaction — no other ledger object has to exist first, so the
hook works against a bare node with no setup.

To go from a keylet to the object it addresses, load it into a slot (see
[Slots and Ledger Objects](slots.md)):

```rust,ignore
let account = SlotObject::from_keylet(&keylet_account(accid)?)?;
let seq: u32 = account.get(sfSequence)?.value()?;
```

## `account_id!` for compile-time r-addresses

Several keylet arguments are `&AccountId`, but a classic Xahau/XRPL
r-address (`rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh`) is base58-encoded text, not
the raw 20-byte form the Hook API and `keylet_xxx` want. `account_id!`
decodes that text entirely at compile time — base58 decode, version-byte
check, and double-SHA256 checksum verification all run inside the proc
macro at `cargo build` time, never inside the compiled wasm:

```rust,ignore
use rshooks::prelude::*;

const OWNER: AccountId = account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh");
```

Because the expansion is a bare `AccountId([..])` literal, `OWNER` works in
`const`/`static` position and the compiled wasm is byte-identical to
hand-writing the 20-byte array yourself — `examples/14_account-id-macro`'s
e2e suite asserts exactly that against a hand-written control. A malformed
address (bad checksum, wrong length, wrong version byte) is a
`compile_error!`, not a runtime failure:

```rust,compile_fail
// Bad checksum — last character altered.
rshooks::account_id!("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTH");
```

Reach for `account_id!` whenever a keylet argument, a hard-coded genesis
account, or any other fixed r-address needs to become an `AccountId` — it
replaces hand-computing or hex-pasting the 20 bytes yourself.

## `CurrencyCode::from_iso` for 3-character currencies

`keylet_line` takes a `&CurrencyCode`. Standard ISO-style codes (`USD`,
`EUR`, ...) are only 3 ASCII bytes, but the on-ledger encoding is always
20 bytes: twelve zeros, the three characters, five more zeros. A
160-bit non-standard currency still uses the 20-byte tuple constructor;
the 3-character form is `from_iso`, usable in `const`/`static` position:

```rust,ignore
use rshooks::prelude::*;

const USD: CurrencyCode = CurrencyCode::from_iso(b"USD");
```

The argument is `&[u8; 3]`, so `b"US"` or `b"USDT"` is a type error
rather than a silently-wrong encoding. Native XRP/XAH is a native amount,
not `from_iso(b"XRP")`.
