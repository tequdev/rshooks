# NOP-padded optional and variable-length fields in `txn_template!` — design

Status: implemented. Extends `txn_template!` (`crates/rshooks/src/txn.rs`, `docs/DESIGN.md`
§5.5, `docs/TXN_TEMPLATE_FIELDS_DESIGN.md`) with fields that are present-or-absent, or of a
runtime-chosen length, while keeping every byte offset compile-time-fixed. The mechanism is
xahaud's serialized-object NOP: a `0x99` byte where a field header is expected is skipped by
the deserializer.

## 1. The host mechanism

`STObject::set(SerialIter&, int depth)` and `STArray::STArray(SerialIter&, SField const&, int
depth)` (xahaud `src/libxrpl/protocol/STObject.cpp`, `STArray.cpp`; introduced by commit
`24384be242` "allow nops to be specified in emitted txns", shipped in every release since
2023.12.29, not amendment-gated) both begin their field loop with:

```cpp
uint8_t nop_counter = 0;
while (!sit.empty()) {
    int type, field;
    sit.getFieldID(type, field);
    if (type == 9 && field == 9) {          // the single byte 0x99
        if (++nop_counter == 64)
            Throw<std::runtime_error>("Too many NOPS");
        continue;
    }
    ...
```

Consequences that shape this design:

- A NOP is exactly one byte, `0x99`, in a **field-header position** — between fields of an
  `STObject`, or between elements of an `STArray`. It carries no value.
- The limit is **63 NOPs per container instance, cumulative**, not consecutive: the counter
  is local to one `set` call (one `STObject` level) or one `STArray` constructor and is not
  reset by intervening real fields. Every nested object and every array has its own counter.
- `emit` and `etxn_fee_base` both parse the blob through `STTx(SerialIter&)`, so NOPs are
  invisible to the fee computation, the transaction hash, and the ledger's stored
  (re-serialized) form.
- The Hook API's own lightweight parser (`HookAPI::get_stobject_length`, behind
  `sto_validate`/`sto_subfield`/`sto_subarray`/`sto_emplace`/`sto_erase`) rejects
  `STI_NUMBER` (type 9) outright. NOP-padded bytes must never be handed to the `sto_*` family.
  No library or example code does so today; the rule is documented, not enforced.
- Client-side codecs (ripple-binary-codec and its forks) do not know the NOP. Only the
  pre-emit blob ever contains one; nothing off-ledger needs to decode it except
  `rshooks-testenv`.

## 2. Goals and non-goals

Goals:

- **Optional fields**: any fixed-width kind, a nested `object`, a nested `array`, or an
  array element may be declared `optional`. Absent is the baked default (the whole slot is
  NOPs); setting writes the header and value at the same compile-time offset every other
  kind uses; clearing restores the NOPs. An absent field costs zero instructions.
- **Runtime-selected `Amount` form** (`any_amount`): one 49-byte slot that holds either the
  8-byte native form followed by 40 NOPs, or the 48-byte issued form.
- **Variable-length `VL`** (`vl(sfX, MIN, MAX)`): a slot sized for `MAX` whose payload length
  is chosen at runtime within `[MIN, MAX]`, the unused tail NOP-filled.
- **Every host constraint that can be decided from the declaration is a compile error**:
  the 63-NOP budget per container (worst case over every optional/variable field the
  container directly holds), `MIN <= MAX`, `MAX <= MAX_VL_LEN`, and "an emit-plumbing field
  is never optional". Nothing is left to a runtime `EMISSION_FAILURE` that the macro could
  have caught.
- `rshooks-testenv` accepts NOPs exactly where xahaud does (emit parse, `Prepared` decoding,
  `otxn::from_emitted`) with the same 63-per-container limit, and keeps rejecting them
  exactly where xahaud does (`sto_*`).
- Existing templates are byte-identical (`examples/10_emit-txn`, `21_txn-template-nested`
  unchanged in `metrics.json` and output bytes).

Non-goals:

- Runtime element **counts** beyond what one array's 63-byte budget allows. A
  `GenesisMint`-style 0..=21-element array stays `StoWriter`/hand-rolled territory
  (`examples/80_governance/src/mint_txn.rs`).
