# txn-template-optional

## What you'll learn

`txn_template!`'s NOP-padded optional and variable-length field kinds
(`docs/NOP_PADDING_DESIGN.md`), mostly through their *inferred* (bare
`sfX`) spellings: `optional sfX`, `any_amount(sfX)` (a runtime-chosen
native-or-issued `Amount` slot, not inferable), `vl(sfX, MIN, MAX)` (a
runtime-chosen-length `VL` blob within a fixed-`MAX` slot, not
inferable), a whole-container `optional <View>: sfX { .. }` view, a
homogeneous array with `0` or `1` `optional` elements, and a named array
with one required element and one `optional` element. Every one of
these keeps its byte offset compile-time-fixed regardless of whether
it's present at runtime — absence is encoded as `0x99` (`NOP`) bytes in
field-header position, which xahaud's `STObject`/`STArray` deserializer
skips on the emit path (never on `sto_*`).

This example declares three templates, one per `#[hook]` entry, and
**every field in all three is legal for its transaction type** per
`crates/rshooks-core/protocol_formats.json` — a Payment or a Remit only
accepts the fields that format's own entry (plus `tx_common`) lists, so
this example reaches for real Remit-only fields (`sfBlob`, `sfMemos`,
`sfAmounts`, `sfMintURIToken`) rather than borrowing an arbitrary
`sfcode` the way a purely-synthetic fixture might. `OptionalPayment`
covers the Payment side alone; `OptionalRemit` and `OptionalVl` split
the Remit side across two entries so `vl`'s `MAX`-proportional WCE (see
"Cost" below) stays legible in `metrics.json` on its own, apart from
`OptionalRemit`'s other, independent kinds.

## Code walkthrough

`OptionalPayment` (`#[hook(0, name = "tplpay", ..)]`):

```rust
txn_template! {
    struct OptionalPayment {
        transaction_type = ttPAYMENT,
        sequence: sfSequence = 0,
        destination_tag: optional sfDestinationTag,
        first_ledger_sequence: sfFirstLedgerSequence = 0,
        last_ledger_sequence: sfLastLedgerSequence = 0,
        amount: native_amount(sfAmount) = 1,
        fee: native_amount(sfFee) = 0,
        send_max: optional any_amount(sfSendMax),
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: sfAccount,
        destination: sfDestination,
        emit_details: emit_details,
    }
}
```

