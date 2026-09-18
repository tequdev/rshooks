# Slots and Ledger Objects

The Hook API's **slot machine** is a set of 255 numbered registers a hook
can load deserialized ledger objects and transactions into, then navigate
into their fields and array elements. `rshooks` gives you two ways to work
with it: a raw layer that mirrors the host API one function per call, and a
typed layer (`SlotObject<T>`) that replaces slot-number bookkeeping with
Rust types. This page covers the typed layer in depth, measures it against
the raw one, and explains why the raw numbered functions are deliberately
kept out of the prelude.

## The slot machine, briefly

A slot holds one deserialized object — a transaction, a ledger entry, or a
field/array-element derived from one already loaded. Slots are numbered
`1..=255`; passing `0` as a target slot number asks the host to
auto-assign one. Every slot a hook populates is freed automatically when
the hook returns, so a short-lived hook that reads a handful of fields once
never needs to think about cleanup at all — the cost model only starts to
matter once a loop derives more slots than the 255-slot budget allows.

## The typed layer: `SlotObject<T>`

`rshooks::slot_obj::SlotObject<T>` is a handle to one loaded slot, typed by
what it holds. Slot numbers are auto-assigned by the host and never appear
in hook source:

```rust,ignore
let account = SlotObject::from_keylet(&keylet_account(accid)?)?;
let seq: u32 = account.get(sfSequence)?.value()?;
let bal: XFL = account.get(sfBalance)?.as_xfl()?;
```

Four constructors load a **root** slot:

- `SlotObject::from_otxn()` — the originating transaction
- `SlotObject::from_meta()` — the originating transaction's metadata
  (only available inside `#[cbak]`)
- `SlotObject::from_keylet(&keylet)` — the ledger object a keylet points at
  (see [Keylets](keylets.md))
- `SlotObject::from_txn_hash(&hash)` — a transaction looked up by hash

Once the loaded object's ledger-entry or transaction type is known,
`rshooks::views::ledger`/`rshooks::views::tx` give named field accessors
built on top of exactly this constructor set (`RippleState::from_keylet`,
`Payment::from_slot`, ...) instead of a `.get(sfXxx)` call per field — see
[Typed Views](views.md).

### `.get(sfXxx)` and subfield navigation

`.get(key)` derives a child slot and borrows the parent, so one loaded
object can yield several children without reloading it:

```rust,ignore
let signers = SlotObject::from_keylet(&keylet_signers(accid)?)?;
let entries = signers.get(sfSignerEntries)?;      // SlotObject<STArray>
let first = entries.get(0u32)?;                   // SlotObject<STObject>
let who: AccountId = first.get(sfAccount)?.value()?;
```

The key decides both the navigation and the resulting type: an `SField<T>`
constant (`sfAccount`, `sfBalance`, ...) navigates a field and yields
`SlotObject<T>`; a `u32` index navigates an array element and yields
`SlotObject<STObject>`. This is checked at compile time —
`SlotObject<STObject>::get(0)` (indexing an object) and
`SlotObject<STArray>::get(sfAccount)` (field-navigating an array) are both
compile errors, not runtime surprises.

### `.value()`

Once you've navigated to a leaf field, `.value()` reads it out, consuming
the handle:

```rust,ignore
let dest_slot = txn.get(sfDestination)?;
let dest: AccountId = dest_slot.value()?;
```

`value()` is generated for the scalar and fixed-size types this layer
understands — `u8`/`u16`/`u32`/`u64`, `AccountId`, `Hash`, `CurrencyCode` —
plus the two amount-shaped types below. No turbofish is needed: the
`SField<T>` (or the earlier navigation) already fixed `T`.

### `AmountBytes`, `IssueData`, and `CastTarget`

`SlotObject<Amount>` and `SlotObject<Issue>` classify their contents by
serialized length rather than assuming a shape:

```rust,ignore
pub enum AmountBytes {
    Native(NativeAmount), // 8 bytes
    Iou(IouAmount),       // 48 bytes
}

pub enum IssueData {
    Native,             // 20 bytes
    Iou(IssuedAsset),   // 40 bytes: currency and issuer
}

pub struct IssuedAsset {
    pub currency: CurrencyCode,
    pub issuer: AccountId,
}
```

`SlotObject<Amount>::value()`/`take_value()` return `AmountBytes`;
`SlotObject<Issue>::value()`/`take_value()` return `IssueData`. Both
reject an MPT-length encoding as `HookError::ParseError` rather than
guessing — MPT amounts are out of scope for this layer since Xahau has no
amendment for them yet. `SlotObject<Amount>::as_xfl()` (see
[XFL](xfl.md)) is the more common route when what you actually want is the
numeric value rather than the raw bytes, and it works identically for
native and IOU amounts.

`IouAmount` itself gives back its `(currency, issuer)` identity without a
separate decode step: `.currency()`, `.issuer()`, and `.asset()` (the pair,
as an `IssuedAsset`) all borrow the wire bytes in place rather than parsing
them. `.matches_asset(&asset)` compares an amount's currency/issuer against
an already-known `IssuedAsset` the same way — via `buf_eq_20`, never a
`memcmp` loop — without constructing an intermediate `IssuedAsset` to do it:

```rust,ignore
let AmountBytes::Iou(iou) = payment.amount()? else {
    accept!(); // native, out of scope for this check
};
let asset = iou.asset(); // IssuedAsset { currency, issuer }
if iou.matches_asset(&expected) { /* ... */ }
```

`try_cast::<U>()` retypes a handle after checking the slot's serialized
type ID against `U`'s `CastTarget` implementation — `STObject`, `STArray`,
`Amount`, `Issue`, `u8`/`u16`/`u32`/`u64`, `Hash`, `AccountId`,
`CurrencyCode` all implement it. Any failure (a mismatch or an underlying
host error) consumes the handle and best-effort clears the slot.
`assume_type::<U>()` is the free, unchecked twin, for when the caller
already knows the slot's contents from context the type system can't see.

## `slot_path!` for multi-hop navigation

A chain of `.get(a)?.get(b)?.get(c)?` leaks every intermediate slot — each
temporary handle is dropped without clearing, and nothing clears
automatically on drop. `slot_path!` clears each intermediate as soon as its
child exists, so a 10-hop path costs one live slot, not ten:

```rust,ignore
use rshooks::slot_path;

let signers = SlotObject::from_keylet(&keylet_signers(accid)?)?;
let first: AccountId = slot_path!(signers[sfSignerEntries][0u32][sfAccount])?.value()?;
```

The root is borrowed and never cleared (it's the caller's handle, evaluated
once); every intermediate is cleared unconditionally, before its result is
inspected, so a hop that fails cannot leak the parent that produced it.

## Recycling with `take_*`

`.value()`/`.as_xfl()`/`.raw()`/`.raw_exact()` all consume the handle
without clearing the slot — deliberately: this matches the C cost model
exactly (a C `slot_subfield` followed by a `slot()` read leaks the slot
identically), and an implicit clear would tax every read with an extra host
call the C idiom never pays. For a short hook reading a few fields once,
that's the right tradeoff — the host frees every slot when the hook
returns regardless.

A loop deriving one child slot per iteration is the case that actually
needs to give slots back mid-execution: 255 is the whole per-execution
budget, so a 300-iteration loop that derives a slot each time will run out.
`take_value()`/`take_xfl()`/`take_raw_exact()` read *and* clear, on both
the success path and the failure path:

```rust,ignore
let mut ok: u32 = 0;
let mut i: u32 = 0;
while i < LOOP_ITERATIONS {
    guard!(LOOP_ITERATIONS);
    i = i.wrapping_add(1);
    if let Ok(leaf) = slot_path!(root[sfSignerEntries][0u32][sfAccount]) {
        if leaf.take_value().map(|_: AccountId| true).unwrap_or(false) {
            ok = ok.wrapping_add(1);
        }
    }
}
```

`examples/15_slot-objects` proves this live: a 260-iteration loop of plain
`.get()` + `.value()` calls (over the 255-slot budget) would exhaust the
budget partway through, but the same loop through `take_value()` completes
all 260 iterations — including a separate 260-iteration loop of *failing*
`take_value()` calls, proving the clear happens on the failure path too, and
one of failing `try_cast`s, proving the same for cast failures.

## Measured: typed vs. raw

`examples/08_slot-ledger` rewrote a raw numbered-slot walk
(`otxn_slot` → `slot_subfield` → `slot_exact`) into the typed
equivalent and built both at this workspace's `opt-level = 3`:

| version | worst-case instructions | wasm size |
|---|---|---|
| raw, numbered slots, no clears | 197 | 925 bytes |
| typed, no clears | 197 | 925 bytes |
| raw, numbered slots + 3 `slot_clear` | 209 | 965 bytes |
| typed + 3 clears via `take_*` | 219 | 980 bytes |

The first two rows — the apples-to-apples comparison, same host calls, same
cleanup policy — are byte-identical: every typed wrapper is
`#[inline(always)]` over the same host call, so the type layer adds nothing.
The bottom two rows aren't directly comparable to each other:  `take_*`
clears on the failure path as well as success, while the raw code's
`slot_clear` calls only run after a successful read, so the extra ten
instructions buy strictly stronger cleanup rather than being layer
overhead.

## Why the raw numbered functions aren't in the prelude

`rshooks::api::slot` (`slot_set`, `slot_clear`, `slot_subfield`,
`slot_subarray`, `slot_type`, `slot_count`, `slot_size`, `slot`,
`meta_slot`, and friends) mirrors the host API directly — plain `u32` slot
numbers, one function per host call. It addresses the exact same 255
registers `SlotObject` does. Calling `slot_clear(3)` while a `SlotObject`
happens to hold slot 3 corrupts that handle's meaning: it keeps looking
valid but starts describing whatever the host puts there next. This is a
**logic** hazard, not a memory-safety one (no `unsafe` is involved on
either side), so nothing prevents it at the type level — the mitigation is
that these functions are kept out of the prelude and reachable only through
an explicit path (`rshooks::api::slot::slot_clear`,
`rshooks::api::otxn::otxn_slot`), so mixing the two layers is at least
always visible at the call site.

Reach for the raw layer only when a hook genuinely wants to place things in
specific numbered slots and manage them itself. Otherwise, default to
`SlotObject`: it costs nothing extra and the type system catches mistakes
the raw layer can't.
