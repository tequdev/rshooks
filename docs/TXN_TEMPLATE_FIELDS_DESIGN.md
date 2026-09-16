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
  (Present-or-absent fields and a runtime-chosen-length `VL` within a fixed `MAX` are no
  longer out of scope for `txn_template!` as of `docs/NOP_PADDING_DESIGN.md`; only runtime
  element *counts* remain `StoWriter`'s job.)
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
  | <name>: fixed_vl(sfX, N) $(= <[u8; N] expr>)?  // fixed-length VL blob
  | <name>: object(sfX) { <field>* }              // STObject, closed by 0xE1
  | <name>: array(sfX) [ <element>* ]             // STArray, named elements, closed by 0xF1
  | <name>: array(sfX) [ <Elem>: object(sfY) { <field>* } ; <N> ]  // STArray, homogeneous, indexed
  | <name>: sfX $(= <expr>)?                      // inferred scalar kind (§2.7)
  | <name>: sfX { <field>* }                      // inferred object
  | <name>: sfX [ <element>* ]                    // inferred array
  | <name>: sfX [ <Elem>: sfY { <field>* } ; <N> ] // inferred array, homogeneous, indexed
  | <name>: sfX = NativeAmount(<u64 drops>)                 // default-shape: infers native_amount (§2.7)
  | <name>: sfX = IouAmount(<XFL>, <CurrencyCode>, <AccountId>)  // default-shape: infers amount (§2.7)
  | <name>: sfX = AnyAmount()                     // default-shape: infers any_amount (§2.7), no value
  | <name>: sfX = []                              // default-shape: infers empty_vl (§2.7)
  | <name>: sfX = [ <elem>+ ]                     // default-shape: infers fixed_vl, N from the literal (§2.7)
  | <name>: sfX = *<byte string literal>          // default-shape: infers fixed_vl, N from the literal (§2.7)

<element> := [<name>:] object(sfX) { <field>* }   // only objects directly inside an array
           | [<name>:] sfX { <field>* }          // inferred object element
```

Trailing commas are accepted everywhere a field list is accepted (as today), except after
`<N>` in the homogeneous array form. `emit_details` inside an `object`/`array` is a compile
error (§4.5).

`<Elem>` names the generated element-view type; `<N>` is a `usize` const expression
(literal or a named const), at least 1. See §2.5.

`<name>:` is optional on a named array's own element (only — the homogeneous `<Elem>: ..; N`
form is unaffected, its `<Elem>` is never optional): an element without one is numbered by
its zero-based position among every element in the list, named or not — see §2.7.

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
| `fixed_vl(sfX, N)` | VL (7) | `vl_length_prefix(N)` + N | zeroed, or the declared `[u8; N]` | `set_x(&[u8; N])` |
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
  entry, one issued entry) fall out naturally. A homogeneous, indexed form for the case
  where every element has the *same* shape is §2.5.
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

### 2.5 Homogeneous arrays

```rust,ignore
txn_template! {
    struct Remit {
        // .. required fields ..
        amounts: array(sfAmounts) [
            AmountEntry: object(sfAmountEntry) {
                amount: amount(sfAmount) = (XFL!(0), USD, USD_ISSUER),
            }; 3
        ],
        emit_details: emit_details,
    }
}

let mut entry = txn.amounts(1).expect("index in range"); // Option<AmountEntry<'_>>
entry.set_amount_value(XFL!(1.5));
```

For the case where every element has the *same* declared shape, `array(sfX) [ Elem:
object(sfY) { <field>* } ; N ]` declares that shape once and reserves `N` back-to-back
copies of it, generating:

- A standalone element-view type, named `Elem`: `pub struct Elem<'a> { bytes: &'a mut
  [u8] }`, with `Elem::LEN` (that element's fixed byte length: header + inner fields +
  `0xE1`), `Elem::TEMPLATE: [u8; Elem::LEN]` (the baked default, copied `N` times into the
  reserved region), the *same* inner setters (`set_amount`/`set_amount_value`, ...) a
  `txn_template!` struct itself would generate for the identical field list, and `bytes()`.
  There is no owned constructor — a view is only ever produced by the parent's accessor.
- On the parent, a runtime-indexed accessor named by the field path (no `set_` prefix):
  `fn amounts(&mut self, index: usize) -> Option<Elem<'_>>`, `None` for `index >= N`,
  otherwise a view over `self.bytes[OFF + index * Elem::LEN .. OFF + (index + 1) *
  Elem::LEN]` (`slice::get_mut`, no unsafe, no raw indexing panic).