- `destination_tag: optional sfDestinationTag` — the inferred (bare)
  spelling of `optional u32_field(sfDestinationTag)`: absent by default
  (its whole `header + 4-byte value` slot is `NOP`s); `DEST_TAG` present
  routes through the generated `set_destination_tag(u32)`. Container
  charge: `5` (the slot's own byte count).
- `amount: native_amount(sfAmount) = 1` — Payment's `sfAmount` is
  `presence: "required"`, so it stays a plain, always-present field
  (unlike `send_max` below).
- `send_max: optional any_amount(sfSendMax)` — Payment's optional
  `sfSendMax`: absent by default (`header + 48`, all `NOP`); once
  present, its *form* is chosen at runtime: `set_send_max_native(u64) ->
  Result<()>` (8-byte value, 40 trailing `NOP`s in the reserved 48-byte
  region) or `set_send_max_issued(XFL, &CurrencyCode, &AccountId)` (the
  full 48-byte issued form, no `NOP`s). `pay` picks the issued form when
  `ISSUER` supplies an issuer (currency baked `USD`, value `1.0`), the
  native form (500 drops) when only `SEND_MAX` is present, and leaves it
  absent otherwise. `any_amount` cannot infer from `sfSendMax`'s
  `STI_AMOUNT` type (shared with `native_amount`/`amount`), so it stays
  explicit. Container charge: `49` (header + the full 48-byte region,
  the whole-slot worst case for an *optional* `any_amount`).

Top-level budget: `5 + 49 = 54`, under the 63-`NOP` limit.

`OptionalRemit` (`#[hook(1, name = "tplremit", ..)]`):

```rust
txn_template! {
    struct OptionalRemit {
        transaction_type = ttREMIT,
        sequence: sfSequence = 0,
        destination_tag: optional sfDestinationTag,
        first_ledger_sequence: sfFirstLedgerSequence = 0,
        last_ledger_sequence: sfLastLedgerSequence = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        blob: optional vl(sfBlob, 2, 8),
        account: sfAccount,
        destination: sfDestination,
        memos: optional Memos: sfMemos [
            memo: sfMemo {
                memo_type: fixed_vl(sfMemoType, 4) = *b"note",
            }
        ],
        amounts: sfAmounts [
            first: sfAmountEntry {
                amount: any_amount(sfAmount),
            },
            second: optional Second: sfAmountEntry {
                amount: sfAmount = IouAmount(XFL::from_raw_bits(0), USD, USD_ISSUER),
            },
        ],
        emit_details: emit_details,
    }
}
```

- `destination_tag: optional sfDestinationTag` — the same inferred
  optional scalar `OptionalPayment` uses above.
- `blob: optional vl(sfBlob, 2, 8)` — Remit's own `sfBlob` field
  (`presence: "optional"`); absent by default; `BLOB` present writes the
  5-byte `"blob!"` payload through `set_blob(&[u8]) -> Result<()>`
  (accepting `[2, 8]`). `vl` cannot infer from `sfBlob`'s `STI_VL` type
  (shared with `empty_vl`/`fixed_vl`), so it stays explicit. Container
  charge: `11` (`header(2) + vl_length_prefix(8)(1) + 8`, the whole-slot
  worst case for an *optional* `vl`, unlike `OptionalVl::note` below).
- `memos: optional Memos: sfMemos [ .. ]` — the inferred spelling of
  `optional Memos: array(sfMemos) [ .. ]`: the whole array is
  present-or-absent (unlike `20_state-interface`-style always-present
  arrays): `enable_memos() -> Memos<'_>` restores the declared element
  (`memo_type` at its baked `*b"note"` default) and returns a view;
  `memos() -> Option<Memos<'_>>`; `clear_memos()` NOP-fills the whole
  region back. `remit` calls `enable_memos()` only when `MEMO` is
  present. Container charge: the whole region's byte count (`10`).
- `amounts: sfAmounts [ first: sfAmountEntry { .. }, second: optional
  Second: sfAmountEntry { .. } ]` — a *named* array (not a homogeneous,
  indexed one): `first` (`any_amount`, always present) is flattened as
  `set_amounts_first_amount_native(u64) -> Result<()>`/
  `set_amounts_first_amount_issued(..)`; `remit` always writes it to a
  real, constructible 1-drop native amount rather than leaving it at
  `any_amount`'s raw issued-zero encoding default — a required named
  element, unlike an `optional` one, has no "absent" state to fall back
  to, so it needs a value a real ledger would accept. `second` is the
  inferred spelling of `optional Second: object(sfAmountEntry) { .. }`
  as a named array element, baked to `IouAmount(0, USD, USD_ISSUER)`
  when enabled (`amount: sfAmount = IouAmount(..)`, the same
  default-shape desugar `examples/21_txn-template-nested` uses) —
  `enable_amounts_second() -> Second<'_>`, `amounts_second() ->
  Option<Second<'_>>` (`None` only if absent), `clear_amounts_second()`.
  A homogeneous array can't express this shape (its elements share one
  presence rule), and two fully `optional` `sfAmountEntry` elements
  (each `header(2) + amount(sfAmount)`'s 48-byte issued form +
  terminator = `2 + 49 + 1 = 52` bytes; `2 * 52 = 104 > 63`) wouldn't fit
  the array's own budget anyway — one required (`0` charge, always
  writes) plus one `optional` (`52 <= 63`) does. `first`'s `any_amount`
  and `second`'s `amount` both reserve the same 48-byte value region, so
  the two elements are the same total size (`52`) despite the different
  kind. `amount(sfAmount)` and `any_amount(sfAmount)` cannot infer
  (`STI_AMOUNT`), so both stay explicit.

Top-level budget: `5 + 11 + 10 = 26` (`amounts`'s own array charges
nothing to the top level — arrays are always-present containers), well
under 63; `second`'s `52`-byte charge lands against `amounts`'s own
budget instead.

`OptionalVl` (`#[hook(2, name = "tplvl", ..)]`, a second Remit):

