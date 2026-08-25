# sto-writer

## What you'll learn

`rshooks::sto_writer::StoWriter`: a bounded, allocation-free writer for a
runtime-sized `STObject`/`STArray`, for transactions `txn_template!` can't
describe because their shape isn't known at compile time. This hook builds
a Remit — `sfAmounts` is one such runtime-sized field: a native-amount
entry always, plus a second, issued-amount entry only when the invoking
transaction supplies hook parameters for it — and emits it via
`StoWriter::prepare_for_emit()`/`Prepared::emit()`, the dynamic
counterpart to `txn_template!`'s own `prepare_for_emit()` lifecycle.

## Code walkthrough

```rust
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

`main` reserves one emission slot, reads the required `DEST` hook
parameter (a 20-byte `AccountId`) and the optional `CUR`/`ISSUER` pair
(a `CurrencyCode` and an `AccountId`; both must be present to add the
issued entry), calls `build_remit`, then `prepare_for_emit()` followed by
`Prepared::emit()`. `prepare_for_emit()` patches
`FirstLedgerSequence`/`LastLedgerSequence`/`Account`/`Fee` and appends the
runtime-sized `sfEmitDetails` field itself — there is no `emit_details()`
call in `build_remit` at all; see `rshooks::sto_writer`'s module doc
comment for why that field is `prepare_for_emit`'s job, not the caller's.

The field order above (`TransactionType`, `Flags`, `Sequence`, ...,
`Amounts`) reads naturally top-to-bottom, but `StoWriter` does not require
it: fields land in exactly the order the methods are called, and the host
accepts an emitted blob's fields in any order (canonicalizing on
re-serialization) — see `rshooks::sto_writer`'s module doc comment for the
full explanation. `begin_array`/`begin_object`/`end_object`/`end_array`
are the only structural rule that *is* enforced: an `STArray`'s direct
children may only be opened with `begin_object`.

### Buffer sizing

`BUF_LEN = 285` covers the fixed emit-plumbing prefix (95 bytes:
`TransactionType`/`Flags`/`Sequence`/`FirstLedgerSequence`/
`LastLedgerSequence`/`Fee`/`SigningPubKey`/`Account`/`Destination`/
`Amounts`' own header/`AmountEntry`'s own header/a native `Amount`/its
`ObjectEndMarker`/the `ArrayEndMarker`), plus a second, issued-amount
`AmountEntry` (52 bytes: header + a 48-byte `STAmount` + `ObjectEndMarker`)
sized for the worst case (both parameters present), plus
`EMIT_DETAILS_MAX_LEN` (138 bytes) — `StoWriter::prepare_for_emit`
requires that much headroom beyond everything already written, to append
`EmitDetails` into.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/17_sto-writer/Cargo.toml
```

No extra flags needed. The backing buffer is a `HookStatic` (a wasm data
segment / BSS, not a stack local), the same static-buffer idiom
`10_emit-txn`'s README describes — recommended for any hook with a buffer
this large, since it avoids a compiler-generated zero-init loop.

## Unit tests

```sh
cargo test --manifest-path examples/Cargo.toml -p sto-writer
```

Two equivalent layouts exercise the real `StoWriterRemit` entry through
`rshooks_testenv::TestEnv::invoke` — no wasm build, no node: `tests/remit.rs`
(an integration test against the crate as a library) and an in-crate
`#[cfg(test)]` module at the bottom of `src/lib.rs`. See
`book/src/testing/unit-tests.md` for the full walkthrough of both layouts.
Both cover the full `prepare_for_emit()`/`Prepared::emit()` path with the
`sfAmounts` array present — native-only and native-plus-issued shapes, the
`DEST`-missing rollback, and `cbak` — plus, in-crate only,
`build_remit`/`prepare_for_emit`'s own byte-level correctness against a
small local `HostBackend` mock (`build_remit` is private, so only an
in-crate test can call it directly).

## Error codes

`StoWriterError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback!` code for each failure this hook can exit with:

| variant | code | meaning |
|---|---|---|
| `ReserveFailed` | 1 | `etxn_reserve(1)` failed to reserve an emission slot |
| `MissingDestination` | 2 | the `DEST` hook parameter was missing or not a 20-byte `AccountId` |
| `BufferAlreadyTaken` | 3 | the static build buffer had already been `take()`n |
| `BuildFailed` | 4 | a `StoWriter` write failed (out of space, bad nesting, or a duplicate write of a required field) |
| `PrepareFailed` | 5 | `prepare_for_emit` failed to fill in the host-supplied fields |
| `EmitFailed` | 6 | the prepared transaction could not be emitted |

## Cost of `StoWriter`, here

Measured (`rshooks build`/`check`, this workspace's `opt-level = 3`
profile):

| | worst-case instructions | size | max nesting depth |
|---|---:|---:|---:|
| `main` (index 0, `cbak` declared) | 776 | 2347 bytes | 3 |

Well within the 32-level nesting budget, the 65,535-instruction WCE
ceiling, and the 65,535-byte `SetHook` size limit. Higher than
`10_emit-txn`'s fixed-template Payment (358 WCE, 1326 bytes) — expected,
since this hook does strictly more work at runtime (two hook-parameter
reads, a conditional issued-amount branch, and `StoWriter`'s own
bounds/duplicate checks on every field, versus a `const fn`-baked template
with none of that at runtime). See the `feat/sto-writer` PR body for a
closer apples-to-apples comparison against a hand-written checked
serializer building the identical shape.