- A `StoWriter` NOP primitive. `StoWriter` already handles runtime shapes by construction.
- Any change to `prepare_for_emit`'s contract or to `Prepared`.

## 3. Grammar

```text
<field> :=
    <existing kinds, unchanged>
  | <name>: optional <scalar_kind>(sfX $(, N)?)          // no `= default`; absent by default
  | <name>: any_amount(sfX)                              // 49-byte slot, IOU zero by default
  | <name>: optional any_amount(sfX)
  | <name>: vl(sfX, MAX)                                 // == vl(sfX, 0, MAX)
  | <name>: vl(sfX, MIN, MAX)
  | <name>: optional vl(sfX, MIN, MAX)
  | <name>: optional <View>: object(sfX) { <field>* }    // whole object present-or-absent
  | <name>: optional <View>: array(sfX) [ <element>* ]   // whole array present-or-absent
  | <name>: array(sfX) [ <Elem>: optional object(sfY) { <field>* } ; <N> ]  // per-element

  // inferred spellings (§3.3): kind taken from sfX's own serialized type
  | <name>: optional sfX                                 // any inferable scalar kind
  | <name>: optional <View>: sfX { <field>* }             // == optional <View>: object(sfX) { .. }
  | <name>: optional <View>: sfX [ <element>* ]           // == optional <View>: array(sfX) [ .. ]
  | <name>: sfX [ <Elem>: optional sfY { <field>* } ; <N> ]  // == array(sfX) [ Elem: optional object(sfY) { .. } ; N ]

<element> := <name>: object(sfX) { <field>* }
           | <name>: sfX { <field>* }                    // inferred: == object(sfX) { .. }
           | <name>: optional <View>: object(sfX) { <field>* }
           | <name>: optional <View>: sfX { <field>* }    // inferred: == optional <View>: object(sfX) { .. }

<scalar_kind> := u8_field | u16_field | u32_field | u64_field | hash128 | hash160 | hash256
               | currency | native_amount | amount | native_issue | issue | account_id
               | empty_vl | fixed_vl
```

`optional` scalars take no `= default` (the default is "absent"). `optional
native_issue`/`optional empty_vl` are allowed: the value is fixed, so the setter takes no
argument and just makes the field present.

**Inferred spellings.** Every `optional` form above has a bare-`sfX` twin, on the same terms
`docs/TXN_TEMPLATE_FIELDS_DESIGN.md` §2.7 already gives the non-`optional` kinds: the kind is
read off `sfX`'s own serialized type (`Infer<{ sti_of(sfX) }>`), not spelled out. `optional
sfX` covers every scalar kind `InferKind` already infers for the non-`optional` form (`u8`–
`u64`, `hash128`/`hash160`/`hash256`/`currency`, `account_id`); a non-inferable STI (`Amount`,
`VL`, `Issue`, an object/array with no `{ .. }`/`[ .. ]` body) is a named compile error pointing
at the matching explicit `optional` form (`optional native_amount`/`optional amount`/
`any_amount`/`optional any_amount`, `optional empty_vl`/`optional fixed_vl`, `optional vl`,
`optional native_issue`/`optional issue`). `optional <View>: sfX { .. }`/`[ .. ]` and a
homogeneous array's `Elem: optional sfY { .. }` are pure token rewrites into the explicit
`object`/`array` forms above — same budget, same generated API, only the spelling differs.

### 3.1 Kind table (additions)

| kind | slot bytes | baked default | worst-case NOPs charged to the enclosing container | setters |
|---|---|---|---|---|
| `optional <scalar>(sfX)` | that kind's header + value | all NOPs | slot bytes | `set_x(<same args as the kind>)`, `clear_x()` |
| `any_amount(sfX)` | header + 48 | header + issued zero (48 bytes) | 40 | `set_x_native(u64) -> Result<()>`, `set_x_issued(XFL, &CurrencyCode, &AccountId)` |
| `optional any_amount(sfX)` | header + 48 | all NOPs | header + 48 | the two above, plus `clear_x()` |
| `vl(sfX, MIN, MAX)` | header + `vl_length_prefix(MAX)` + MAX | header + prefix(MIN) + MIN zero bytes + NOPs | `slot(MAX).saturating_sub(slot(MIN))` where `slot(n) = prefix_len(n) + n` | `set_x(&[u8]) -> Result<()>` (length must be in `[MIN, MAX]`, else `HookError::InvalidArgument`) — call at most once per hook execution, see §3.1.1 |
| `optional vl(sfX, MIN, MAX)` | as above | all NOPs | slot bytes | `set_x(&[u8]) -> Result<()>`, `clear_x()` — same once-per-execution rule |
| `optional View: object(sfX) { .. }` | header + inner + `0xE1` | all NOPs | slot bytes | `enable_x() -> View<'_>` (restores `View::TEMPLATE`, returns the view), `x() -> Option<View<'_>>` (`None` when the slot's first byte is a NOP), `clear_x()` |
| `optional View: array(sfX) [ .. ]` | header + elements + `0xF1` | all NOPs | slot bytes | same trio |
| `[Elem: optional object(sfY) { .. }; N]` | N × `Elem::LEN` | every element absent (all NOPs) | self-contained: `N * Elem::LEN <= 63` is asserted against **this array's own** budget (these NOPs sit in the array's own `STArray` field loop), charging the parent nothing | the runtime-indexed accessor is unchanged (`Option<Elem<'_>>`, `None` only for an out-of-range index — an in-range but absent element is `Some` of a view whose `is_present()` reads `false`); on the view: `Elem::enable(&mut self)`, `Elem::clear(&mut self)`, `Elem::is_present(&self) -> bool`, plus the inner setters |

