# Emitting Transactions

A hook doesn't just accept or reject the transaction that invoked it — it
can also emit brand-new transactions of its own, which the network
processes independently once this hook returns. This page walks through
the emission lifecycle end to end: reserving emission slots, building a
transaction with `txn_template!`, emitting it, and reacting to the outcome
in a paired `#[cbak(<index>)]`, using `examples/10_emit-txn`'s Payment
template as the worked example throughout.

## The emission lifecycle

1. **Reserve.** Call `etxn_reserve(count)` before building or emitting
   anything — it tells the host how many transactions this invocation
   intends to emit, and every `emit` call after that must stay within the
   reserved count.
2. **Build.** Fill in a transaction's bytes — normally through a
   `txn_template!`-declared type (see below), which handles the
   protocol-level plumbing fields for you.
3. **Emit.** Hand the finished bytes to `emit`, which returns the emitted
   transaction's hash.
4. **React (optional).** If this entry declares a paired `#[cbak(<index>)]`,
   the host calls it later, when the emitted transaction actually settles
   on ledger — or bounces.

```rust,ignore
if etxn_reserve(1).is_err() {
    rollback!(b"emit-txn: etxn_reserve failed", EmitTxnError::ReserveFailed);
}
```

`etxn_reserve` must run before the corresponding `emit` call; it is not
optional bookkeeping. `rshooks::api::etxn` also exposes the lower-level
pieces this all rests on — `etxn_burden`, `etxn_fee_base`,
`etxn_generation`, `etxn_nonce`/`etxn_nonce_buf` — for hooks that need them
directly, but `txn_template!`'s `prepare_for_emit()` (below) already calls
the ones a typical Payment-shaped emission needs.

## `txn_template!`

`rshooks` deliberately does not ship a built-in `PaymentTemplate` type —
any new field or transaction shape would then require a `rshooks` release.
Instead, mirroring xahaud's own C "Tx Builder" split, `txn_template!` is a
declarative macro: you declare an ordered field list, and it generates a
byte-exact, fixed-offset template plus typed setters, computed entirely at
compile time.

```rust,ignore
txn_template! {
    /// A payment template for emitted transactions.
    struct Payment {
        transaction_type = ttPAYMENT,
        flags: u32_field(sfFlags) = tfCANONICAL,
        source_tag: u32_field(sfSourceTag) = 0,
        sequence: u32_field(sfSequence) = 0,
        destination_tag: u32_field(sfDestinationTag) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        amount: native_amount(sfAmount) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        destination: account_id(sfDestination),
        emit_details: emit_details,
    }
}
```

(from `examples/10_emit-txn`.) Each field uses one of the uniform kinds
below, plus the structural `emit_details` marker, which must be declared
last and reserves space for the host's own `EmitDetails` field with no
header of its own:

| kind | serialized type | wire bytes after header | default | setter |
|---|---|---|---|---|
| `u8_field(sfX) = e` | UINT8 | 1 | required | `set_x(u8)` |
| `u16_field(sfX) = e` | UINT16 | 2 | required | `set_x(u16)` |
| `u32_field(sfX) = e` | UINT32 | 4 | required | `set_x(u32)` |
| `u64_field(sfX) = e` | UINT64 | 8 | required | `set_x(u64)` |
| `hash128(sfX)` | UINT128 | 16 | zeroed | `set_x(&[u8; 16])` |
| `hash160(sfX)` | UINT160 | 20 | zeroed | `set_x(&[u8; 20])` |
| `hash256(sfX)` | UINT256 | 32 | zeroed | `set_x(&Hash)` |
| `currency(sfX)` | CURRENCY | 20 | zeroed | `set_x(&CurrencyCode)` |
| `native_amount(sfX) = e` | AMOUNT | 8 | required drops | `set_x(u64) -> Result<()>` |
| `amount(sfX)` | AMOUNT | 48 | IOU zero, zero currency/issuer | `set_x(xfl, &currency, &issuer)`, `set_x_value(xfl)` |
| `amount(sfX) = (xfl, cur, iss)` | AMOUNT | 48 | the declared triple | same as above |
| `native_issue(sfX)` | ISSUE | 20 | zeroed | none |
| `issue(sfX)` | ISSUE | 40 | zeroed | `set_x(&CurrencyCode, &AccountId)` |
| `account_id(sfX)` | ACCOUNT | 1 + 20 | zeroed | `set_x(&AccountId)` |
| `empty_vl(sfX)` | VL | 1 | empty blob | none |
| `fixed_vl(sfX, N) = e` | VL | VL-prefix(N) + N | zeroed, or the declared `[u8; N]` | `set_x(&[u8; N])` |
| `object(sfX) { .. }` | OBJECT | inner + 1 (`0xE1`) | inner defaults | inner setters, prefixed |
| `array(sfX) [ .. ]` | ARRAY | elements + 1 (`0xF1`) | inner defaults | inner setters, prefixed |

