# The `StoWriter` API

[Emitting Transactions](emitting.md) covers `txn_template!`: a declarative
macro that bakes a transaction's field offsets and total length into a
`const fn`, computed entirely at compile time. That only works when the
transaction's shape is known ahead of time. A transaction with a
runtime-sized nested `STArray`/`STObject` — Remit's `sfAmounts`, one
`sfAmountEntry` per destination, present or absent depending on what the
invoking transaction's hook parameters supply — cannot be described that
way. `rshooks::sto_writer::StoWriter` is the runtime counterpart:
a bounded, allocation-free cursor over caller-owned storage that writes
field headers, tracks open containers, and checks every write against the
buffer's real bounds. This page walks through it end to end using
`examples/17_sto-writer`'s Remit hook as the worked example throughout —
the same emission lifecycle [Emitting Transactions](emitting.md) covers
(reserve, build, emit, react), with `StoWriter` standing in for
`txn_template!` at the "build" step.

## Field order is caller-supplied, not canonical

`StoWriter` writes fields in exactly the order its methods are called and
never reorders or validates that order. xahaud accepts a serialized
object's fields in any order and always re-serializes sorted by field code,
so the on-ledger transaction is canonically ordered regardless of write
order — writing fields outside ascending `(type, field)` order changes
nothing about validity or `etxn_fee_base` (the serialized size is the same
either way; only in-buffer field position differs). What *is* enforced:
every write is checked against the buffer's real bounds with
overflow-checked cursor arithmetic; `begin_object`/`begin_array` and
`end_object`/`end_array` must match (an `STArray`'s direct children may
only be opened with `begin_object` — a bare scalar or nested array directly
inside an open array is rejected); nesting is bounded by
`STO_WRITER_MAX_DEPTH` (10); and no write succeeds once `prepare_for_emit`
has finalized the writer.

## Building a transaction

`StoWriter::new(buf)` wraps caller-owned storage as a fresh writer, empty,
at the top-level container. Scalar fields have one method per `STI_*`
shape:

| method | writes |
|---|---|
| `u16_field(f, value)` | an `STI_UINT16` field (e.g. `sfTransactionType`) |
| `u32_field(f, value)` | an `STI_UINT32` field (e.g. `sfFlags`, `sfSequence`) |
| `account_id(f, &value)` | an `STI_ACCOUNT` field (a 1-byte VL length of `20`, then the 20 raw bytes) |
| `empty_vl(f)` | an `STI_VL` field as an empty blob (a 1-byte zero-length marker) — what `SigningPubKey` looks like on an emitted transaction |
| `native_amount(f, drops)` | an `STI_AMOUNT` field encoded as a native (XRP/XAH) amount |
| `iou_amount(f, xfl, &currency, &issuer)` | an `STI_AMOUNT` field encoded as an issued amount, via the `float_sto` host call |

Containers nest with `begin_object(f)`/`end_object()` (an `STObject` field,
e.g. `sfAmountEntry`) and `begin_array(f)`/`end_array()` (an `STArray`
field, e.g. `sfAmounts`) — legal directly inside an `STObject`; an
`STArray`'s direct children may only be `begin_object`, never a bare scalar
or a nested array. `as_bytes()`/`len()`/`is_empty()` read back what has
been written so far at any point, including mid-construction with open
containers — unlike `Prepared::as_bytes`, this needs neither the container
stack closed nor any emit-plumbing field present.

`examples/17_sto-writer`'s `build_remit` builds a Remit with a native
`sfAmounts` entry always, plus a second, issued-amount entry only when the
hook's `CUR`/`ISSUER` parameters are both present:

```rust,ignore
fn build_remit<'a>(
    buf: &'a mut [u8; BUF_LEN],
    destination: &AccountId,
    issued: Option<&(CurrencyCode, AccountId)>,
) -> Result<StoWriter<'a>> {
    let mut w = StoWriter::new(buf);
    w.u16_field(sfTransactionType, rshooks::raw::tts::ttREMIT)?;
    w.u32_field(sfFlags, tfCANONICAL)?;
    w.u32_field(sfSequence, 0)?;
    w.u32_field(sfFirstLedgerSequence, 0)?;
    w.u32_field(sfLastLedgerSequence, 0)?;
    w.native_amount(sfFee, 0)?;
    w.empty_vl(sfSigningPubKey)?;
    w.account_id(sfAccount, &AccountId::default())?;
    w.account_id(sfDestination, destination)?;

    w.begin_array(sfAmounts)?;
    w.begin_object(sfAmountEntry)?;
    w.native_amount(sfAmount, 1)?;
    w.end_object()?;
    if let Some((currency, issuer)) = issued {
        w.begin_object(sfAmountEntry)?;
        w.iou_amount(sfAmount, XFL::one(), currency, issuer)?;
        w.end_object()?;
    }
    w.end_array()?;

    Ok(w)
}
```

(from `examples/17_sto-writer`.) The field order here — `TransactionType`,
`Flags`, `Sequence`, ..., `Amounts` — reads naturally top-to-bottom, but
nothing about `StoWriter` requires it; see "Field order" above.

## Required fields and duplicate rejection

`StoWriter` detects the same six required emit-plumbing fields
`txn_template!` does — `sfSequence`, `sfFirstLedgerSequence`,
`sfLastLedgerSequence`, `sfFee`, `sfSigningPubKey`, `sfAccount` — by value,
as they are written, recording an offset (or a presence flag, for
`Sequence`/`SigningPubKey`) for `prepare_for_emit` to patch or verify
later. Because `FirstLedgerSequence`/`LastLedgerSequence`/`Account`/`Fee`
are patched at each field's *recorded* offset, a second write of any of
these six fields would leave the first occurrence unpatched or duplicated
in the emitted blob — a serialized object cannot repeat a field — so a
repeat write is rejected with `HookError::AlreadySet`. Any other field may
be written more than once as far as `StoWriter` is concerned; whether a
repeated non-plumbing field is otherwise valid is between the caller and
the host.

## `prepare_for_emit()` and `Prepared::emit()`

There is no public `emit_details` method on `StoWriter` — unlike
`txn_template!`'s generated type, which declares `emit_details` as a
structural marker field in its field list, `StoWriter::prepare_for_emit`
appends the runtime-sized `sfEmitDetails` field itself, at the current
cursor, once every container the caller opened has been closed:

1. Requires every container to be closed (`depth == 0`) and all six
   required fields to have been written; otherwise
   `HookError::InvalidArgument`.
2. Patches `FirstLedgerSequence`/`LastLedgerSequence` from
   `ledger_seq() + 1` / `+ 5` (i.e. `FirstLedgerSequence + 4`), the same
   values `txn_template!`'s `prepare_for_emit` computes.
3. Patches `Account` from `hook_account()`.
4. Appends `sfEmitDetails` at the cursor via `etxn_details`, trusting its
   *returned* length (116 bytes without a `#[cbak]` export, 138 bytes
   with one) — `buf` must have at least `EMIT_DETAILS_MAX_LEN` (138) bytes
   of headroom beyond everything already written, or this step fails with
   `HookError::InvalidArgument`.
5. Computes `etxn_fee_base` over the full serialized prefix, *including*
   the just-appended `EmitDetails`, and patches `Fee`.
6. Finalizes the writer (every write after this point fails with
   `HookError::InvalidArgument`) and returns a `Prepared<'_, StoWriter<'_>>`
   handle sized to exactly what was written.

`Sequence` and `SigningPubKey` are left untouched (checked for presence
only) — exactly as in `txn_template!`'s macro-generated
`prepare_for_emit`. `StoWriter::prepare_for_emit` returns the same
`crate::txn::Prepared` type `txn_template!`'s does, so `Prepared::emit()`
— the thin wrapper over `rshooks::api::etxn::emit_buf` that passes exactly
`Prepared::as_bytes()` — works identically either way:

```rust,ignore
let Ok(mut w) = build_remit(buf, &destination, issued.as_ref()) else {
    rollback!(b"sto-writer: build failed", StoWriterError::BuildFailed)
};

let Ok(prepared) = w.prepare_for_emit() else {
    rollback!(
        b"sto-writer: prepare_for_emit failed",
        StoWriterError::PrepareFailed
    )
};

match prepared.emit() {
    Ok(_hash) => accept!(b"sto-writer: emitted", 0),
    Err(_) => rollback!(b"sto-writer: emit failed", StoWriterError::EmitFailed),
}
```

(from `examples/17_sto-writer`.) As with `txn_template!`, `etxn_reserve`
must already have been called before `prepare_for_emit`/`emit` — neither
calls it for you.

## The statics idiom, again

`StoWriter`'s backing buffer belongs in a `static` for the same reason
[Emitting Transactions](emitting.md#the-statics-idiom-for-the-template-buffer)
gives for a `txn_template!` type: a `HookStatic`-held buffer lands in a
wasm data segment/BSS instead of being materialized by runtime stores,
which matters even more here given `StoWriter`'s buffers tend to be larger
than a single fixed-shape template's. `examples/17_sto-writer` follows the
same pattern:

```rust,ignore
const BUF_LEN: usize = 285;
static BUF: HookStatic<[u8; BUF_LEN]> = HookStatic::new([0u8; BUF_LEN]);
```

```rust,ignore
let Some(buf) = BUF.take() else {
    rollback!(
        b"sto-writer: static buffer already taken",
        StoWriterError::BufferAlreadyTaken
    );
};
```

### Buffer sizing

`BUF_LEN = 285` covers the fixed emit-plumbing prefix (95 bytes:
`TransactionType`/`Flags`/`Sequence`/`FirstLedgerSequence`/
`LastLedgerSequence`/`Fee`/`SigningPubKey`/`Account`/`Destination`/
`Amounts`' own header/`AmountEntry`'s own header/a native `Amount`/its
`ObjectEndMarker`/the `ArrayEndMarker`), plus a second, issued-amount
`AmountEntry` sized for the worst case — both hook parameters present (52
bytes: header + a 48-byte `STAmount` + `ObjectEndMarker`) — plus
`EMIT_DETAILS_MAX_LEN` (138 bytes), the headroom `prepare_for_emit`
requires beyond everything already written. Sizing a `StoWriter` buffer is
manual in exactly this way — there is no macro to compute it, since the
shape is runtime-dependent by design.

## Unit tests

`examples/17_sto-writer` exercises the real `StoWriterRemit` entry through
`rshooks_testenv::TestEnv::invoke`, in both layouts [Off-Chain Unit
Tests](../testing/unit-tests.md) covers: `tests/remit.rs` and an in-crate
`#[cfg(test)]` module. Both cover the full
`prepare_for_emit()`/`Prepared::emit()` path with the `sfAmounts` array
present — native-only and native-plus-issued shapes, the `DEST`-missing
rollback, and `cbak` — and, in-crate only, `build_remit`/`prepare_for_emit`
itself against a small local `HostBackend` mock for byte-level assertions
(`build_remit` is private, so only an in-crate test can call it directly;
see `rshooks::raw::backend` on [The Raw Layer](../reference/raw.md) for
what that mock hooks into).

## Cost, here

Measured (`rshooks build`/`check`, `examples/`'s `opt-level = 3` profile):
`examples/17_sto-writer`'s `main` entry (index 0, `cbak` declared) comes to
746 worst-case instructions, 2282 bytes, and a max nesting depth of 3 —
comfortably inside the 65,535-instruction WCE ceiling and the 65,535-byte
`SetHook` size limit, and higher than `10_emit-txn`'s fixed-template
Payment (327 WCE, 1260 bytes), which is expected: this hook does strictly
more work at runtime (two hook-parameter reads, a conditional
issued-amount branch, and `StoWriter`'s own bounds/duplicate checks on
every field, versus a `const fn`-baked template with none of that at
runtime).