A view over `&mut [u8]`, not a `#[repr(C)]`/transmuted struct or a direct `txn.amounts[n]`
index expression, is deliberate: the generated types stay ordinary safe Rust, and the
workspace's `indexing_slicing` lint (`docs/DESIGN.md` §8) would make a raw `[n]`
panic-on-out-of-range unusable inside a hook anyway — `Option` makes the out-of-range case
an ordinary checked branch. Wire bytes: `header(sfX)`, `N` copies of `Elem::TEMPLATE`
back to back, then `0xF1` — a new `codec::write_repeated` helper (`write_const_bytes`
applied `N` times at `Elem::LEN`-sized strides) bakes the `N` copies at compile time.

Nesting depth: a homogeneous array's element counts as **two** levels against
`STO_WRITER_MAX_DEPTH` (the array itself, then the element), the same as a named array's
object element — checked once, directly, rather than through two separate nested-entry
steps. A homogeneous array may itself be declared inside another homogeneous array's
element (or a named object), to whatever depth that bound allows.

Implementation: the homogeneous-array arm spawns a **second, independent**
`$crate::__txn_template_step!` invocation for `Elem`, seeded fresh (its own `order`,
`prefix`, a single `stack` frame, and a new `mode = elem` state slot every arm threads
through unchanged) with `fields = [ ..inner.., @end_object ]`, so the *existing*
`@end_object` arm closes it and runs its order check exactly as it would for a named
object. An `elem`-mode base case (`fields = []`, `mode = elem`) emits only the view type
above — none of a `tpl`-mode base case's plumbing/presence/kind asserts,
`prepare_for_emit`, or `TemplateBytes`/`Default`/`Clone`. `Self::LEN` can't name the array
length of a sibling `Self::TEMPLATE` const in the same impl (an anonymous-const-with-
generic-`Self` restriction, since `Self` carries a lifetime parameter here) — `LEN` is a
private, module-level const outside the impl instead, and both `Elem::LEN` and
`Elem::TEMPLATE` alias it.

### 2.6 `fixed_vl` (fixed-length VL blob)

```rust,ignore
memo_type: fixed_vl(sfMemoType, 4) = *b"note",
memo_data: fixed_vl(sfMemoData, 8),
```