A scalar `optional` setter writes `header + value` in one go; its offset constant is the slot
start (header included), unlike the existing kinds whose `OFF` is the value offset. `clear_x`
writes `[NOP; SLOT]` with a compile-time `SLOT` — always at most 63 bytes, so it never lowers
to a `memset` libcall (the build-time failure `docs/DESIGN.md` §2 describes for larger
fills). `FieldEntry`'s `payload offset` column keeps the same meaning between a kind's
`optional` and non-`optional` forms (both `account_id` and `optional account_id` record
`header + 1`, past the VL length byte).

`any_amount`'s native form is `header + encode_native_amount(drops) + [NOP; 40]`; the 40 NOPs
sit after the 8-byte value, where the parser expects the next field header. Its issued form
is exactly the existing `amount` encoding. `set_x_native` returns `Err(InvalidArgument)` for
drops above the native maximum, as `native_amount`'s setter does.

Every `optional`/`vl`/`any_amount` field whose `sfcode` is one of the six emit-plumbing codes
is rejected **only at depth 0** — `find_field`/`prepare_for_emit` only recognize a plumbing
field at the top level (`FieldEntry`'s `depth` column), so a same-named field nested inside
an `object`/`array` (e.g. `optional account_id(sfAccount)` inside a `Signer`-shaped object,
alongside a required top-level `sfAccount`) is not plumbing at all and may freely be
`optional`/`vl`/`any_amount`.

#### 3.1.1 `vl`'s setter, and its guard budget

`vl`'s setter writes the header and `vl_length_prefix(len)` with direct, unguarded stores
(the prefix is at most 3 bytes — one unconditional store plus two `if width >= 2/3` stores,
no loop), then one `guard_m!`-protected loop over the *rest* of the reserved region
(`vl_slot_size(MAX) - prefix_len`, itself bounded by the compile-time constant
`vl_slot_size(MAX)`) writing `value.get(i).copied().unwrap_or(NOP)` per position — a single
branch-light expression standing in for what would otherwise be a three-way
prefix/payload/NOP-tail branch inside the loop. This roughly halved the setter's contribution
to worst-case instruction count end to end (`examples/22_txn-template-optional`'s `pay`
entry: WCE 7796 → 4434 after this change, measured with `mise run build-examples` /
`rshooks check`).

**The guard budget is per hook execution, not per call**: xahaud's `_g` counter is cumulative
for the whole hook run (`docs/DESIGN.md` §2 C2), so calling a `vl` field's `set_x` more than
once in one execution — including once per inlined call site, if the compiler duplicates the
setter — can exceed the declared budget and abort with a guard violation. Each `vl`/`optional
vl` setter's doc comment says so explicitly ("call at most once per hook execution for this
field").

`line!()`, used directly inside `__txn_template_step!`'s own body (not from a captured
fragment), resolves to wherever that `line!()` token is written in `crates/rshooks/src/txn.rs`
— the `vl` arm's own definition — not to the `txn_template!` call site in the hook author's
source. Every `vl` field's setter, across every template in the crate, therefore shares the
same `line!()` value, so `guard_m!`'s `$n` disambiguator alone must make two `vl` fields'
guard ids distinct. `codec::fnv1a_16` (a tiny compile-time FNV-1a hash, truncated to 16 bits)
hashes `concat!(stringify!($Name), "::", stringify!($($prefix)*), stringify!($field))` — the
declaring template name and the field's full `_`-joined path, unique per field within one
template since two fields sharing both would already be a duplicate method name, a plain Rust
compile error — XORed with the field's own slot offset (`REGION_OFF & 0xFFFF`), giving each
`vl` field a disambiguator that is distinct except by hash collision, including between two
`vl` fields at the same offset in two different templates in one crate (a view's `prev`
restarts at its own container header, so offsets alone collide across templates/views). The
hash is computed in a `const` binding, so it costs nothing at runtime.