Every kind checks, at compile time, that the declared `sfXxx` constant's
serialized type matches — `u32_field(sfFee)` (an issued/native `AMOUNT`
field) is rejected rather than silently writing the wrong wire
representation. Integer kinds are big-endian.

The macro computes cumulative byte offsets and the template's total length
at compile time, bakes the field headers and defaults into a `const fn
new()` (so the whole thing lands in a wasm data segment — see the statics
idiom below), and generates one setter per field that has one, per the
table above:

```rust,ignore
txn.set_amount(1).is_err();       // Result<()> — native_amount setters can fail out-of-range
txn.set_destination(&dest);        // infallible — account_id setters
```

### Required fields

An emitted transaction is invalid at the protocol level without six fields
plus `EmitDetails`, so **every** `txn_template!` declaration must include
all of them, each with the matching kind:

| required field | `sfcode` | kind |
|---|---|---|
| Sequence | `sfSequence` | `u32_field` |
| FirstLedgerSequence | `sfFirstLedgerSequence` | `u32_field` |
| LastLedgerSequence | `sfLastLedgerSequence` | `u32_field` |
| Fee | `sfFee` | `native_amount` |
| SigningPubKey | `sfSigningPubKey` | `empty_vl` |
| Account | `sfAccount` | `account_id` |
| *(structural)* | — | `emit_details` |