`fixed_vl(sfX, N)` declares an `STI_VL` field whose length is fixed at declaration time,
so its rippled VL length prefix is computed and baked in at compile time exactly like any
other kind's header. `N` is a `usize` const expression (a literal or a named const), at
least 1 — `N = 0` is a compile error, since `empty_vl` is the one spelling for an empty
blob (so `sfSigningPubKey`'s required-kind check keeps rejecting `fixed_vl(sfSigningPubKey,
..)` with its existing "must be declared as `empty_vl`" message, unaffected by this kind
existing). Works at the top level and inside any `object`/element, the same `ctx = obj`
rule every other scalar kind follows.

Wire bytes: `header(sfX)`, then rippled's VL length prefix for `N` — `vl_length_prefix`
(new in `codec`) implements the encoding directly: `N <= 192` is a single byte (`N`
itself); `193..=12480` is two bytes, `[193 + ((N - 193) >> 8), (N - 193) & 0xFF]`;
`12481..=918744` is three, `[241 + ((N - 12481) >> 16), ((N - 12481) >> 8) & 0xFF, (N -
12481) & 0xFF]` — then `N` payload bytes. Default is `N` zero bytes unless a `= <[u8; N]
expr>` is declared; the setter takes `&[u8; N]`, an infallible fixed-size write. A
declared default whose length doesn't match `N` is a compile-time type error (`let __d:
[u8; N] = <expr>;` inside the generated `new()`), not a truncation or a runtime panic.

Only fixed-length `VL` is covered here — `Vector256`/`PathSet` and a genuinely
variable-length blob stay deferred (§6).

### 2.7 Inferred kinds

```rust,ignore
flags: sfFlags = tfCANONICAL,          // infers u32_field
account: sfAccount,                    // infers account_id
invoice_id: sfInvoiceID,               // infers hash256
amounts: sfAmounts [ Entry: sfAmountEntry { amount: amount(sfAmount) }; 2 ],
memos: sfMemos [ m: sfMemo { memo_type: fixed_vl(sfMemoType, 4) } ],
```

`macro_rules!` cannot inspect an identifier's STI directly, so inference is dispatched at
the type level. `sti_of(sfcode)` (already used by every explicit kind's own STI-agreement
check) is a `const fn`, so `Infer<{ sti_of(sfcode) }>` — a unit struct generic over a
`u32` const parameter — is a concrete, nameable type from inside the macro. One
`impl InferKind for Infer<{ STI_X }>` per inferable STI supplies everything a bare
`field: sfXxx` declaration needs: the setter's value type (an associated GAT,
`Value<'a>`), the wire size (`LEN`, plus a `PREFIX` for `account_id`'s VL length byte),
the `FIELDS` kind tag, and the `write` call itself. `INFERABLE`/`HAS_DEFAULT` back the two
compile-time `assert!`s that reject an ambiguous STI or a default/no-default mismatch. A
non-inferable STI (`AMOUNT`, `VL`, `ISSUE`, `OBJECT`/`ARRAY` without a body, `PATHSET`,
`VECTOR256`, `UINT192`, `NUMBER`, `XCHAIN_BRIDGE`) still gets an `impl` — `INFERABLE =
false`, and `Value<'a> = ExplicitKindRequired` (an empty marker whose name doubles as the
type-mismatch diagnostic when a default is ascribed to it). `PATHSET`/`VECTOR256`/
`UINT192`/`NUMBER`/`XCHAIN_BRIDGE` back no `txn_template!` kind at all (explicit or
inferred) — a field with one of those serialized types needs `StoWriter` directly.