### 3.2 Compile-time checks (all `E0080` `const _: () = assert!(..)` items, one per check)

- **NOP budget**: for every container — the top level, each named `object`, each named
  `array`, each homogeneous array, each `optional` view, each homogeneous element view — the
  sum of the worst-case NOP column above over its direct children is `<=
  codec::MAX_NOPS_PER_CONTAINER` (63). The message names the container and the budget.
  Worst case means "everything optional absent at once, every `vl` at `MIN`, every
  `any_amount` native", which is exactly the state the host may be asked to parse.
- **Plumbing never optional**: an `optional` (or `vl`, `any_amount`) field whose `sfcode` is
  one of `sfSequence`, `sfFirstLedgerSequence`, `sfLastLedgerSequence`, `sfFee`,
  `sfSigningPubKey`, `sfAccount` **at depth 0** is an error with a message saying so
  (independently of the existing kind-agreement check, whose message would be misleading
  here). Every `optional`/`vl`/`any_amount` arm runs this check, plain (non-`optional`)
  `any_amount`/`vl` included — `field_kind_ok`'s depth-0-only required-field check would
  already reject those (the wrong `KIND_*` tag), but the dedicated message is clearer. A
  nested field with the same `sfcode` (`depth != 0`) is exempt.
- **`vl` bounds**: `MIN <= MAX`, `MAX <= codec::MAX_VL_LEN`, and `MAX >= 1` (an
  always-empty blob is `empty_vl`). The NOP-budget charge for `vl(sfX, MIN, MAX)` uses
  `slot(MAX).saturating_sub(slot(MIN))`, not a bare subtraction: when `MIN > MAX` (already a
  named compile error above), a bare `usize` subtraction would underflow and `write_nops`'s
  own bounds assert would fire too, burying the one useful message under a second, confusing
  one.
- The existing STI-agreement, canonical-order, and depth checks apply unchanged;
  `optional` does not change a field's `sfcode` position in the order list.

### 3.3 Macro state

`__txn_template_step!` gains a `nops = [ <expr>, ... ]` accumulator carried exactly like
`order`: reset to `[]` when a container opens, saved in the `stack` frame, and asserted
against the budget at `@end_object` / `@end_array` / the `mode = tpl` base case (the
`mode = elem`/`mode = elem_opt` base cases don't separately assert — by the time either is
reached, the spawn's own `@end_object`/`@end_array` has already run and restored the popped,
now-unused `nops`). The homogeneous-array arm (both the plain and the `optional`-element
forms) asserts its own array's budget inline (there is no `@end_array` for it in the parent
stream); for the `optional`-element form this charges nothing to the parent — the array field
itself is never optional, so its header always writes, and the potential NOPs are inside the
array's own `STArray` field loop, not the parent's.

