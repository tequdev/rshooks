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

(from `examples/10_emit-txn`.) Each field uses one of four uniform kinds —
`u32_field(sfXxx) = default`, `native_amount(sfXxx) = default` (always a
`u64` drops value), `account_id(sfXxx)` (defaults to all-zero), or
`empty_vl(sfXxx)` (an empty variable-length blob, no setter generated,
since there's nothing to set) — plus the structural `emit_details` marker,
which must be declared last and reserves space for the host's own
`EmitDetails` field with no header of its own.

The macro computes cumulative byte offsets and the template's total length
at compile time, bakes the field headers and defaults into a `const fn
new()` (so the whole thing lands in a wasm data segment — see the statics
idiom below), and generates one `set_<field>` method per
`u32_field`/`native_amount`/`account_id` field:

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
    fn main(&self) -> i64 { /* ... */ }

    #[cbak(0)]
    fn cbak(&self) -> i64 {
        accept!()
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
fn main(&self) -> i64 { /* ... */ }
```

`rshooks build` cross-checks a declared `can_emit` against whether the
compiled entry's wasm actually calls `emit` — a mismatch either way (a
declared type never emitted, or an emit with no matching declaration)
surfaces as a build-time warning, never a hard error. See [Per-Hook
Attributes](../build/metadata.md) for the full attribute reference,
including the three-state semantics of an omitted vs. explicitly empty
`can_emit`, and how it interacts with `on` and the other per-entry
attributes.
