# Typed Data with Derives

Hook state values, hook-state keys, and Hook API parameter names/values all
share the same underlying shape: a fixed-size, named-field Rust struct that
needs to cross the boundary into and out of a protocol byte buffer. This
page covers the conversion traits that shape rests on (`ToBytes`/
`FromBytes`/`FixedRead`), the four derive macros that generate them for
your own structs, and why those derives cost nothing over hand-packing the
bytes yourself. [Hook State](state.md) and [Hook and Transaction
Parameters](parameters.md) both build directly on what's covered here.

## The conversion traits

Two small traits fix exactly how a fixed-size value crosses the boundary:

```rust
pub trait ToBytes {
    const MAX_LEN: usize;
    fn write(&self, buf: &mut [u8]) -> usize;
}

pub trait FromBytes: Sized {
    fn read(buf: &[u8]) -> Result<Self>;
}
```

`ToBytes::write` encodes `self` into the front of `buf`, returning
`Self::MAX_LEN` on success or `0` if `buf` is too short — never a partial
write. `FromBytes::read` decodes `Self` from `buf`, failing with
`HookError::TooSmall` if `buf` is shorter than expected. Every primitive
this crate cares about implements both: `u8`/`u16`/`u32`/`u64`/`i64`,
`XFL`, `[u8; N]` for any `N`, and every `rshooks::types` newtype
(`AccountId`, `Hash`, `CurrencyCode`, ...).

A third trait, `FixedRead`, backs the `*_exact` family covered in [Reading
the Originating Transaction](otxn.md) and [Hook State](state.md) — it reads
a value in one shot from a caller-buffer host call (`otxn_field`,
`hook_param`, `state`, `slot`), by allocating exactly its own fixed-size
buffer and requiring the read to fill it exactly.

Every field's byte layout follows one crate-wide convention: **little-endian,
back-to-back, in declaration order**. This is deliberately the opposite
convention from the originating transaction's own wire fields, which are
big-endian Xahau Binary — see [Reading the Originating
Transaction](otxn.md)'s "Decoding a raw field" section for the full
two-world rule. `ToBytes`/`FromBytes` are for *hook-private* data: state and
parameter values this crate's own typed layer wrote, never protocol fields
read directly off the transaction.

## Four derives, four narrow roles

A struct used as a state key, a state value, a parameter name, or a
parameter value all share the same "fixed-offset, named-field struct"
shape — but they play genuinely different roles, so `rshooks` keeps them as
four separate, narrower derives rather than one derive covering everything:

| derive | role | generates | can be read back? |
|---|---|---|---|
| `HookData` | hook-state **value** | `ToBytes` + `FromBytes` + `FixedRead` + `LEN` | yes |
| `HookKey` | hook-state **key** | `ToBytes` + `StateKeyEncode` (≤32-byte check) | no |
| `ParamName` | parameter **name** | `ToBytes` only (1–32-byte check) | no |
| `ParamValue` | parameter **value** | `FromBytes` + `FixedRead` only | yes (that's all it's for) |

The roles are deliberately narrow:

- A **key** or a **name** is only ever encoded *outward* — handed to
  `state`/`hook_param` to *locate* something — never read back and decoded
  as itself. `HookKey`/`ParamName` reflect that by generating no
  `FromBytes`/`FixedRead`/`LEN` at all: trying to read one back as a value
  fails to compile with an ordinary trait-bound error.