Each `stack` frame gained a fifth element, the container's own field name (an `ident`):
`[ [prefix] [order] [nops] name ctx ]`, populated by every arm that pushes a frame (including
the `optional`/homogeneous-`optional`-element spawns' single dummy frame, which uses the
spawned view/element type's own name, `$View`/`$Elem`) and read back by `@end_object`/
`@end_array` to name the container in the budget-overflow message (`` `entry`'s
optional/variable-length fields could together need more than 63 NOPs `` rather than a bare
"this object's").

An `optional` container is compiled the way a homogeneous element already is, but with its
own macro `mode`, `elem_opt` (distinct from the existing `elem`): a sub-invocation emits the
view type (`View::LEN`, `View::TEMPLATE`, inner setters, `enable()`, `clear()`,
`is_present()`), and the parent bakes `[NOP; View::LEN]` and generates the
`enable_`/accessor/`clear_` trio — the accessor (no `enable_` prefix) returns
`Option<View<'_>>`, `None` when the slot's first byte is a NOP, so calling an inner setter
through it can never write into an absent (NOP-filled) slot and corrupt it; `enable_x`
remains unconditional (`View`'s underlying byte range is always in bounds — only whether it
currently holds a real header or NOPs is in question). `elem_opt` is a genuinely separate
`mode` value (not a flag folded into the existing `elem` base case) so the many
already-existing `mode = elem` homogeneous-array element types (which have no need for
`enable`/`clear`/`is_present`) keep their existing shape and dead-code baseline untouched.
`mode = elem_opt` learns to start in `ctx = arr` (closing with `@end_array`) so an optional
*array* view works the same way; a homogeneous array's own (always-`Option<Elem<'_>>`,
bounds-checked) indexed accessor is unchanged — only its element view type moves to
`elem_opt` when the element is `optional object(sfY) { .. }`, gaining `enable`/`clear`/
`is_present` without changing the accessor's signature.

New `FieldEntry` kind codes (`KIND_OPTIONAL_*`, `KIND_ANY_AMOUNT`, `KIND_VL`) keep
`field_kind_ok` exact; `field_present`/`find_field` are unchanged (presence in the table is
about the declaration, not the runtime state, and both already only match a depth-0 row).

### 3.4 `codec` additions

- `pub const NOP: u8 = 0x99;`
- `pub const MAX_NOPS_PER_CONTAINER: usize = 63;`
- `pub const fn write_nops<const N: usize>(bytes: &mut [u8; N], offset: usize, n: usize)` — the
  same `<const N: usize>`-array shape as the existing `write_const_bytes`/`write_field_header`
  (not the sketched `&mut [u8]`), used only to bake compile-time-constant-length NOP fills
  into `new()`/`TEMPLATE`; `vl`'s own runtime, variable-length NOP tail is written by its
  setter's own guarded loop instead (§3.1.1), never through this helper.
- `pub const fn vl_slot_size(n: usize) -> usize` (= `vl_length_prefix(n).1 + n`)
- `pub const fn vl_field_size<T>(f: SField<T>, max: usize) -> usize` (= header +
  `vl_slot_size(max)`)
- `pub const fn any_amount_field_size(f) -> usize` (= header + 48, an alias of
  `iou_amount_field_size` under a name that documents `any_amount`'s own semantics)
- `pub const ANY_AMOUNT_NATIVE_NOPS: usize = 40;`
- `pub const fn is_one_of(needle: u32, haystack: &[u32]) -> bool` — backs the plumbing check,
  generic over the comparison codes rather than importing `sfield` names into `codec`.
- `pub const fn fnv1a_16(bytes: &[u8]) -> u16` — the `vl` guard-id disambiguator's hash
  (§3.1.1).

Two small internal helper macros (not part of `codec`, `#[doc(hidden)]` `#[macro_export]`
like `__txn_template_step!` itself) factor out logic shared by several arms:
`__txn_template_check_nop_budget!(label_expr, $($nops:tt)*)` (the sum-and-assert against
`MAX_NOPS_PER_CONTAINER`, used at `@end_object`/`@end_array`/the top-level base case) and
`__txn_template_check_not_plumbing!($sfcode, $field, $kindword_literal, $depth_expr)` (§3.2's
depth-0-only plumbing check).

## 4. `rshooks-testenv`

