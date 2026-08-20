# Hook State

A Hook's persistent storage is a flat key-value store scoped to the
account it's installed on (and, for a foreign read, another account's
namespace too). `rshooks` gives you three tiers of access to it, from a raw
buffer read all the way up to a struct field with generated typed accessor
methods. This page walks through all three, plus reading another account's
state. If you haven't read [Typed Data with Derives](typed-data.md)
yet, the `#[derive(HookKey)]`/`#[derive(HookData)]` derives it covers are
what the higher tiers here are built on.

## The state model: 32-byte keys, host-side left-padding

Every state entry is addressed by a key of up to 32 bytes. The Hook API
accepts a key from 1 to 32 bytes and **left-pads a shorter key internally**
to its own fixed-width storage slot — the same idiom a C hook uses when it
calls `state(&v, 8, "RR", 2)` with a 2-byte literal key. `rshooks` mirrors
this at every layer: a short key is sent to the host at its own real,
unpadded length, never locally zero-padded to 32 bytes. Values, by
contrast, are read and written as plain bytes with no implied structure —
interpreting them is entirely up to the layer you're using.

## Tier 1: the loose, single-value API

`rshooks::api::state` (re-exported by the prelude) is the lowest-level
typed convenience over the raw `state`/`state_set` host calls: a family of
small functions for exactly the primitive cases, each taking a raw
`&[u8]`-like key with no key-type story of its own.

```rust
use rshooks::prelude::*;

let mut buf = [0u8; 32];
let key = [0u8; 32];
let written = state(&mut buf, &key)?;

state_set(&buf[..written], &key)?;
```

For the common primitive shapes there are dedicated helpers —
`state_u32`/`state_set_u32`, `state_i64`/`state_set_i64`,
`state_xfl`/`state_set_xfl`, and their `state_update_*` read-modify-write
counterparts — all little-endian via `state_exact` under the hood. The one
outlier is `state_u64`/`state_update_u64`, which use the host's as-int64
mode and read/write **big-endian** — intended for an entry whose bytes
originated from Xahau Binary itself (a protocol-mirroring value, or interop
with a C hook), not one this crate's own typed layer wrote. For a
little-endian `u64` written by the typed layer, use `state_u64_le` instead.
`state_exact::<T>` is the general fixed-length escape hatch this tier is
built on, identical in spirit to `otxn_field_exact` (see [Reading the
Originating Transaction](otxn.md)): `T` must be exactly the right length,
inferred from context, no turbofish.

Reach for this tier for a one-off primitive read/write with no reuse
value, or as the escape hatch [Hook Chains](../concepts/chains.md#a-real-limit-typed-accessor-density-inside-one-entry)
covers for a hook whose typed-accessor call-site density has outgrown its
nesting budget. For a hook with more than a couple of distinct state
entries, the next two tiers pay off quickly.

## Tier 2: `state_keys!` — a typed key enum, independent value type

`crate::state`'s `state_get`/`state_set_loose`/`state_update_loose` work
for *any* type implementing `ToBytes`/`FromBytes` — not just the
primitives Tier 1 hard-codes — paired with a `state_keys!`-declared enum
for the key side:

```rust
use rshooks::prelude::*;
use rshooks::state_keys;

state_keys! {
    /// This hook's persistent data.
    enum DataKey {
        /// A running counter.
        Counter,
        /// A per-owner balance, keyed by the owner's account.
        Balance(AccountId),
    }
}

let count: Option<u64> = state_get(&DataKey::Counter)?;
state_set_loose(&DataKey::Counter, &1u64)?;
```

A unit variant (`Counter`) encodes to just its 1-byte discriminant, no
padding at all. A tuple variant (`Balance(AccountId)`) carries exactly one
`ToBytes` payload, encoded at runtime as "discriminant byte + payload,"
again with no trailing padding — the real length sent to the host is `1 +
Payload::MAX_LEN`. Declaration order matters: the macro assigns each
variant a sequential `u8` discriminant, so inserting or reordering a
variant changes every later variant's encoded key (and thus which on-chain
slot it addresses).

`state_get`/`state_set_loose`/`state_update_loose` still take the key and
the value type as *independent* generic parameters, though — nothing stops
calling `state_get::<SomeOtherType>(&DataKey::Counter)` for a pairing that
was never intended, as long as `SomeOtherType: FromBytes` (true of nearly
every fixed-size type this crate provides). That's exactly the gap Tier 3
closes.

## Tier 3: `#[state(...)]` struct fields — a key permanently paired with its value type

