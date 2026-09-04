# `txn_template!` field-kind completion — design

Status: implemented. Extends `txn_template!` (`crates/rshooks/src/txn.rs`,
`docs/DESIGN.md` §5.5) from its original uniform scalar kinds to every fixed-width serialized
type Xahau's `sfields.macro` uses, plus fixed-shape `STObject`/`STArray` nesting. Variable-length
kinds (`Blob`, `Vector256`, `PathSet`, non-empty VL in general) are out of scope for this
iteration; §6 records what is deliberately deferred.

## 1. Goals and non-goals

Goals:

- Every field a hook can plausibly emit whose wire size is fixed by the declaration alone
  gets a `txn_template!` kind, so a fixed-shape emitted transaction never needs
  `StoWriter` or hand-rolled bytes.
- `Amount`-typed fields get two kinds with two fixed sizes: `native_amount(sf)` stays the
  8-byte native form; `amount(sf)` is **always** the 48-byte issued (IOU) form.
- `STObject` fields nest when their inner field list is declared inline; `STArray` fields
  nest when every element is declared inline (the element count is therefore known at
  declaration time).
- Each kind checks, at compile time, that the `sfXxx` constant's serialized type ID
  (`code >> 16`) matches the kind (today only the six emit-plumbing fields are
  kind-checked).
- Everything stays `const fn`-computable: offsets, total length, baked headers, end
  markers, and defaults land in the data segment exactly as today; setters remain
  `#[inline(always)]` stores at compile-time-proven offsets.

Non-goals:

- Runtime-sized shapes (conditional fields, runtime element counts) — `StoWriter`'s job.
- Any change to `prepare_for_emit`/`Prepared`, to `StoWriter`, or to existing templates'
  bytes (`examples/10_emit-txn` must stay byte-identical; `metrics.json` unchanged).

## 2. Grammar

```text
txn_template! {
    $(#[meta])* $vis struct Name {
        transaction_type = ttXXX,
        <field>*
        <name>: emit_details,          // last, top level only (unchanged)
    }
}

<field> :=
    <name>: u8_field(sfX)  = <u8 expr>
  | <name>: u16_field(sfX) = <u16 expr>
  | <name>: u32_field(sfX) = <u32 expr>          // existing
  | <name>: u64_field(sfX) = <u64 expr>
  | <name>: hash128(sfX)                          // 16 bytes, zeroed
  | <name>: hash160(sfX)                          // 20 bytes, zeroed (STI_UINT160)
  | <name>: hash256(sfX)                          // 32 bytes, zeroed
  | <name>: currency(sfX)                         // 20 bytes, zeroed (STI_CURRENCY)
  | <name>: native_amount(sfX) = <u64 drops>      // existing, 8 bytes
  | <name>: amount(sfX) $(= (<XFL>, <CurrencyCode>, <AccountId>))?   // 48 bytes
  | <name>: native_issue(sfX)                     // 20 bytes (currency only, zeroed)
  | <name>: issue(sfX)                            // 40 bytes (currency + issuer, zeroed)
  | <name>: account_id(sfX)                       // existing
  | <name>: empty_vl(sfX)                         // existing
  | <name>: object(sfX) { <field>* }              // STObject, closed by 0xE1
  | <name>: array(sfX) [ <element>* ]             // STArray, closed by 0xF1

<element> := <name>: object(sfX) { <field>* }     // only objects directly inside an array
```

Trailing commas are accepted everywhere a field list is accepted (as today).
`emit_details` inside an `object`/`array` is a compile error (§4.5).

### 2.1 Kind table

| kind | STI (type id) | wire bytes after header | default | setter |
|---|---|---|---|---|
| `u8_field` | UINT8 (16) | 1 | required `= expr` | `set_x(u8)` |
| `u16_field` | UINT16 (1) | 2 | required | `set_x(u16)` |
| `u32_field` | UINT32 (2) | 4 | required | `set_x(u32)` (unchanged) |
| `u64_field` | UINT64 (3) | 8 | required | `set_x(u64)` |
| `hash128` | UINT128 (4) | 16 | zeroed | `set_x(&[u8; 16])` |
| `hash160` | UINT160 (17) | 20 | zeroed | `set_x(&[u8; 20])` |
| `hash256` | UINT256 (5) | 32 | zeroed | `set_x(&Hash)` |
| `currency` | CURRENCY (26) | 20 | zeroed | `set_x(&CurrencyCode)` |
| `native_amount` | AMOUNT (6) | 8 | required `= drops` | `set_x(u64) -> Result<()>` (unchanged) |
| `amount` | AMOUNT (6) | 48 | IOU zero + zero currency/issuer, or the declared triple | `set_x(XFL, &CurrencyCode, &AccountId)`, `set_x_value(XFL)` |
| `native_issue` | ISSUE (24) | 20 | zeroed | none (native issue is all-zero by definition) |
| `issue` | ISSUE (24) | 40 | zeroed | `set_x(&CurrencyCode, &AccountId)` |
| `account_id` | ACCOUNT (8) | 1 + 20 | zeroed | `set_x(&AccountId)` (unchanged) |
| `empty_vl` | VL (7) | 1 | `0x00` | none (unchanged) |
| `object` | OBJECT (14) | inner + 1 (`0xE1`) | inner defaults | inner setters, prefixed |
| `array` | ARRAY (15) | elements + 1 (`0xF1`) | inner defaults | inner setters, prefixed |

