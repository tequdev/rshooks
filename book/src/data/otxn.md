# Reading the Originating Transaction

Every Hook invocation is triggered by a transaction — the *originating
transaction*, or "otxn" for short. Almost every hook needs to inspect it:
who sent it, what type it is, how much it moves, what fields it carries.
`rshooks` exposes this through the `api::otxn` module, re-exported by the
prelude. This page covers dispatching on the transaction's type, the family
of functions for reading its fields, and reading its transaction ID.
Hook and originating-transaction *parameters* (`hook_param`/`otxn_param`)
are a related but separate mechanism, covered in
[Hook and Transaction Parameters](parameters.md).

## Dispatching on transaction type

`otxn_type()` returns a `TxType`, a typed, exhaustive-by-construction enum
decoded from the raw `tt*` code the Hook API returns:

```rust
use rshooks::prelude::*;

match otxn_type() {
    TxType::Payment => { /* ... */ }
    TxType::TrustSet => { /* ... */ }
    TxType::Invoke => { /* ... */ }
    other => {
        // `TxType::Unknown(code)` covers any `tt*` code this crate does not
        // (yet) know a name for — forward-compatible with new transaction
        // types without a hard compile error.
        let _ = other;
    }
}
```

Every known transaction type (`ttPAYMENT`, `ttESCROW_CREATE`,
`ttTRUST_SET`, `ttHOOK_SET`, ...) has its own variant; `TxType::Unknown(u16)`
is the catch-all for a code this crate doesn't model yet. `TxType` also
gives back its raw code via `.code()` when you need it. Most hooks gate
their logic on their entry's declared `on`/`on_incoming`+`on_outgoing` list
already (see [Per-Hook Attributes](../build/metadata.md)), so `otxn_type`
is typically used for a final sanity check or to branch between a handful of
expected types within one hook.

## Reading a field: the typed path is the default

`rshooks` generates a table of typed field constants in the `sfield`
module — one `SField<T>` per `sfXxx` code, each carrying the Rust type that
field's value decodes as. `sfAccount` is an `SField<AccountId>`,
`sfSequence` an `SField<u32>`, `sfAmount` an `SField<Amount>`.
`otxn_field_typed` reads a field using exactly that pairing — no
turbofish, no separate decode step, and no way to accidentally decode the
wrong field as the wrong type (the constant itself pins down the return
type):

```rust
use rshooks::prelude::*;

let sender: Result<AccountId> = otxn_field_typed(sfAccount);
let sequence: Result<u32> = otxn_field_typed(sfSequence);
```

This is the default way to read any field the generated `sfield` table
gives a value type to. Narrow integers (`u8`/`u16`/`u32`) go through the
host's as-int64 mode; `u64` and every fixed-byte type (`Hash`, `AccountId`,
`CurrencyCode`) read their exact wire bytes; `Amount` and `Issue` are
classified by length and come back as `AmountBytes`/`IssueData` rather than
a single scalar, since their wire encoding is one of two shapes:

```rust
use rshooks::prelude::*;

match otxn_field_typed(sfAmount) {
    Ok(AmountBytes::Native(drops)) => {
        // `drops.0` is the raw 8-byte big-endian wire encoding — see
        // "Decoding a raw field" below for why it's `from_be_bytes`, not
        // this crate's `FromBytes` trait.
        let _ = drops;
    }
    Ok(AmountBytes::Iou(_)) => { /* an IOU amount */ }
    Err(_) => { /* field missing or unreadable */ }
}
```

`examples/03_hook-params` uses exactly this pattern to reject any
non-native `Amount`:

```rust
let drops = match otxn_field_typed(sfAmount) {
    Ok(AmountBytes::Native(n)) => u64::from_be_bytes(n.0) & !NATIVE_AMOUNT_FLAG_BITS,
    Ok(AmountBytes::Iou(_)) | Err(_) => rollback!(
        b"hook-params: unsupported (non-native) Amount",
        HookParamsError::UnsupportedAmount
    ),
};
```

(The top two bits of a serialized native amount are format flags, not part
of the drops value, hence the mask.)

Not every field has a modeled value type — `Blob`, `STObject`, `STArray`,
and a handful of others map to `Opaque`, which supports navigation but no
single scalar `value()`. For those, or when you just want the raw wire
bytes, reach for the two escape hatches below.