A missing required field, or one declared with the wrong kind (`sfFee` as
`u32_field` instead of `native_amount`, say), is a compile error naming
exactly which field and check failed — never a runtime surprise. Declared
fields' `sfXxx` codes must also be in strictly increasing canonical order,
which is a compile error too (and incidentally catches an accidental
duplicate field, since two equal codes can't be strictly increasing).

### `amount`: the 48-byte issued form

`native_amount` stays the 8-byte native (XRP/XAH) form; `amount` is always
the 48-byte issued (IOU) form: `[8-byte value][20-byte currency][20-byte
issuer]`. The value bytes are a pure bit transform of the XFL —
`xfl.raw_bits() | (1 << 63)`, big-endian — so encoding an `amount` field
needs no host call, at compile time or at runtime: an `XFL`'s canonical bit
layout already occupies the same positions `STAmount`'s issued 8-byte value
uses, and setting the top bit is `STAmount`'s own "not native" flag.

An `amount(sfX)` field with no default reserves the canonical IOU zero with
an all-zero currency and issuer; the host rejects an issued amount emitted
in that state (a real issuer is required), so leaving it unset is an
authoring bug the host surfaces at emit time, not something the macro
catches. A declared default bakes the currency and issuer into the data
segment instead:

```rust,ignore
amount: amount(sfAmount) = (XFL!(0), CurrencyCode::from_iso(b"USD"), account_id!("r...")),
```

Two setters follow from that split:

- `set_x(xfl, &currency, &issuer)` rewrites all 48 bytes.
- `set_x_value(xfl)` writes only the 8 value bytes, keeping the baked or
  previously set currency/issuer — the intended hot path once a default
  triple has fixed the currency/issuer: one 8-byte store, no host call.

### `fixed_vl`: a fixed-length blob

`empty_vl(sfX)` stays the empty blob; `fixed_vl(sfX, N)` is for a `VL`
field whose length is fixed by the declaration rather than empty — a memo
type code, a fixed-width tag, and the like. `N` (a `usize` const
expression, at least 1) is part of the declaration, so rippled's VL length
prefix — one, two, or three bytes depending on `N`'s own magnitude — is
computed and baked in at compile time, the same as every other kind's
header. `N = 0` is a compile error: `empty_vl` is the one spelling for an
empty blob, so `sfSigningPubKey`'s required-kind check keeps accepting
only `empty_vl`.

```rust,ignore
memo_type: fixed_vl(sfMemoType, 4) = *b"note",
memo_data: fixed_vl(sfMemoData, 8),
```

Without a default the payload is `N` zero bytes; a declared default must
be exactly `[u8; N]` — a wrong-length default is a compile-time type
error, not a truncation. The setter, `set_x(&[u8; N])`, is an infallible
fixed-size write. Only fixed-length `VL` is covered this way; a genuinely
variable-length blob, `Vector256`, and `PathSet` stay out of scope (see
"Deferred kinds" below).

`fixed_vl` works the same way inside a nested container — a homogeneous
`sfMemos` array (see "Nested `STObject`/`STArray`" below) whose element
declares both fields:

```rust,ignore
memos: array(sfMemos) [
    Memo: object(sfMemo) {
        memo_type: fixed_vl(sfMemoType, 4) = *b"note",
        memo_data: fixed_vl(sfMemoData, 8),
    }; 1
],
```

```rust,ignore
let Some(mut memo) = txn.memos(0) else {
    rollback!(b"emit-txn: index out of range", EmitTxnError::IndexOutOfRange);
};
memo.set_memo_data(b"payload!");
```

`examples/21_txn-template-nested` carries exactly this memo alongside its
issued-amount entries.

### Nested `STObject`/`STArray`

`object(sfX) { <field>* }` and `array(sfX) [ .. ]` nest a fixed inner field
list, or a fixed element list, directly inside a template — every
element's count and shape is known at declaration time, so the whole thing
stays as compile-time-computable as the scalar kinds. An array's elements
must each be an `object(sfX) { .. }`; a bare scalar, or another `array`,
directly inside an `array` is a compile error. `array` itself comes in two
forms.

#### Named elements

Each element is declared individually, so heterogeneous shapes — one
native entry, one issued entry — fall out naturally:

```rust,ignore
txn_template! {
    struct Remit {
        transaction_type = ttREMIT,
        // .. the required fields, plus `destination: account_id(sfDestination)` ..
        amounts: array(sfAmounts) [
            native: object(sfAmountEntry) {
                amount: native_amount(sfAmount) = 1,
            },
            usd: object(sfAmountEntry) {
                amount: amount(sfAmount) = (XFL!(0), USD, USD_ISSUER),
            },
        ],
        emit_details: emit_details,
    }
}

txn.set_amounts_native_amount(5)?;           // native entry, 8-byte store
txn.set_amounts_usd_amount_value(XFL!(1.5)); // issued entry, 8-byte store
```

Setter names are the `_`-joined declaration path
(`set_amounts_native_amount`,
`set_amounts_usd_amount`/`set_amounts_usd_amount_value`); an array
element's own name (`native`, `usd`) is only a path segment, not a
repetition index.

#### Homogeneous, indexed elements

When every element has the *same* declared shape, `array(sfX) [ Elem:
object(sfY) { <field>* } ; N ]` declares that shape once and reserves `N`
back-to-back copies of it (`N` a `usize` const expression, at least 1),
instead of one setter per element:

```rust,ignore
txn_template! {
    struct Remit {
        transaction_type = ttREMIT,
        // .. the required fields, plus `destination: account_id(sfDestination)` ..
        amounts: array(sfAmounts) [
            AmountEntry: object(sfAmountEntry) {
                amount: amount(sfAmount) = (XFL!(0), USD, USD_ISSUER),
            }; 2
        ],
        emit_details: emit_details,
    }
}

let mut i: usize = 0;
loop {
    guard!(2);
    if i >= 2 {
        break;
    }
    let Some(mut e) = txn.amounts(i) else {
        rollback!(b"emit-txn: index out of range", EmitTxnError::IndexOutOfRange);
    };
    let Ok(value) = XFL::new(0, (i as i64).wrapping_add(1)) else {
        rollback!(b"emit-txn: XFL::new failed", EmitTxnError::AmountValueFailed);
    };
    e.set_amount_value(value);
    i = i.wrapping_add(1);
}
```

This generates an element-view type named `Elem` (`AmountEntry` above) —
`AmountEntry::LEN`, a baked `AmountEntry::TEMPLATE` default, and the same
inner setters (`set_amount`/`set_amount_value`) a template with that field
list would generate itself, all writing into a `&mut [u8]` view — plus, on
the parent, a runtime-indexed accessor named by the field path with **no**
`set_` prefix: `fn amounts(&mut self, index: usize) ->
Option<AmountEntry<'_>>`. `None` for `index >= N` is the whole
out-of-range story; there's no `txn.amounts[n]` indexing operator (and no
`unsafe`/`#[repr(C)]` behind the view type) — the workspace's
panic-on-out-of-range indexing lint would make a raw `[n]` unusable inside
a hook anyway, so `Option` plus a guarded loop is the idiom.