```rust
txn_template! {
    struct OptionalVl {
        transaction_type = ttREMIT,
        sequence: sfSequence = 0,
        first_ledger_sequence: sfFirstLedgerSequence = 0,
        last_ledger_sequence: sfLastLedgerSequence = 0,
        invoice_id: optional sfInvoiceID,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        note: vl(sfBlob, 190, 194),
        account: sfAccount,
        destination: sfDestination,
        mint: optional Mint: sfMintURIToken {
            flags: optional sfFlags,
            uri: fixed_vl(sfURI, 4) = *b"ipfs",
        },
        amounts: sfAmounts [
            Entry: optional sfAmountEntry {
                amount: sfAmount = IouAmount(XFL::from_raw_bits(0), USD, USD_ISSUER),
            }; 1
        ],
        emit_details: emit_details,
    }
}
```

- `invoice_id: optional sfInvoiceID` — the inferred spelling of `optional
  hash256(sfInvoiceID)`; `INVOICE` present routes through
  `set_invoice_id(&Hash)`. Container charge: `34`.
- `note: vl(sfBlob, 190, 194)` — not optional, but variable-length: the
  slot is sized for `MAX = 194` (a two-byte `VL` length prefix, since
  `194 > 192`), and `set_note(&[u8]) -> Result<()>` accepts any length in
  `[190, 194]`. `vl` writes 190 bytes (a *one*-byte prefix, five trailing
  `NOP`s to fill the 194-sized slot) unless `NOTE_LONG` is present, in
  which case it writes the full 194 (filling the slot exactly) — one hook
  parameter exercises both `vl_length_prefix` widths from the same
  declaration. `OptionalVl` reuses the same `sfcode` (`sfBlob`) as
  `OptionalRemit::blob` above — they're different templates, so nothing
  collides. Container charge: `slot(194) - slot(190) = 196 - 191 = 5`
  (small, despite `MAX`'s own size — the charge is the worst-case *NOP
  count*, not `MAX` itself).
- `mint: optional Mint: sfMintURIToken { flags: optional sfFlags, uri:
  fixed_vl(sfURI, 4) = *b"ipfs" }` — the inferred spelling of `optional
  Mint: object(sfMintURIToken) { .. }`, a whole-container-optional
  `object` view over Remit's own `sfMintURIToken`, whose real inner
  format requires `sfURI` and allows `sfFlags`/`sfDigest` optionally:
  `uri` is baked (`fixed_vl` with a default — required fields are never
  `optional`), `flags` is independently `optional` inside the view.
  `enable_mint() -> Mint<'_>` restores the baked default (`uri =
  "ipfs"`, `flags` absent) and returns a view whose own `set_flags(u32)`
  writes the inner optional field; `mint() -> Option<Mint<'_>>`;
  `clear_mint()` NOP-fills the region back. `vl` calls `enable_mint()`
  then `set_flags(1)` only when `MINT` is present. Container charge: `14`
  (`header(2) + flags(5, all-NOP absent) + uri(6) + terminator(1)`).
- `amounts: sfAmounts [ Entry: optional sfAmountEntry { .. } ; 1 ]` — a
  *homogeneous* array (contrast `OptionalRemit::amounts`'s named one
  above) of `0` or `1` `optional` elements: `amounts(0) ->
  Option<Entry<'_>>` (the index is always in range; `None` is
  unreachable here, kept only because the accessor's signature is
  shared with the general, runtime-`N` case), and the returned `Entry`
  starts absent — `entry.enable()` restores the same baked
  `IouAmount(0, USD, USD_ISSUER)` default `OptionalRemit::amounts.second`
  uses, `entry.clear()` NOP-fills it back. `vl` calls `entry.enable()`
  only when `AMT_ENTRY` is present, then `set_amount_value` to a positive
  value, since Remit rejects a zero amount. Container charge (the array's own
  budget, independent of the top level): `1 * Entry::LEN = 52`.