A field on a `#[hooks]` struct (see [Anatomy of a Hook](../concepts/anatomy.md))
can declare a hook-state **entity**: a key bound to exactly one value type,
via a `State<V>` field carrying a `#[state(...)]` attribute. There is no
second, independently-chosen value type left for a mismatch to hide in —
passing the wrong value where this field's `V` is expected is a compile
error.

The attribute has exactly two forms, because the key's *shape* is carried
by an ordinary Rust type (`S::KeyArgs`, resolved through the field
generated for it) rather than needing its own bespoke struct declaration:

| form | key shape | example |
|---|---|---|
| `key = <expr>` | fully fixed — a constant expression whose type implements key-encoding (a byte-string literal works directly) | `#[state(key = b"RR")]` |
| `key_by = <TypePath>` | keyed — constructed per call site from any type already implementing `StateKeyEncode` (a `#[derive(HookKey)]` struct, a `state_keys!` enum, or a primitive array type) | `#[state(key_by = DepositKey)]` |

### `key = ...`: a fixed key

```rust
use rshooks::*;

#[hooks]
pub struct StateCounter {
    /// Persistent invocation counter, stored at the fixed key `"counter"`.
    #[state(key = b"counter")]
    counter: State<u64>,
}
```

declares a field named `counter`, addressed by the fixed key `b"counter"`,
holding a `u64`. Because the struct has a named field, the macro also
generates a `static` value named after the struct (`StateCounter`, same
name, different namespace — see [Anatomy of a Hook](../concepts/anatomy.md#the-struct-has-no-runtime-instance-but-every-entry-borrows-it)).
An entry (or a helper inside the same `#[hooks] impl`) declares `&self` to
receive that static and calls the field's accessors as
`self.counter.get()`; code outside the impl reaches the identical static by
the struct's own name instead: `StateCounter.counter.get()`. `key` also
accepts a `const` reference to something more structured than a literal, as
long as it encodes:

```rust,ignore
const ENABLED_KEY: StateKey = StateKey(pad!(b"enabled"));

#[hooks]
pub struct StateForeign {
    #[state(key = &ENABLED_KEY)]
    enabled: State<[u8; 1]>,
}
```

### `key_by = ...`: a key constructed per call site

Use this when the key varies at runtime — keyed by the calling account, for
example:

```rust,ignore
#[derive(HookKey, Clone, Copy)]
struct DepositKey {
    tag: u8,
    owner: AccountId,
}

#[hooks]
pub struct TypedData {
    #[state(key_by = DepositKey)]
    deposits: State<DepositValue>,
}
```

`deposits` on its own is the *field*, not yet addressed to a specific
entry — call `.at(args)` to bind the key's runtime arguments and get a
handle with the same accessor set. Inside the `#[hooks] impl`, an entry
reaches it as `self.deposits`:

```rust,ignore
#[hook(0, on = [Invoke])]
fn main(&self) -> HookResult {
    let deposit = self.deposits.at(DepositKey { tag: DEPOSIT_TAG, owner });
    let current = deposit.get()?;
    deposit.set(&next)?;
    // ...
}
```

`DepositKey` here is any type that already implements `StateKeyEncode` —
most often a `#[derive(HookKey)]` struct (see [Typed Data with
Derives](typed-data.md)) or a `state_keys!` enum, exactly the same key
types Tier 2 uses, so a key shape you've already declared for Tier 2 slots
directly into `key_by` with no redeclaration.

### The generated accessors

Every `#[state(...)]` field — used directly for `key = ...`, or through
`.at(args)` for `key_by = ...` — gets the same six methods:

| method | signature | behavior |
|---|---|---|
| `.get()` | `Result<Option<V>>` | `Ok(None)` for "no entry"; a genuine decode failure or host error is `Err`, never confused with absence. |
| `.set(&value)` | `Result<usize>` | Writes `value`, returning the byte count written. |
| `.update(f)` | `Result<usize>` where `f: FnOnce(Option<V>) -> V` | Reads (`Option<V>`, same absence handling as `.get()`), applies `f`, writes the result — one round trip. |
| `.delete()` | `Result<()>` | See "Deleting an entry" below. |
| `.get_foreign(ns, acct)` | `Result<Option<V>>` | Same as `.get()`, but on another namespace/account — see "Foreign state" below. |
| `.set_foreign(&value, ns, acct)` | `Result<usize>` | Same as `.set()`, foreign-addressed. |

These are thin, `#[inline(always)]` forwards to the same underlying
functions Tier 1/2 call (`state_get`, `state_set_loose`, and so on) — the
struct field's job is purely to fix the key and value type together at the
declaration site, not to introduce a new code path.

## The counter walkthrough

`examples/02_state-counter` is the smallest complete tutorial for the typed
layer:

```rust,ignore
#[hooks]
pub struct StateCounter {
    #[state(key = b"counter")]
    counter: State<u64>,
}

#[hooks]
impl StateCounter {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let count = self.counter.get().unwrap_or(Some(0)).unwrap_or(0);

        let next = count.wrapping_add(1);
        if self.counter.set(&next).is_err() {
            rollback!(
                b"state-counter: state_set failed",
                StateCounterError::StateSetFailed
            );
        }

        Ok(Accept::new(b"state-counter: incremented", next as i64))
    }
}
```

`main` declares a `&self` receiver, so `self.counter.get()` returns
`Result<Option<u64>>`: `Ok(None)` means "no entry yet" (see below), so the
double `unwrap_or` handles both "never written" and "an unexpected read
error" the same way, defaulting to zero either way.

## `Ok(None)` means "no entry" — never a special-cased error

Every typed read here maps "no entry for this key" to `Ok(None)`, the same
shape as `HashMap::get` — ordinary, not exceptional. Every *other* error,
including a present-but-undersized entry that fails to decode as `T`,
still comes back as `Err`, so a genuine decode failure is never mistaken
for "nothing was ever stored here."

## Deleting an entry

The Hook API has no dedicated "delete" call — an entry is deleted by
writing zero bytes to it, which also refunds the owner reserve it was
holding. `.delete()` is the explicit spelling for that, independent of any
value type — deliberately not reachable by pairing a key with a value type
that happens to encode to nothing, which would spell "delete" as an
accident of the value type rather than an intent at the call site.
`examples/12_typed-data` deletes a depositor's record on full withdrawal
for exactly this reason (releasing the reserve, rather than leaving a
zeroed entry behind):

```rust,ignore
if deposit.delete().is_err() {
    rollback!(
        b"typed-data: state_set failed",
        TypedDataError::StateSetFailed
    );
}
```

## Foreign state: reading another account's entries

`.get_foreign(ns, acct)`/`.set_foreign(&value, ns, acct)` (and the raw-tier
`state_foreign`/`state_foreign_get`/`state_foreign_get_typed` free
functions they forward to) read or write a state entry belonging to
another account, or another namespace on this hook's own account.
`namespace`/`account` are `Option<&[u8]>`, defaulting to "this hook's own"
when passed `None`.

`examples/09_state-foreign` reads a flag from a target account configured
via a Hook parameter:

```rust,ignore
#[hooks]
pub struct StateForeign {
    /// The target account whose flag this hook reads (`ACCT`).
    #[hook_param(name = b"ACCT", required)]
    acct: HookParam<AccountId>,

    /// The target account's flag, read via `get_foreign` under
    /// [`ENABLED_KEY`] in this hook's own namespace.
    #[state(key = &ENABLED_KEY)]
    enabled: State<[u8; 1]>,
}

#[hooks]
impl StateForeign {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let Ok(target) = self.acct.get_required() else {
            rollback!(
                b"state-foreign: ACCT parameter not configured",
                StateForeignError::AcctNotConfigured
            )
        };

        let flag = match self
            .enabled
            .get_foreign(None, Some(target.as_ref()))
        {
            Ok(Some(v)) => v,
            Ok(None) => rollback!(
                b"state-foreign: not configured on target account",
                StateForeignError::NotConfiguredOnTarget
            ),
            Err(_) => rollback!(
                b"state-foreign: state_foreign read failed",
                StateForeignError::ReadFailed
            ),
        };

        if flag[0] == 0 {
            rollback!(
                b"state-foreign: target account's flag is off",
                StateForeignError::FlagOff
            );
        }

        Ok(Accept::from_code(0))
    }
}
```

Passing `namespace = None` and `account = Some(target.as_ref())` reads the
entry keyed `ENABLED_KEY` **in this hook's own namespace, but on
`target`'s account** — the shape for "the same hook code, installed on
account A and account B, where A wants to read a flag B's copy of the hook
maintains about itself." `get_required()` (covered in [Hook and
Transaction Parameters](parameters.md)) is this example's way of turning a
missing `ACCT` parameter into an immediate, distinct rollback reason,
distinct from `ACCT` being present but the target having no matching state
entry (`Ok(None)` from `get_foreign`).

## Where to go next

Every typed value type on this page — the `u64` in the counter example, the
`DepositValue` struct, `AccountId` as a `DepositKey` field — is either a
primitive `rshooks` already implements `ToBytes`/`FromBytes` for, or a
struct built with `#[derive(HookKey)]`/`#[derive(HookData)]`. See [Typed
Data with Derives](typed-data.md) for how those derives work, their exact
byte layout, and why they cost nothing over hand-packing. See [Hook
Chains](../concepts/chains.md) for how a `#[state(...)]` field declared
once is shared across every Hook entry in the same chain.