## The raw escapes: `otxn_field` and `otxn_field_exact`

`otxn_field` reads a field into a caller-provided buffer and returns the
number of bytes written — the least opinionated option, usable for any
field regardless of whether this crate models a typed read for it:

```rust
use rshooks::prelude::*;

let mut buf = [0u8; 20];
let written = otxn_field(&mut buf, sfAccount)?;
```

`otxn_field_exact` is the fixed-length middle ground: it requires the
field to be exactly `T`'s length (any `FixedRead` type — a
`rshooks::types` newtype, or a raw `[u8; N]`), with `T` inferred from
context rather than a turbofish:

```rust
use rshooks::prelude::*;

let sender: AccountId = otxn_field_exact(sfAccount)?;
let raw_sequence: [u8; 4] = otxn_field_exact(sfSequence)?;
```

There's also `otxn_field_u64`, the as-int64 escape hatch for a field of at
most 8 bytes with the top bit clear — the same convention
`otxn_field_typed`'s narrow-integer impls use internally.

## Decoding a raw field: `from_be_bytes`, not `FromBytes`

This is the one trap worth calling out explicitly. The bytes
`otxn_field`/`otxn_field_exact` hand back are **Xahau Binary** — the
protocol's own big-endian wire format — never this crate's little-endian
`FromBytes` trait, which is the convention for *hook-private* data (state
and parameter values this crate's own typed layer wrote, covered in
[Hook State](state.md) and [Typed Data with Derives](typed-data.md)). A
numeric protocol field read through a raw escape needs an explicit
`u64::from_be_bytes(...)` at the call site — exactly what the
`hook-params` example above does for `sfAmount`'s native drops value.
`otxn_field_typed` already does this decoding itself for every field it
models, so a field with a modeled type never needs the idiom at all.

## The transaction ID

`otxn_id` writes the originating transaction's hash into a caller buffer;
`otxn_id_buf` is the fixed-size convenience twin that returns a `Hash`
directly:

```rust
use rshooks::prelude::*;

let id: Hash = otxn_id_buf(0)?;
```

`flags = 0` prefers the emit-failure transaction ID where applicable; other
flag values pass through verbatim to the host.

## Burden and generation

For an emitted transaction (one a hook itself created via `emit`, see
[Emitting Transactions](../emit/emitting.md)), `otxn_burden` and
`otxn_generation` report the emit chain's burden and depth; for a normal,
directly-submitted transaction they read back `1` and `0` respectively.

## A worked example: the firewall pattern

`examples/05_firewall` is a compact illustration of the typed field path
end to end — read the sender as an `AccountId`, compare it against a
configured blocklist, and roll back on a match:

```rust,ignore
#[hooks]
impl Firewall {
    #[hook(0, on = [Payment])]
    fn main(&self) -> i64 {
        let Ok(sender) = otxn_field_typed(sfAccount) else {
            rollback!(
                b"firewall: could not read otxn sender",
                FirewallError::CouldNotReadSender
            )
        };

        let Some(blocked) = blocked_account() else {
            accept!()
        };

        // Avoid `==`, which can compile to an unguarded loop.
        if buf_eq_20(&sender, &blocked) {
            rollback!(b"firewall: blocked account", FirewallError::BlockedAccount);
        }

        accept!()
    }
}
```

`otxn_field_typed(sfAccount)` reads back an `AccountId` directly — no
turbofish, no manual length check. Note the comparison: `sender ==
blocked` would compile to a byte-compare loop the Hook API's guard checker
would need a `guard!` for (see [Guards and Loops](../concepts/guards.md));
`buf_eq_20` is loop-free by construction (every byte index is a
source-level literal) and sidesteps the issue entirely. `blocked_account()`
reads a Hook parameter declared on the `Firewall` struct — see [Hook and
Transaction Parameters](parameters.md).

## Nested fields: slots

Everything on this page reads a *top-level* field of the originating
transaction directly. A field nested inside an object or array (an entry in
a `Memos` array, a field of a `SignerListSet` signer entry, ...) needs
slot-based access instead — load the transaction into a slot with
`otxn_slot` and navigate from there. See [Slots and Ledger
Objects](slots.md) for the full slot API.