Top-level budget: `34 + 5 + 14 = 53`, under 63 (`amounts`'s own `52`-byte
charge lands against its own budget instead, same as `OptionalRemit`'s).

All three entries share the `DEST` (required) hook parameter for
`sfDestination`; every other parameter is optional and gates exactly one
of the fields above — see `src/lib.rs`'s `TxnTemplateOptional` struct doc
comments for the full list.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/22_txn-template-optional/Cargo.toml
```

Three artifacts: `0.pay.wasm` (`#[hook(0, ..)]`), `1.remit.wasm`
(`#[hook(1, ..)]`), `2.vl.wasm` (`#[hook(2, ..)]`) — no `cbak` on any of
them beyond a bare accept, since no entry's own logic depends on the
emitted transaction's outcome.

## Unit tests

```sh
cargo test --manifest-path examples/Cargo.toml -p txn-template-optional
```

Two layers, split by what each needs to observe:

- `src/lib.rs`'s in-crate `#[cfg(test)]` module constructs
  `OptionalPayment`/`OptionalRemit`/`OptionalVl` directly (private types,
  so only reachable in-crate) and asserts their raw, pre-emit `bytes()` —
  the NOP-padding mechanism itself: an absent/`MIN`-length field's `NOP`
  bytes at the exact declared offset, a present/`MAX`-length one filling
  that same slot exactly, and — for `note` — that shrinking back from
  `MAX` to `MIN` re-`NOP`-fills the vacated tail rather than leaving
  stale bytes from the longer write. No host backend is needed, since
  none of these calls reach `prepare_for_emit`/`emit`.
- `tests/pay.rs`, `tests/remit.rs`, and `tests/vl.rs` drive the real
  `TxnTemplateOptional` chain through `rshooks_testenv::TestEnv::invoke`
  — no wasm build, no node — and assert the *decoded* functional
  behavior against `env.emitted()`'s canonical (re-serialized, NOP-free)
  blob: present with exactly the bytes written, absent by byte-count
  delta against the present-state blob (not by searching for the
  header's *absence* — a short header can coincidentally match part of a
  longer, unrelated one elsewhere in the blob, e.g. `EmitDetails`). Each
  `amounts` field is covered by its element count instead (counted by
  the array's own `0xE1` terminators between its header and the closing
  `0xF1`): `OptionalRemit::amounts` is one element with `AMT_ENTRY`
  absent, two once it enables `second`; `OptionalVl::amounts` is zero
  with `AMT_ENTRY` absent, one once it enables the sole element. Also
  covers `send_max`'s native/issued forms and `note`'s `MIN`/`MAX`
  (prefix-width-crossing) lengths.

Every expected region in both layers is built from
`rshooks::txn::codec::field_header`/`NOP` rather than hardcoded header
bytes, matching `examples/21_txn-template-nested`'s test style.

## Error codes

`TxnTemplateOptionalError` (`rshooks::hook_errors!`, see `src/lib.rs`) is
the `rollback!` code for each failure any of the three entries can exit
with:

| variant | code | meaning |
|---|---|---|
| `ReserveFailed` | 1 | `etxn_reserve(1)` failed to reserve an emission slot |
| `MissingDestination` | 2 | the `DEST` hook parameter was missing or not a 20-byte `AccountId` |
| `BufferAlreadyTaken` | 3 | the static template buffer had already been `take()`n |
| `AmountFailed` | 4 | an `any_amount`/`native_amount`/`amount` value was out of range |
| `NoteFailed` | 5 | `OptionalVl::note`'s length fell outside `[190, 194]` — unreachable by construction, kept only because the setter returns `Result` |
| `BlobFailed` | 6 | `OptionalRemit::blob`'s length fell outside `[2, 8]` — unreachable by construction, kept only because the setter returns `Result` |
| `AmountsIndexOutOfRange` | 7 | `OptionalVl::amounts`'s index was out of range — unreachable by construction (the index is the literal `0`, always below the declared element count `1`), kept only because the accessor returns `Option` |
| `PrepareFailed` | 8 | `prepare_for_emit` failed to fill in the host-supplied fields |
| `EmitFailed` | 9 | the prepared transaction could not be emitted |

## Cost

Current WCE, wasm size, and max nesting depth for all three entries live
in [`metrics.json`](./metrics.json), refreshed by `mise run
record-example-metrics`. `note`'s `vl(sfBlob, 190, 194)` setter's
`guard_m!`-protected copy loop, bounded by `MAX`, is the dominant, and
essentially only, contributor to `tplvl`'s WCE (`docs/NOP_PADDING_DESIGN.md`
§3.1: "The static WCE contribution is proportional to `MAX`") — isolating
it in its own entry, away from `tplpay`'s and `tplremit`'s other,
independent kinds, is what keeps that cost legible per entry rather than
folded into a total that also pays for `any_amount`/`optional`
scalars/whole-container views.