#### Choosing between them, and shared rules

Named elements read best when each entry's shape genuinely differs (a
native amount next to an issued one, say); the homogeneous indexed form
is for a repeated element shape whose count is fixed at declaration time,
built or inspected through a loop rather than named individually. A few
rules apply either way, once containers nest:

- Canonical `(type, field)` order is checked **per container**: each
  object's own direct fields must be strictly increasing, same as the
  template's top-level fields. An array's elements are not order-checked
  against each other — they typically share one repeated `sfcode` (every
  `sfAmounts` element here is an `sfAmountEntry`).
- Container headers and end markers (`0xE1` closing an `object`, `0xF1`
  closing an `array`) are written at compile time, same as every other
  baked byte.
- Nesting depth is bounded at compile time by
  [`STO_WRITER_MAX_DEPTH`](sto-writer.md), the same limit xahaud's
  deserializer enforces — a homogeneous array's element counts as two
  levels against that bound (the array itself, then the element), the
  same as a named array's object element.
- The six emit-plumbing fields (see "Required fields" above) are recognized
  only at the top level — an `sfAccount` nested inside some other object
  neither satisfies the presence check nor gets patched by
  `prepare_for_emit`.
- Every field's declared `sfXxx` still has its serialized type checked
  against its kind, a scalar or nested `array` directly inside an `array`
  is a compile error, and an `emit_details` field inside any container is a
  compile error (it's only meaningful once, at the top, last).

See `examples/21_txn-template-nested` for the worked example — a Remit
whose `sfAmounts` is a homogeneous, indexed array of compile-time-baked
issued entries, filled through the `amounts(i)` accessor (two entries,
written one after the other rather than from a loop) and emitted through
the same lifecycle as `10_emit-txn`'s Payment.

### Deferred kinds

`Vector256`, `PathSet`, and a genuinely variable-length blob (one whose
length isn't fixed by the declaration, unlike `fixed_vl`) have no
`txn_template!` kind yet; see `docs/TXN_TEMPLATE_FIELDS_DESIGN.md` §6 for
what's deferred and why. A field of one of these types still needs
`StoWriter` (below) or hand-rolled bytes.

### `prepare_for_emit()`

Because those seven fields are mandatory, every `txn_template!` invocation
that compiles gets a `prepare_for_emit(&mut self) -> Result<Prepared<'_,
Self>>` for free. It:

1. Reads the current ledger sequence and writes
   `FirstLedgerSequence = ledger_seq + 1`,
   `LastLedgerSequence = FirstLedgerSequence + 4`.
2. Writes `Account` from `hook_account()`.
3. Calls `etxn_details()` into the reserved `EmitDetails` region and uses
   its *returned* length — not the region's max capacity, since the real
   serialized size is 116 bytes without a `#[cbak]` export or 138 bytes
   with one.
4. Slices the template to exactly `emit_details offset + returned length`,
   calls `etxn_fee_base()` over that real slice, and writes `Fee`.
5. Returns a `Prepared<'_, Self>` wrapping both the template and the real
   blob length.

`prepare_for_emit` **overwrites** whatever `FirstLedgerSequence`,
`LastLedgerSequence`, `Fee`, and `Account` were previously set to — their
setters exist, but any value written through them before calling
`prepare_for_emit` is discarded. `Sequence` and `SigningPubKey` are never
touched at runtime; their baked defaults (`0`, and the empty VL marker) are
already correct.

The unprepared template type has **no `as_bytes`/`emit` method of its
own** — only `Prepared` does. That's the compile-time fix for the obvious
footgun: code that tries to read out an emit-sized blob whose plumbing
fields were never actually filled simply fails to compile.

```rust,ignore
let Ok(prepared) = txn.prepare_for_emit() else {
    rollback!(b"emit-txn: prepare_for_emit failed", EmitTxnError::PrepareFailed)
};

match prepared.emit() {
    Ok(_hash) => accept!(b"emit-txn: emitted", 0),
    Err(_) => rollback!(b"emit-txn: emit failed", EmitTxnError::EmitFailed),
}
```

`Prepared::emit()` is a convenience wrapper over `rshooks::api::etxn::emit_buf`
that passes exactly `Prepared::as_bytes()` — the real, emit-sized prefix of
the template's buffer, never the full reserved capacity.

## The statics idiom for the template buffer

A `txn_template!` type is meant to live in a `static`, not a stack local —
this is the same reasoning [Guards and Loops](../concepts/guards.md) covers
for large buffers in general: a `static`'s bytes land in a wasm data
segment (pure data, no runtime store instructions), while materializing the
same bytes into a stack local at runtime costs real, guard-relevant
instructions.

```rust,ignore
static TXN: HookStatic<Payment> = HookStatic::new(Payment::new());
```

```rust,ignore
let Some(txn) = TXN.take() else {
    rollback!(b"emit-txn: static buffer already taken", EmitTxnError::BufferAlreadyTaken);
};
```

`HookStatic::new` is `const`, so `Payment::new()`'s baked-in headers and
defaults land in the data segment directly. `take()` hands out the buffer's
one exclusive `&'static mut` on the first call and `None` on every call
after — sound with no `unsafe` because a hook runs single-threaded in a
freshly instantiated wasm module per invocation, so "handed out at most
once" really does mean at most once, ever.

## `#[cbak(<index>)]`: reacting to the outcome

A Hook entry can optionally pair its `#[hook(<index>, ...)]` with a
`#[cbak(<index>)]` at the same index, exporting `cbak` for that entry's own
build. The host invokes it later — in a separate execution — when a
transaction this hook previously emitted settles on ledger, whether it
succeeds or bounces:

```rust,ignore
#[hooks(description = "Emits a Payment and handles its callback.")]
pub struct EmitTxn;

#[hooks]
impl EmitTxn {
    #[hook(0, name = "emit-tx", on = [Invoke], can_emit = [Payment])]
    fn main(&self) -> HookResult { /* ... */ }

    #[cbak(0)]
    fn cbak(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}
```

(from `examples/10_emit-txn`; a real callback typically inspects the
settled transaction's metadata via `SlotObject::from_meta()` — see
[Slots and Ledger Objects](../data/slots.md) — before deciding how to
react.) `#[cbak(<index>)]` takes only the index — no `name`/`on`/etc. of
its own, since it settles for whatever its paired `#[hook]` at that same
index emitted. Declaring one changes that entry's `EmitDetails` real
serialized size (138 bytes instead of 116), which is exactly why
`prepare_for_emit` reads `etxn_details`'s *returned* length rather than
assuming a fixed one.

## `can_emit` on the entry attribute

Emitting a transaction of a given type is itself a capability a Hook entry
must declare. `#[hook(<index>, ...)]`'s `can_emit` list names every
transaction type this entry's wasm might emit:

```rust,ignore
#[hook(0, name = "emit-tx", on = [Invoke], can_emit = [Payment])]
fn main(&self) -> HookResult { /* ... */ }
```

`rshooks build` cross-checks a declared `can_emit` against whether the
compiled entry's wasm actually calls `emit` — a mismatch either way (a
declared type never emitted, or an emit with no matching declaration)
surfaces as a build-time warning, never a hard error. See [Per-Hook
Attributes](../build/metadata.md) for the full attribute reference,
including the three-state semantics of an omitted vs. explicitly empty
`can_emit`, and how it interacts with `on` and the other per-entry
attributes.

## Runtime-shaped transactions: `StoWriter`

`txn_template!` covers fixed-shape nested containers directly (see "Nested
`STObject`/`STArray`" above) — what it cannot describe is a shape decided
at runtime: a variable element count, or a container present only
sometimes (Remit's `sfAmounts`, one entry per destination, depending on
what the invoking transaction's hook parameters supply). That case needs
[`rshooks::sto_writer::StoWriter`](sto-writer.md) instead: a bounded,
allocation-free cursor over caller-owned storage with its own
`prepare_for_emit()`/`Prepared::emit()` lifecycle, built directly on top of
the same `Prepared` type this page's `prepare_for_emit()` returns. See
[The `StoWriter` API](sto-writer.md).