- A **value** or a **payload** (`HookData`/`ParamValue`) is what actually
  gets read back and interpreted, so it gets the read-side traits.
  `ParamValue` specifically generates no `ToBytes`, since a hook never
  writes its *own* parameters (`hook_param_set` writes a *different*
  hook's parameter, taking a raw `&[u8]`, not a typed value).
- Only `HookKey`, a `state_keys!` enum, or `types::StateKey` implements
  `StateKeyEncode` — an ordinary `HookData` value struct does **not**
  automatically qualify as a key, so a state value can never be passed
  where a key is expected by accident, and vice versa. The same separation
  holds for `ParamName` vs. `ParamValue`.
- `HookKey` and `ParamName` each carry a size bound the Hook API itself
  imposes, checked **at the struct's own definition**, before it's ever
  used: `HookKey` rejects anything over 32 bytes (a hook-state key's fixed
  space); `ParamName` rejects anything outside 1–32 bytes (the Hook API's
  own parameter-name bound, which additionally has a *lower* bound a state
  key doesn't). `HookData`/`ParamValue` have no such cap — a state value or
  parameter payload isn't limited that way (beyond `rshooks::state`'s own
  32-byte typed-storage convenience limit, which the raw `api::state`
  functions bypass for a larger type).

Because a `HookData` struct also happens to satisfy `ParamValue`'s
`FromBytes`/`FixedRead` requirement, it *can* be used directly as a
parameter value — `ParamValue` is the narrower, intent-revealing choice for
a struct that's only ever a parameter payload and never a state value.

## Nesting

A derived struct can be a field of another derived struct — since every
derive only ever requires a field's type to implement the traits it needs,
and every derived struct already does, nesting needs no special support:

```rust
use rshooks::HookData;

#[derive(HookData)]
struct Inner {
    count: u32,
}

#[derive(HookData)]
struct Outer {
    tag: u8,
    inner: Inner,
}

assert_eq!(Outer::LEN, 1 + 4);
```

## The full byte image

Every field is encoded back-to-back, little-endian, in declaration order —
no padding, no per-field length prefix, no reordering. `examples/12_typed-data`
and this crate's own doctests pin this down byte-for-byte, not just as a
round-trip:

```rust
use rshooks::HookData;
use rshooks::convert::ToBytes;

#[derive(HookData, Clone, Copy)]
struct FullImage {
    a: u8,
    b: u16,
    c: u32,
    d: u64,
}

let value = FullImage {
    a: 0x11,
    b: 0x2233,
    c: 0x4455_6677,
    d: 0x8899_AABB_CCDD_EEFF,
};

let mut buf = [0u8; 15];
assert_eq!(value.write(&mut buf), 15);
assert_eq!(FullImage::LEN, 15);

let mut expected = [0u8; 15];
expected[0..1].copy_from_slice(&0x11u8.to_le_bytes());
expected[1..3].copy_from_slice(&0x2233u16.to_le_bytes());
expected[3..7].copy_from_slice(&0x4455_6677u32.to_le_bytes());
expected[7..15].copy_from_slice(&0x8899_AABB_CCDD_EEFFu64.to_le_bytes());
assert_eq!(buf, expected);
```

`u8` + `u16` + `u32` + `u64` = 1 + 2 + 4 + 8 = **15 bytes**, at offsets
`0`, `1`, `3`, `7` — exactly the field declaration order, nothing more.

## A worked example: `examples/12_typed-data`

That example declares composite key/value and name/value structs with the
derives this page covers, then wires each into a `#[hooks]` struct field —
covered in full in [Hook State](state.md) and [Hook and Transaction
Parameters](parameters.md):

```rust,ignore
// Per-account deposit record key: a tag byte + AccountId.
#[derive(HookKey, Clone, Copy)]
struct DepositKey {
    tag: u8,
    owner: AccountId,
}

// Per-account deposit record value.
#[derive(HookData, Clone, Copy)]
struct DepositValue {
    amount: u64,
    deadline: u32,
    flags: u8,
}

// Install-time configuration, read from the `CFG` Hook parameter.
#[derive(ParamValue)]
struct Config {
    min_amount: u64,
    lock_ledgers: u32,
}

#[hooks]
pub struct TypedData {
    /// Per-account deposit record, keyed by [`DepositKey`].
    #[state(key_by = DepositKey)]
    deposits: State<DepositValue>,

    /// Install-time configuration (`CFG`).
    #[hook_param(name = b"CFG", default = Config { min_amount: DEFAULT_MIN_AMOUNT, lock_ledgers: DEFAULT_LOCK_LEDGERS })]
    config: HookParam<Config>,
}
```

`DepositKey` gets `HookKey`-equivalent codegen; `DepositValue`/`Config` get
`HookData`/`ParamValue`-equivalent codegen — the field attributes
(`#[state(key_by = ...)]`, `#[hook_param(...)]`) tie each field's key/name
to its value type, the struct-field equivalent of `HookState`'s pairing
form. Used directly inside the `#[hooks] impl` via a `&self` entry, with no
manual byte packing anywhere:

```rust,ignore
let deposit = self.deposits.at(DepositKey { tag: DEPOSIT_TAG, owner });
let current = deposit.get()?.unwrap_or(EMPTY_DEPOSIT);
// ...
deposit.set(&next)?;
```

That example's own per-invocation instruction (`action`/`amount`) is
declared a different way entirely — as extra arguments on the entry fn's
own signature, per the Hook Parameter Signature Interface — see [Hook and
Transaction Parameters](parameters.md#signature-parameters-fn-arguments),
not a fourth derived struct here.

### What the derives replace

Without them, `DepositKey`/`DepositValue` would need hand-written encode/
decode functions — every field's offset counted by hand, every reader kept
in sync with every writer by hand:

```rust
// Key: tag (1 byte) || owner (20 bytes) — 21 bytes total, sent to the
// host exactly as-is (the host itself left-pads a key shorter than its
// fixed 32-byte storage width — see "Key length and padding" in Hook
// State — no local zero-padding here).
fn make_key(owner: &AccountId) -> [u8; 21] {
    let mut out = [0u8; 21];
    if let Some(b) = out.get_mut(0) {
        *b = DEPOSIT_TAG;
    }
    if let Some(dst) = out.get_mut(1..21) {
        dst.copy_from_slice(owner.as_ref());
    }
    out
}

// Value: amount (8 bytes LE) || deadline (4 bytes LE) || flags (1 byte).
fn encode_value(v: &DepositValue) -> [u8; 13] {
    let mut out = [0u8; 13];
    if let Some(dst) = out.get_mut(0..8) {
        dst.copy_from_slice(&v.amount.to_le_bytes());
    }
    if let Some(dst) = out.get_mut(8..12) {
        dst.copy_from_slice(&v.deadline.to_le_bytes());
    }
    if let Some(b) = out.get_mut(12) {
        *b = v.flags;
    }
    out
}
```

`#[derive(HookData)]` generates the equivalent of this — the same fixed,
compile-time offsets, the same `.get_mut()`-guarded fixed-size copies —
once, from the struct definition itself, and keeps `ToBytes`/`FromBytes`
in sync automatically as fields are added, removed, or reordered.

## The zero-cost claim: measured, not assumed

Every field offset in a derived struct is a compile-time constant, and
every field read/write delegates straight to that field's own
`ToBytes::write`/`FromBytes::read` — no per-field loop, and (for a total
size this toolchain's release profile still lowers to inlined stores
rather than a `memset`/`memcpy` builtin call) no unguarded loop at all.
`examples/12_typed-data`'s README backs this with a real
`rshooks build`/`check` measurement: this hook's core deposit-ledger
logic (`DepositKey`/`DepositValue`/`Config`), built twice — once with the
derives as committed, once with all three hand-packed instead, everything
else byte-for-byte identical:

| version | worst-case instructions | wasm size |
|---|---|---|
| derived (as committed) | 441 | 1504 bytes |
| hand-packed | 525 | 1674 bytes |

The derived version isn't just as cheap — it measures **cheaper**: the
generated `write`/`read` check the struct's total length once
(`buf.get_mut(..Self::MAX_LEN)`), then copy every field through
already-proven-in-bounds fixed offsets, whereas naive hand-packing
re-checks bounds with a separate `.get()`/`.get_mut()` call per field. A
hand-written version that front-loads one length check the same way could
match the derive's number — the point is the derive *always* generates
that shape, by construction, without a hook author having to discover and
apply the trick themselves. Both versions are guard-clean at the source
level; no `--auto-guard`/`--default-maxiter` needed for either.

Composite parameter *names* have one caveat: unlike a plain byte-string tag
(`CFG` above, free — the wire encoding *is* the in-memory bytes,
handed to the host with no copy), a struct-shaped name like `AdminName` in
[Hook and Transaction Parameters](parameters.md) has to actually run its
`write()` at runtime, since Rust has no stable way to run a trait method at
compile time. That's still measured cheap (+29 worst-case instructions in
that example) — see that page's "Composite names" section for the number
and why it can't go to zero.

## What each derive rejects at compile time

All four share the same field grammar: a plain, non-generic, named-field
struct with at least one field, every field a fixed-size type implementing
the traits that derive needs. An enum, a tuple struct, or a unit struct is
rejected:

```rust
use rshooks::HookData;

#[derive(HookData)]
enum NotAStruct {
    A,
    B,
}
```

A field of a variable-length type (a bare slice, a `Vec`, ...) fails with
rustc's own trait-bound error against the generated impls, naming the
missing trait — the derive doesn't implement its own type checker. And a
`HookData` value struct doesn't automatically work as a key:

```rust
use rshooks::HookData;
use rshooks::prelude::*;

#[derive(HookData)]
struct NotAKey {
    a: [u8; 20],
}

// ERROR: `NotAKey` has no `StateKeyEncode` impl — use `HookKey` for a key.
let _ = state_get::<u64>(&NotAKey { a: [0; 20] });
```

For the complete grammar, every generated item, and the full set of
`compile_fail` examples pinning each misuse, see the `HookKey`/`HookData`/
`ParamName`/`ParamValue` derives' own rustdoc — this page summarizes the
parts most relevant to everyday hook code.