`emit_walk.rs`'s three walkers (`walk_fields`, `walk_array_body`, `walk_array_elements`)
gain a NOP mode selected by the caller: **tolerant** (skip `0x99`, count per container, fail
past 63 with a distinct error) for the emit path (`validate_emit_blob`, `Prepared`
decoding, `otxn::from_emitted`, `emitted()` inspection), **strict** (today's behavior: a
`0x99` header is an unknown type and the parse fails) for `host/sto.rs` and
`host/slots.rs`, matching `get_stobject_length`. `decode_header` itself is unchanged.
`TestEnv`'s emit failure for "Too many NOPS" surfaces as `EMISSION_FAILURE`, as on the host.

## 5. Examples, metrics, docs

- New `examples/22_txn-template-optional`: three `#[hooks]` entries, `tplpay`
  (`OptionalPayment`, a Payment), `tplremit` (`OptionalRemit`, a Remit), and `tplvl`
  (`OptionalVl`, a second Remit), together exercising every new kind at least once, mostly
  through the inferred spellings (§3.3) — explicit only where a kind cannot be inferred
  (`any_amount`, `native_amount`, `empty_vl`, `fixed_vl`, `vl`, `amount`). Every field in all
  three templates is legal for its transaction type per
  `crates/rshooks-core/protocol_formats.json` (a sfield only that format's `tx_common`/
  type-specific list actually allows) — `OptionalPayment`'s `send_max: optional
  any_amount(sfSendMax)` and `OptionalRemit`/`OptionalVl`'s `blob`/`note: vl(sfBlob, ..)` use
  `sfSendMax`/`sfBlob` rather than an arbitrarily-chosen `sfcode`, and `OptionalRemit`/
  `OptionalVl`'s whole-container-optional views use `sfMintURIToken`'s own `flags`/`uri`
  sub-fields rather than a stand-in `object`. `OptionalRemit::amounts` is a named array
  (`first: sfAmountEntry { amount: any_amount(sfAmount) }`, `second: optional Second:
  sfAmountEntry { .. }`) — the motivating case for a named array over a homogeneous one: one
  required entry (always written to a real, constructible amount — `remit` never leaves it at
  its raw encoding default) and one that may or may not be there — while `OptionalVl::amounts`
  is the homogeneous "`0` or `1` entries" case (`sfAmounts [ Entry: optional sfAmountEntry {
  .. } ; 1 ]`). `env.emitted()` is canonical (NOP-free, matching the ledger's re-serialized
  form), so the byte-level NOP-padding assertions (a field's raw pre-emit slot: `NOP`-filled
  when absent/`MIN`-length, filled exactly when present/`MAX`-length, and — for `note` — that
  shrinking back to `MIN` after writing `MAX` re-`NOP`-fills the vacated tail rather than
  leaving stale bytes) live in `src/lib.rs`'s in-crate `#[cfg(test)]` module (the template
  types are private) against the template's own `bytes()`, never `prepare_for_emit`/`emit`;
  `tests/pay.rs`/`tests/remit.rs`/`tests/vl.rs` instead assert the decoded functional behavior
  against `emitted()` — present with exactly the written bytes, absent by byte-count delta
  against the present-state blob (not by searching for the header's *absence*, since a short
  header can coincidentally match part of a longer, unrelated one elsewhere in the blob), and
  each `amounts` field's element count by counting the array's own `0xE1` element terminators
  — covering both states for every kind, `any_amount`'s native/issued forms, and `note`'s
  `MIN`/`MAX` lengths. Includes a `metrics.json`.
- `examples/17_sto-writer` stays as the `StoWriter` demonstration; its README points at 22
  for the fixed-shape alternative.
- `tests/ui/fail`: budget overflow at the top level, inside a nested object, inside an
  array; an optional plumbing field; `vl` with `MIN > MAX`; `vl` with `MAX = 0`; `optional`
  scalar given a `= default`, explicit or inferred; a non-inferable STI given a bare `optional
  sfXxx`. `tests/ui/pass`: one template using every optional form, explicit and inferred,
  plus the named-array motivating case (one required element, one `optional`).
- `docs/DESIGN.md` §5.5, `docs/TXN_TEMPLATE_FIELDS_DESIGN.md` §1/§6 (the deferred
  variable-length `VL` item now points here), `book/src/emit/emitting.md` (new "Optional and
  variable-length fields" section, the `StoWriter` hand-off narrowed to runtime element
  counts), `book/src/emit/sto-writer.md` intro, `examples/README.md` row.