Integer kinds are written big-endian (Xahau binary format policy, `docs/DESIGN.md` §8).

The `hash128`/`hash160`/`hash256`/`currency` setters are infallible. `set_x_value` on
`amount` is infallible (§3.2); `set_x` on `amount` is infallible too.

### 2.2 `amount` (48-byte issued form)

Region layout after the header: `[8-byte value][20-byte currency][20-byte issuer]`
(`types::IouAmount`'s layout). Value encoding is a pure bit transform of the XFL, so no
host call is needed either at compile time or at runtime:

```text
value = (xfl.raw_bits() as u64 | 0x8000_0000_0000_0000).to_be_bytes()
```

XFL bit layout (`xfl.rs`): bit 63 clear, bit 62 sign (set = positive), bits 54..=61
exponent + 97, bits 0..=53 mantissa, canonical zero = 0. `STAmount`'s issued 8-byte value
uses the identical field positions with bit 63 set ("not native"), and canonical zero =
`0x8000_0000_0000_0000` — so the OR covers zero and nonzero alike. This matches xahaud's
`float_sto` (and `rshooks-testenv`'s reimplementation of it, `host/float.rs`) byte for
byte. The existing `codec::MAX_NATIVE_DROPS`-style range failure has no analogue here: a
canonical XFL's exponent/mantissa ranges are exactly `STAmount`'s, so the setters cannot
fail. An `XFL` built through `XFL::from_raw_bits` with non-canonical bits produces a value
the host rejects at emit time; the setters do not re-validate (documented, same trust as
every other `XFL` consumer in the crate).

Two setters per `amount` field:

- `set_x(xfl, &currency, &issuer)` — writes all 48 bytes.
- `set_x_value(xfl)` — writes only the 8 value bytes, keeping the baked or previously set
  currency/issuer. With a declared default triple this is the intended hot path: one
  8-byte store, no host call.

Declared default: `amount(sfAmount) = (XFL!(0), CurrencyCode::from_iso(b"USD"),
account_id!("r..."))` — all three constructors are already `const`. Without a default the
region is the canonical IOU zero with an all-zero currency/issuer, which xahaud rejects if
emitted unset (an issued amount needs a real issuer), so a template that never calls
`set_x` on such a field is an authoring bug the host surfaces, not the macro. (§7 lists an
optional compile-time mitigation.)

### 2.3 `issue` / `native_issue`

`STIssue` serializes as the 20-byte currency alone when the currency is XRP/XAH, else
currency + issuer (40 bytes). Mirroring the amount split: `native_issue` is a fixed 20 zero
bytes (no setter), `issue` a fixed 40 bytes with `set_x(&CurrencyCode, &AccountId)`. The
only `ISSUE` fields today are AMM/XChain (dormant on Xahau mainnet, feature-gated in
`sfield.rs`); the kinds are cheap and complete the fixed-width set, so they are included
rather than left as a gap.

### 2.4 Nested containers

```rust,ignore
txn_template! {
    struct Remit {
        transaction_type = ttREMIT,
        flags: u32_field(sfFlags) = tfCANONICAL,
        sequence: u32_field(sfSequence) = 0,
        first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
        last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
        fee: native_amount(sfFee) = 0,
        signing_pub_key: empty_vl(sfSigningPubKey),
        account: account_id(sfAccount),
        destination: account_id(sfDestination),
        amounts: array(sfAmounts) [
            native: object(sfAmountEntry) { amount: native_amount(sfAmount) = 1 },
            usd: object(sfAmountEntry) {
                amount: amount(sfAmount) = (XFL!(0), USD, USD_ISSUER),
            },
        ],
        emit_details: emit_details,
    }
}

txn.set_amounts_native_amount(5)?;          // native entry, 8-byte store (Result: 62-bit range)
txn.set_amounts_usd_amount_value(XFL!(1.5)); // issued entry, 8-byte store
```

- Setter names are the `_`-joined declaration path: `set_<outer>_<inner>_<leaf>`. Array
  elements are named like any other field; the element name is only a path segment.
- Array elements are declared one by one. That is what "element count known ahead" means
  here: the shape of every element is fixed, and heterogeneous element shapes (one native
  entry, one issued entry) fall out naturally. There is no `; N` repetition sugar —
  `macro_rules!` cannot count, and a proc-macro rewrite is out of scope.
- Wire bytes: `object` writes `header(sfX)`, the inner fields, then `0xE1`
  (`ObjectEndMarker`); `array` writes `header(sfX)`, each element (itself an object with
  its own header and `0xE1`), then `0xF1` (`ArrayEndMarker`). Container headers are
  ordinary field headers (`codec::field_header`), the same bytes `StoWriter::begin_object`
  /`begin_array` write.
- Canonical order is checked **per container**: each object's direct fields must have
  strictly increasing `sfXxx` codes (as today for the top level). An array's elements are
  not order-checked (they share one code; xahaud keeps array element order as written).
- Nesting depth is bounded at compile time by `STO_WRITER_MAX_DEPTH` (10), the limit
  xahaud's `STVar` deserializer enforces and the one `StoWriter` already uses.
- The six emit-plumbing fields are recognized only at the top level. A nested `sfAccount`
  (inside a `Signer` or `HookGrant` object, say) neither satisfies the
  presence check nor gets patched by `prepare_for_emit`.

## 3. Implementation

### 3.1 `codec` additions (`txn.rs`)

All `pub`, `const fn`, panic-free at runtime (compile-time panics only in `const`
contexts, as today):

- Size helpers: `u8_field_size`, `u16_field_size`, `u64_field_size`,
  `fixed_field_size(f, n)` (header + `n`; backs `hash128/160/256`, `currency`,
  `issue`, `native_issue`), `iou_amount_field_size` (header + 48),
  `container_header_size(f)` (header only), plus `OBJECT_END_MARKER: u8 = 0xE1`,
  `ARRAY_END_MARKER: u8 = 0xF1`.
- `encode_iou_amount_value_const(xfl: XFL) -> [u8; 8]` and a runtime
  `encode_iou_amount_value(out: &mut [u8], xfl: XFL) -> Result<()>` (the `Result` is only
  the `out.len() < 8` bounds check, for standalone callers; the generated setters index
  with proven offsets and use the const form directly).
- `encode_iou_amount_const(xfl, &CurrencyCode, &AccountId) -> [u8; 48]` for the declared
  default.
- `sti_of(f) -> u32` (`code >> 16`) and a `codec::sti` module of `STI_*` type-id
  constants the kind checks compare against (no crate currently names them; they are
  protocol constants from `SField.h`, hand-written here next to the tests that pin them
  against `sfield.rs`'s generated codes).
- New kind tags for the field table: `KIND_U8_FIELD`, `KIND_U16_FIELD`, `KIND_U64_FIELD`,
  `KIND_HASH128`, `KIND_HASH160`, `KIND_HASH256`, `KIND_CURRENCY`, `KIND_IOU_AMOUNT`,
  `KIND_NATIVE_ISSUE`, `KIND_ISSUE`, `KIND_OBJECT`, `KIND_ARRAY`.
- `FieldEntry` gains a depth column: `(sfcode, kind, payload offset, depth)`.
  `find_field`/`field_present`/`field_kind_ok`/`field_offset_or` only match rows with
  `depth == 0`. This is the one signature change in `codec` (0.x, macro-internal use).

### 3.2 Muncher state (`__txn_template_step!`)

The existing tt-muncher gains four state slots:

| slot | content |
|---|---|
| `prefix = [idents…]` | current setter-name path |
| `ctx = obj \| arr` | what the current container accepts (`arr` accepts only `object` entries) |
| `depth = <usize const expr>` | current nesting depth (table rows, depth assert) |
| `stack = [ { prefix=[…], order=[…], ctx=… } … ]` | saved parent contexts |
| `checks = [ … ]` | accumulated per-container order-check `const _` blocks |

Container rules:

- `<name>: object(sfX) { $($inner:tt)* } $(, $($rest:tt)*)?` (in `ctx = obj`): append
  `sfX` to the current `order`; `init` writes the header at `prev`; `prev += header`;
  push `{prefix, order, ctx}`; `prefix += name`; `order = []`; `ctx = obj`;
  `depth = depth + 1`; `fields = [ $($inner)* , @end_object $(, $($rest)*)? ]`.
- Same rule in `ctx = arr`: identical except `order` is not appended (elements are not
  order-checked) — one extra arm.
- `<name>: array(sfX) [ $($inner:tt)* ]` (in `ctx = obj` only): as `object`, but
  `ctx = arr` for the inner walk and the continuation marker is `@end_array`.
- `@end_object` / `@end_array`: `init` writes the end marker at `prev`; `prev += 1`;
  emit a `const _` order check for the current `order` into `checks`; pop `stack` into
  `prefix`/`order`/`ctx`; `depth = depth - 1`.

The nested `{}`/`[]` groups are flattened into the linear `fields` token list with
`@end_*` continuation markers, so every scalar rule stays a single arm and needs no
knowledge of nesting beyond reading `prefix`/`depth`. Setter names splice
`[<set_ $($prefix _)* $field>]` through the existing `$crate::__paste!`.

Scalar rules: each existing arm is edited to (a) emit `[<set_ $($prefix _)* $field>]`,
(b) push `(code, kind, off, $depth)` rows, and (c) add the STI assertion. New scalar kinds
are one arm each following the `u32_field`/`account_id` pattern. Scalar arms match only
`ctx = obj`; a scalar directly inside an array falls through to the catch-all arm (§4.5).

Base arm: unchanged apart from emitting `$($checks)*` and the top-level order check, and a
final `const _: () = assert!(<max depth> <= STO_WRITER_MAX_DEPTH)`. `prepare_for_emit`
and `Prepared` are untouched.

### 3.3 Compile-time checks (all named `const` assertions or `compile_error!`)

Existing: canonical order (now per container), six required fields present at top level
with the right kinds, `emit_details` present and last.

New:

- STI agreement for every field: `u32_field(sfFee)`, `hash256(sfAccount)`,
  `object(sfAmounts)`, `array(sfAmountEntry)` and the like are rejected with a message
  naming the field and the expected serialized type.
- `emit_details` inside a container: `compile_error!`.
- A scalar or nested `array` directly inside an `array`: `compile_error!` (only
  `object` elements).
- Depth over `STO_WRITER_MAX_DEPTH`.
- A catch-all arm (`fields = [ $($bad:tt)* ]`) giving
  `compile_error!("txn_template!: unrecognized field declaration …")` instead of today's
  bare "no rules expected the token" failure.

## 4. Tests

- `txn.rs` unit tests: a byte-exact fixture per new kind (header + payload + default),
  the `Remit` template above with its expected fixed prefix (header/`0xE1`/`0xF1`
  positions), setter offsets for nested paths, `amount` default triple bytes,
  `encode_iou_amount_value_const` against hand-derived reference vectors for a sample of
  canonical XFLs (zero, positive, negative, minimum/maximum exponent — `rshooks-testenv`
  is not a dev-dependency of `rshooks`, so these are checked by hand against the bit
  layout in §2.2, not against a second encoder), and `FieldEntry` depth filtering (a
  nested `sfAccount` must not satisfy presence).
- `tests/ui/fail`: STI mismatch, scalar inside array, `emit_details` nested, order
  violation inside an object, unknown kind, depth overflow.
- `tests/ui/pass`: a nested template compiles and its inner setters resolve.
- New example `examples/21_txn-template-nested` (Remit with two fixed `AmountEntry`s:
  one `native_amount`, one `amount` with a baked currency/issuer), with a testenv test
  asserting the emitted bytes' `sfAmounts` contents through `TestEnv`, plus
  `metrics.json`. Byte parity with `examples/17_sto-writer`'s two-entry Remit prefix is
  asserted in the example's test (same fields, same order, same bytes).
- `examples/10_emit-txn` bytes/`metrics.json` unchanged (`mise run
  record-example-metrics --check`).

## 5. Documentation

- `book/src/emit/emitting.md`: replace "one of four uniform kinds" with the kind table
  (§2.1), an `amount` subsection (48-byte form, `set_x_value` hot path, default triple),
  and a nested-containers subsection using the Remit example; point runtime-sized shapes
  at `StoWriter` as today.
- `docs/DESIGN.md` §5.5: kind list, per-container ordering, top-level-only plumbing
  detection, the XFL→`STAmount` bit identity.
- `txn.rs` module docs and `examples/README.md` row for the new example.

## 6. Deferred (out of scope here)

- Non-empty VL kinds: `vl(sfX, N)` (fixed-length blob, e.g. `sfPublicKey` 33 bytes,
  `sfMemoData` with a fixed literal), `Vector256` (`sfURITokenIDs`, `sfHookNamespaces`),
  `PathSet`. These need VL length-prefix encoding (1–3 bytes, size-dependent) and are the
  next iteration.
- `Number` (`sfNumber`), `UInt192` (`sfMPTokenIssuanceID`), `XChainBridge`: no Xahau
  transaction emits them today; add on demand with the `fixed_field_size` pattern.
- `; N` element repetition sugar for homogeneous arrays (needs a proc macro).
- Type-level guard that an `amount` field with the zero default is set before emit (a
  runtime "unset" sentinel would cost WCE on every emit; a typestate would change the
  template's public shape). Left to the host's own validation.