An inferred integer field's default is type-checked directly against the field's integer
type (`let __v: u32 = <expr>;`, no `as` cast) — stricter than the explicit kind's own `(<expr>)
as u32` cast, which silently truncates an out-of-range default instead of rejecting it. A
default that only compiled because of that truncating cast does not compile in the bare
form.

`object`/`array` inference is a pure token rewrite, not a type-level dispatch: a bare
`name: sfXxx { .. }`/`name: sfXxx [ .. ]` (or a homogeneous/named array element's own
`Elem: sfY { .. }`) desugars to `name: object(sfXxx) { .. }`/`array(sfXxx) [ .. ]` before
recursing, so it reuses the existing container arms and their nested-order/depth checks
unchanged. Desugar arms are generic over `ctx` (they fire in both `obj` and `arr`) and are
ordered: the container desugars sit right where the analogous explicit-form arm they
rewrite into does (so a partially-explicit spelling like `array(sfX) [ Elem: sfY { .. } ;
N ]` — explicit array, inferred element — still resolves), while the two scalar desugar
arms (with/without default) sit after both `emit_details` arms and before the catch-all,
so `field: emit_details` keeps matching its own dedicated arm first.

Byte-for-byte identity with the explicit spelling is the whole point: `Infer<STI>`'s
`KIND`/`LEN`/`PREFIX`/`write` are computed from the exact same `codec` primitives
(`field_header`, `fixed_field_size`, `write_const_bytes`) the explicit arms call directly,
so the generated `TEMPLATE`, `FIELDS` row, and setter offset are identical either way — see
`crates/rshooks/src/txn.rs`'s `mod tests` for the twinned-fixture proofs.

A named array's own elements (not the homogeneous `<Elem>: ..; N` form) go through one more
rewrite ahead of all of the above: `$crate::__txn_template_index_elements!`, a `rshooks-macros`
proc macro (alongside `$crate::__paste!`), splits the element list on top-level commas and
prepends `<N>:` — `<N>` the element's zero-based position — to whichever ones don't already
start `<tt>:` (an explicit name), before splicing the (now fully named) list back into
`fields = [ .. ]`. Every element arm that captures the name — the explicit `object(sfY) {
.. }`/`optional object(sfY) { .. }` forms and their inferred-kind desugars — takes it as
`$name:tt` rather than `$name:ident` so an integer literal is accepted there too; the
`@end_object`/`@end_opt_object`/`@end_array`/`@end_opt_array` arms that later name the
container in a doc string or a generated method (`stringify!($name)`,
`[<set_ $prefix $name>]`) do the same, since a bare digit literal stringifies and
concatenates (via `$crate::__paste!`'s literal-digit support) exactly like an identifier
does. The homogeneous form is untouched: its `<Elem>` always names a real view type, never a
position.

#### Default-shape desugar

`native_amount`/`amount`/`empty_vl`/`fixed_vl` cannot infer from the STI alone — `AMOUNT`
covers both the native and issued wire shapes, `VL` covers both the empty and
fixed-length ones — so a bare `field: sfXxx = <default>` instead infers the kind from the
*shape* of `<default>`, purely as a token rewrite ahead of the plain inferred-scalar arm
(so `$default:expr` there never gets a chance to parse `NativeAmount(500)` as an ordinary,
unresolvable call expression):

```text
field: sfXxx = NativeAmount(d)              -> field: native_amount(sfXxx) = d
field: sfXxx = IouAmount(xfl, cur, iss)      -> field: amount(sfXxx) = (xfl, cur, iss)
field: sfXxx = AnyAmount()                  -> field: any_amount(sfXxx)
field: sfXxx = []                           -> field: empty_vl(sfXxx)
field: sfXxx = [ <elem>+ ]                  -> field: fixed_vl(sfXxx, N) = [ <elem>+ ]
field: sfXxx = *<byte string literal>       -> field: fixed_vl(sfXxx, N) = *<byte string literal>
```

`NativeAmount`/`IouAmount`/`AnyAmount` are macro syntax markers matched as literal tokens,
not real types or functions — nothing named that way needs to exist in scope. `AnyAmount()`
takes no value: `any_amount` has no baked default to thread through (issued zero, always),
so its rewrite leaves no `= ..` behind, unlike the other two. `N` in the two array/byte-string
rewrites is recovered from the default literal itself via a new `codec::array_len<const N:
usize>(_: &[u8; N]) -> usize { N }`, spliced in as `{ array_len(&[ <elem>+ ]) }`/`{
array_len(&*<byte string literal>) }` — a block expression, since `fixed_vl`'s own `$n:expr`
fragment splices into a `[u8; $n]` array-length position. A `fixed_vl` default spelled as a
named const (`field: sfX = SOME_CONST`) is not one of the literal shapes above, so it
falls through to the plain inferred-scalar-with-default arm and is rejected there —
`fixed_vl(sfX, N) = SOME_CONST` (the explicit form, `N` spelled out) is still required,
since there is no literal to recover `N` from. `issue`/`native_issue`, and a zero-default
`amount(sfX)` (no `=` at all), are untouched by this desugar — none of the shapes above
match them.

#### Inferred `optional` forms

`field: optional sfXxx` is `optional <scalar_kind>(sfXxx)`'s (`docs/NOP_PADDING_DESIGN.md`
§3.1) bare-`sfXxx` twin: `InferKind` gains a second kind tag, `OPTIONAL_KIND` (the
`codec::KIND_OPTIONAL_*` row for this STI's optional form, `u8::MAX` — unused — for a
non-inferable one), alongside the existing `KIND`. The setter writes `header + PREFIX +
value` in one call (mirroring `optional account_id`'s own `PREFIX`-aware setter, generalized
to any `InferKind`), `clear_x` NOP-fills `fixed_field_size(sfXxx, PREFIX.len() + LEN)`, and
that same size is the field's worst-case NOP charge to its enclosing container. A
non-inferable STI names the matching explicit `optional` form in its error message
(`optional native_amount`/`optional amount`/`any_amount`/`optional any_amount`, `optional
empty_vl`/`optional fixed_vl`, `optional vl`, `optional native_issue`/`optional issue`) —
except the three `Amount`-shaped kinds with no baked default, which get their own
default-shape markers, the `optional` twins of the non-`optional` ones above:

```text
field: optional sfXxx = AnyAmount()         -> field: optional any_amount(sfXxx)
field: optional sfXxx = NativeAmount()      -> field: optional native_amount(sfXxx)
field: optional sfXxx = IouAmount()         -> field: optional amount(sfXxx)
```

All three take no value, same as the non-`optional` `AnyAmount()` marker: absent is the
only default an `optional` field has, so there is nothing to thread through even for
`NativeAmount`/`IouAmount`, unlike their non-`optional` forms above (which do take one).

`optional sfXxx { .. }`/`[ .. ]` and a homogeneous array's `Elem: optional sfY { .. }` are
pure token rewrites, exactly like their non-`optional` counterparts above: `optional sfXxx {
.. }` -> `optional object(sfXxx) { .. }` (legal wherever the explicit form is — a
top-level/nested-object field, or a named element inside an array), `optional sfXxx [ .. ]`
-> `optional array(sfXxx) [ .. ]`, and `Elem: optional sfY { .. } ; N` -> `Elem: optional
object(sfY) { .. } ; N` under either a bare or an explicit outer `array(..)`.

#### Named `optional` containers have no view type

`optional object(sfX) { <field>* }`/`optional array(sfX) [ <element>* ]` (whole-container
present-or-absent) compile *inline*, exactly like the plain `object`/`array` forms above —
`<field>*`'s own fields flatten onto the parent as ordinary `set_x_<field>` methods, sharing
the same order/depth/NOP-budget checks — rather than spawning a separate view type. The only
addition: the container's slot defaults to absent (`NOP`-filled), and every one of its own
setters (plus a generated `enable_x(&mut self)`) first ensures it — and every enclosing
`optional` ancestor — is present, copying in its own baked defaults if not. A generated
`clear_x(&mut self)` NOP-fills the whole slot back to absent, and `is_x_present(&self) ->
bool` reads presence without changing it. See `docs/NOP_PADDING_DESIGN.md` §3.3 for exactly
how the container's own writes are captured into a standalone `const` and the ancestor-
presence prelude (`$crate::__txn_template_ensure!`) is threaded through `mode`.

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
- `txn.rs` unit tests for §2.5: a `TestRemitIndexed` template (`AmountEntry::LEN`/
  `TEMPLATE` byte-exact, the full fixed prefix through three repeated elements plus the
  `0xF1` marker, each index's accessor writing without disturbing its neighbours,
  out-of-bounds indices returning `None`, `bytes()` returning exactly `LEN` bytes), and a
  `HookChain` template nesting a homogeneous `sfHookGrants` array inside a homogeneous
  `sfHooks` array's element, proving a two-level indexed accessor
  (`hooks(i)?.grants(j)?`) lands in the right element and leaves every other element at
  its baked default.
- `txn.rs` unit tests for §2.6: `vl_length_prefix` pinned at every boundary (the one-byte
  form, the smallest and largest two-byte lengths, the smallest and largest three-byte
  length, and the over-limit panic), `fixed_vl_field_size`/`write_vl_length_prefix`, a
  `TestMemoFixture` (`sfMemos` holding one `Memo` element with both a defaulted and a
  zeroed `fixed_vl` field, `Memo::LEN`/`TEMPLATE` byte-exact, both setters landing at their
  expected offsets), and a top-level `BlobFixture` at the two-byte prefix boundary
  (`N = 193`) with its header/prefix/payload offsets and setter pinned.
- `tests/ui/fail`: STI mismatch, scalar inside array, order violation inside an object,
  unknown kind, a nested `sfAccount` not satisfying the top-level presence check, nesting
  depth over `STO_WRITER_MAX_DEPTH` (ten levels of plain nested `object`s), a homogeneous
  array's element count below 1, `emit_details` nested -- once inside a named `object`,
  once inside a homogeneous array's element -- a `fixed_vl` field declared with `N = 0`,
  a `fixed_vl` field on a non-`STI_VL` `sfXxx` code, and a `fixed_vl` default whose array
  length doesn't match the declared `N`.
- `tests/ui/pass`: a nested template compiles and its inner setters resolve; a
  homogeneous-array template compiles and its indexed accessor is callable in a loop over
  every valid index; a `fixed_vl` field (with and without a declared default) inside a
  homogeneous array element compiles and its setters are reachable through the element's
  own accessor.
- `sto_writer.rs` unit tests for `StoWriter::vl`: the one-byte and two-byte VL-prefix
  forms, a cross-check against `codec::vl_length_prefix` at every prefix-width boundary,
  and the over-maximum-length rejection.
- New example `examples/21_txn-template-nested`: a Remit whose `sfAmounts` is a
  homogeneous, two-element `AmountEntry` array (both entries issued, baked
  `USD`/`USD_ISSUER` default, written through `Remit::amounts`'s indexed accessor) and
  whose `sfMemos` is a homogeneous, one-element `Memo` array with two `fixed_vl` fields
  (`memo_type` baked at its `*b"note"` default, `memo_data` written at runtime through
  `Remit::memos`'s accessor); hook parameters (`DEST`, an optional `ISSUER` overriding
  the baked issuer through the full 48-byte `amount` setter) are read via declared
  `#[hook_param]` fields. Testenv tests assert the emitted `sfAmounts`/`sfMemos` regions
  byte-exact, plus `metrics.json`. Byte parity with an equivalent `StoWriter`-built
  prefix (`u16_field`/... for the fixed fields, `begin_array`/`begin_object`/`vl`/
  `iou_amount` for `sfMemos`/`sfAmounts`) is asserted over `Remit`'s whole fixed prefix.
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

- `Vector256` (`sfURITokenIDs`, `sfHookNamespaces`) and `PathSet` — distinct wire shapes
  from a plain `VL` blob (`Vector256` is a flat run of 32-byte hashes with no per-element
  header; `PathSet` is its own nested path/step grammar). `fixed_vl(sfX, N)` (§2.6) covers
  every *fixed*-length `VL` field; `vl(sfX, MIN, MAX)`
  (`docs/NOP_PADDING_DESIGN.md`) covers a runtime-chosen-length one within a compile-time
  `MAX`; `Vector256`/`PathSet` remain the actual VL-family gaps.
- `Number` (`sfNumber`), `UInt192` (`sfMPTokenIssuanceID`), `XChainBridge`: no Xahau
  transaction emits them today; add on demand with the `fixed_field_size` pattern.
- Type-level guard that an `amount` field with the zero default is set before emit (a
  runtime "unset" sentinel would cost WCE on every emit; a typestate would change the
  template's public shape). Left to the host's own validation.
