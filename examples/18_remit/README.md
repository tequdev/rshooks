# remit

Reserves one emission slot (`etxn_reserve(1)`), builds a `Remit` transaction
to the sender of the originating transaction using `rshooks::RemitBuilder`
(one native `sfAmountEntry` of 1 drop, one issued `sfAmountEntry` of 10 USD
from a fixed demo issuer), and `emit()`s the result. Also exports `cbak`,
called when the emitted transaction settles.

`RemitBuilder` (`rshooks::remit`) is the runtime-sized counterpart to
`examples/10_emit-txn`'s `txn_template!`-generated `Payment`: `Remit`'s
`sfAmounts` is a variable-length `STArray` (one `sfAmountEntry` per
destination-bound asset), which a compile-time template cannot describe.
It is built directly on `rshooks::StoWriter` (see that module's doc comment
for the generic runtime STObject/STArray writer this wraps) and bakes in
`Remit`'s field layout — `TransactionType`, `Flags`, `Sequence`,
`FirstLedgerSequence`, `LastLedgerSequence`, `Fee`, `SigningPubKey`,
`Account`, `Destination`, then the `Amounts` array, with `sfEmitDetails`
appended by `prepare_for_emit` — so a hook only ever supplies the
destination and the amounts.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/18_remit/Cargo.toml
```

`REMIT_BUF` (the `RemitBuilder`'s backing storage) is a `HookStatic<[u8;
320]>`, for the same reason `examples/10_emit-txn`'s `Payment` template is a
`static`: a wasm data segment / BSS buffer instead of a runtime-materialized
stack array. 320 bytes covers the fixed prefix, the `Amounts` array header,
one native entry (12 bytes), one issued entry (52 bytes), the array
terminator, and `EMIT_DETAILS_MAX_LEN` bytes for `prepare_for_emit` to
append `sfEmitDetails` into, with headroom to spare.

## Unit tests

```sh
cargo test --manifest-path examples/Cargo.toml -p remit
```

Two equivalent layouts exercise the real `EmitRemit` entry through
`rshooks_testenv::TestEnv::invoke` — no wasm build, no node: `tests/remit.rs`
(an integration test against the crate as a library) and an in-crate
`#[cfg(test)]` module at the bottom of `src/lib.rs`. See
`examples/10_emit-txn/README.md` and `book/src/testing/unit-tests.md` for
the same two-layout pattern documented in full.

## WCE and wasm size

Measured with `cargo run -p rshooks-build -- check` against this example's
own build (one native + one issued amount, `cbak` exported):

| | worst-case instructions (hook) | wasm size |
|---|---|---|
| `remit` (this example, `RemitBuilder`) | 508 | 1772 bytes |

For an apples-to-apples comparison isolating `RemitBuilder` itself (same two
amounts, same destination-read/reserve/emit shape, no `cbak` on either
side), a scratch two-entry hook (not committed) compared `RemitBuilder`
against a hand-rolled equivalent — checked byte-slice writes at fields'
`codec::field_header`-derived offsets, patched the same way, `sfEmitDetails`
appended last, but without `StoWriter`'s container-tracking machinery:

| | worst-case instructions (hook) | wasm size |
|---|---|---|
| `RemitBuilder` | 508 | 1672 bytes |
| hand-rolled checked equivalent | 577 | 1966 bytes |

`RemitBuilder` is smaller on both axes (-12% WCE, -15% size) — the shared,
fully-inlined `StoWriter`/`codec` helpers `RemitBuilder` centralizes end up
duplicated less by LLVM than the hand-rolled version's inline bounds checks
at every one of its ~30 independent write sites.

## Error codes

`EmitRemitError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback!` code for each failure this hook can exit with:

| variant | code | meaning |
|---|---|---|
| `ReserveFailed` | 1 | `etxn_reserve(1)` failed to reserve an emission slot |
| `CouldNotReadSender` | 2 | `otxn_field_typed(sfAccount)` did not return an `AccountId` |
| `BufferAlreadyTaken` | 3 | the static `REMIT_BUF` had already been `take()`n |
| `BuildFailed` | 4 | `RemitBuilder::new` failed |
| `PushAmountFailed` | 5 | `push_native_amount`/`push_issued_amount` failed |
| `PrepareFailed` | 6 | `prepare_for_emit` failed |
| `EmitFailed` | 7 | the prepared Remit failed to emit |
