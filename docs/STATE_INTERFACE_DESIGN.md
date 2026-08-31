# Hook State Interface (XAH-009d) — design

Status: implements the **XAH-009d** draft ("HookState On-ledger Interface", provisional
name; spec title "Hook State Interface"). The spec is a draft, so the entire surface is
gated behind the `unstable-state-interface` cargo feature (default off). `unstable-*`
features are exempt from semver — breaking changes may land in a minor release while the
spec is a draft.

The interface is a `HookParameters` convention that exposes a Hook's state layout as a
machine-readable, typed key/value schema. It requires no protocol change: declarations
are ordinary `HookParameters` entries on the `HookDefinition` / `Hook` object.

## 1. Wire format (version 0, normative summary)

### 1.1 Reserved prefix

Declaration `HookParameterName`s begin with the four bytes `5F534900` (`_SI\x00` —
ASCII `_SI` plus version byte `0x00`). A hook adopting the interface MUST NOT use this
prefix for any parameter that does not follow the format below.

### 1.2 Field descriptor

Keys and values share one descriptor shape:

| # | bytes | content |
|---|-------|---------|
| 1 | 1 | field type: serialized `STI_*` type code |
| 2 | 1 | field name length, `0x01`–`0x10` (1–16) |
| 3 | 1–16 | field name, `[A-Za-z][A-Za-z0-9]*` |

### 1.3 `HookParameterName` (State ID + key schema)

| # | bytes | content |
|---|-------|---------|
| 1 | 3 | `5F5349` (`_SI`) |
| 2 | 1 | version `0x00` |
| 3 | 1 | State ID `0x00`–`0xFF` |
| 4 | 1 | key field count |
| 5 | var | key field descriptors, in order |

The complete name MUST fit the protocol's 32-octet `HookParameterName` limit.
State IDs MUST be unique within a hook's advertised interface (identifiers, not
positional indexes — contiguity is not required).

### 1.4 `HookParameterValue` (value schema)

| # | bytes | content |
|---|-------|---------|
| 1 | 1 | value field count (MUST be >= 1) |
| 2 | var | value field descriptors, in order |

Unlike the Parameter Signature Interface, the declaration's `HookParameterValue` is not
a `00` marker — it carries the actual value schema.

### 1.5 Supported types

Version 0 supports only fixed-width types. Integers are big-endian (protocol boundary,
same rationale as `docs/PARAM_SIGNATURE_DESIGN.md`); byte-array types are copied
verbatim.

| type byte | `STI_*` | width | Rust type token |
|-----------|---------|-------|-----------------|
| `0x10` | `STI_UINT8` | 1 | `u8` |
| `0x01` | `STI_UINT16` | 2 | `u16` |
| `0x02` | `STI_UINT32` | 4 | `u32` |
| `0x03` | `STI_UINT64` | 8 | `u64` |
| `0x04` | `STI_UINT128` | 16 | `[u8; 16]` |
| `0x05` | `STI_UINT256` | 32 | `[u8; 32]` \| `Hash` |
| `0x08` | `STI_ACCOUNT` | 20 | `AccountId` |
| `0x11` | `STI_UINT160` | 20 | `[u8; 20]` |
| `0x1A` | `STI_CURRENCY` | 20 | `CurrencyCode` |

Variable-width types (`STI_AMOUNT`, `STI_VL`, `STI_ISSUE`, …) are not part of version 0
and are rejected.

### 1.6 State key encoding

A conforming `HookStateKey` is exactly 32 octets:

```text
StateID || Encode(K0) || Encode(K1) || ... || 0x00 * (31 - len(key_payload))
```

The total encoded key-field width MUST be <= 31 octets. Zero key fields = singleton
state, key = `StateID || 31 zero bytes`.

This deliberately differs from rshooks' ordinary short-key convention (exact-length key,
host left-pads): the interface fixes the physical 32-byte layout, so rshooks builds the
full 32-byte key locally (right-zero-padded) and sends all 32 bytes.

### 1.7 State value encoding

`HookStateData` is the direct concatenation of the encoded value fields in declaration
order — no field IDs, separators, or length prefixes. All fields are fixed-width, so a
schema-aware client decodes unambiguously.

### 1.8 Validation (client view)

A version 0 interface is valid only if the complete effective `_SI\x00` set parses:
unique State IDs, counts matching descriptors, supported types only, valid unique field
names per record, key payload <= 31 octets, >= 1 value field, no trailing bytes. Any
failure invalidates the advertised interface as a whole. rshooks enforces every rule at
compile time, so a generated declaration set is always valid.

## 2. Declaration surface

State interface schemas are declared on `#[hooks]` struct fields with the
`#[state_interface(...)]` attribute:

```rust
#[hooks]
pub struct Treasury {
    #[state_interface(id = 0, key(account: AccountId, token: u32),
                      value(amount: u64, updated: u32))]
    balances: State<Balance>,

    #[state_interface(id = 1, value(paused: u8))]
    paused: State<Config>,
}
```

- `id = <int literal 0..=255>` — the State ID; required; unique across the struct's
  `#[state_interface]` fields.
- `key(name: Type, ...)` — ordered key fields; optional; omitted = singleton state.
- `value(name: Type, ...)` — ordered value fields; required, >= 1 field.
- The field type MUST be `State<VName>` where `VName` is a bare identifier: the macro
  generates `struct VName` from the `value(...)` schema (same "user names the generated
  type" pattern as the multi-type otxn view dispatch enum).
- Field names use the wire charset `[A-Za-z][A-Za-z0-9]*`, 1..=16 bytes.
- Types come from the token table in §1.5, pinned against alias drift by a
  monomorphized `const` assert on `si::SiFieldType::TYPE_BYTE` (token-level type checks
  are alias-forgeable).

The field lives in the ordinary `{Struct}State` namespace (`self.state.balances`), and
all access goes through the existing `State<V, S>` / `StateEntry<V>` accessors:

```rust
// keyed declaration: at(...) with the key fields in declared order
let entry = self.state.balances.at((account, token));
entry.set(&Balance { amount: 1000, updated: 12345 })?;
let b: Option<Balance> = entry.get()?;
entry.delete()?;

// singleton declaration (no key fields): direct accessors
let c: Option<Config> = self.state.paused.get()?;
```

`KeyArgs` is the bare field type for a single key field, a tuple in declared order for
two or more, and `()` for a singleton (which is what enables the direct accessors).

## 3. Generated code

For each `#[state_interface]` field the macro emits:

1. **The value struct** `VName` — `pub` fields mirroring the `value(...)` schema, field
   visibility following the hook-struct field's (same rule as spec markers), with
   `#[derive(Clone, Copy)]` and implementations of `ToBytes` / `FromBytes` /
   `FixedRead` mirroring the `#[derive(HookData)]` codegen shape, except every field is
   encoded via `si::SiFieldType` (big-endian ints, verbatim byte arrays) at
   macro-computed const offsets. `LEN` = sum of field widths. The BE layout is
   intentional: the value bytes are the advertised protocol-facing schema, unlike the
   LE guest-memory-image convention of ordinary state values.

2. **The spec marker** `__RshooksSpec{Struct}Field{N}{UpperCamelName}` (the same
   `marker_name` naming scheme every `#[hooks]` field kind shares — not a
   state-interface-specific prefix) implementing `StateSpec`:

   ```rust
   impl StateSpec for __RshooksSpecTreasuryField0Balances {
       type Value = Balance;
       type KeyArgs = (AccountId, u32);
       #[inline(always)]
       fn encode_key(args: &Self::KeyArgs) -> EncodedStateKey {
           let mut buf = [0u8; 32];
           buf[0] = 0u8; // State ID
           ::rshooks::si::SiFieldType::write_si(&args.0, /* offset 1, width 20 */);
           ::rshooks::si::SiFieldType::write_si(&args.1, /* offset 21, width 4 */);
           EncodedStateKey::new(buf, 32)
       }
   }
   ```

   Singleton declarations additionally get the `with_key` const override
   (`EncodedStateKey::from_short(&[ID, 0, ...; 32])`), matching the const-key
   fast path.

3. **Compile-time validation** (macro diagnostics, mirrored in the trybuild suite):
   duplicate State IDs, invalid/duplicate field names, unsupported types, key payload
   > 31 bytes, declaration name > 32 bytes (4 + 1 + 1 + Σ(2 + name_len) per §1.3),
   value schema empty or its encoded `HookParameterValue` over the protocol's value
   size limit, non-bare value type ident, `#[state_interface]` combined with another
   field attribute.

Feature off: `#[state_interface]` is recognized and rejected with a compile error
naming `unstable-state-interface` (gate-message pattern of the signature interface).

## 4. The `si` module (`crates/rshooks/src/si.rs`)

Gated `#[cfg(feature = "unstable-state-interface")]`. Contents:

- `pub trait SiFieldType`: `const TYPE_BYTE: u8`, `const WIDTH: usize`,
  `fn write_si(&self, out: &mut [u8])`, `fn read_si(bytes: &[u8]) -> Self` — the
  version-0 fixed-width codec, implemented for the §1.5 table.
- `pub const STATE_ID_PREFIX_LEN`, `pub const MAX_KEY_PAYLOAD: usize = 31`, and the
  `is_valid_name` const validator (shared shape with `sig.rs`).
- Module docs: wire-format summary, the BE rationale, the full-32-byte-key rationale
  (§1.6), and the note that declarations are advisory metadata (the protocol does not
  enforce that a hook writes what it advertises).

Declaration bytes (`HookParameterName`/`HookParameterValue` hex) are built at macro
time, not at runtime — the hook binary never materializes them.

Prelude: nothing new. `SiFieldType` stays addressable as `rshooks::si::SiFieldType`
(the generated code uses absolute paths; users only touch `State`/`StateEntry`, already
in the prelude).

## 5. Carrier and template emission

- `rshooks-chain-v2` carrier: `ChainDecls` gains `#[serde(default)] state_interface:
  Vec<SiDecl>` with `SiDecl { field, id: u8, name_hex, value_hex, key: String, value:
  String }` (`key`/`value` are the human-readable `(name: Type, ...)` display forms,
  matching `StateDecl`'s display strings; `name_hex`/`value_hex` are the exact
  declaration entry bytes).
- `sethook.template.json`: every non-gap hook entry's `HookParameters` gains one entry
  per SI declaration — `HookParameterName` = `name_hex`, `HookParameterValue` =
  `value_hex` (the real value schema, not `"00"`). State is chain-level (all entries of
  the struct share the account/namespace state), so the declarations are emitted on
  every entry, after that entry's signature-parameter declarations if any.
- Per-index sidecars carry the chain-level `state_interface` list via the existing
  `ChainSummary`/`ChainDecls` transcription.
- `rshooks-build` stays feature-free (carrier-driven); older carriers without the key
  still parse via `serde(default)`.

## 6. Feature wiring

Identical topology to `unstable-param-sig-interface`:

- `rshooks/Cargo.toml`: `unstable-state-interface =
  ["rshooks-macros/unstable-state-interface"]`; added to the docs.rs `features` list
  (never `all-features` — `host-panic-handler` must not be std-enabled).
- `rshooks-macros`: leaf feature.
- `rshooks-testenv`: forwards to `rshooks/unstable-state-interface`; its integration
  test carries `required-features`.
- CI and `mise` unstable steps run with both unstable features enabled.

## 7. Tests

- Spec test vectors pinned verbatim: declaration name
  `5F534900000208076163636F756E740205746F6B656E`, declaration value
  `020306616D6F756E74020775706461746564`, and the worked entry — key
  `004B4E9C06F24296074F7BC48F92A97916C6DC5EA90000002A00000000000000` /
  data `00000000000003E800003039` for `account =
  4B4E9C06F24296074F7BC48F92A97916C6DC5EA9, token = 42, amount = 1000,
  updated = 12345`.
- `si.rs` unit tests: per-type encode/decode round-trips, BE assertions, name
  validation edges.
- Macro unit tests: type table, name validation, declaration hex construction.
- trybuild: `tests/ui/si/{fail,pass}` for the §3 diagnostics plus
  `tests/ui/no_si_feature` for the gate, wired into the existing `ui.rs`
  feature-conditional driver.
- `rshooks-build` unit tests: carrier round-trip (incl. unknown-field rejection),
  template emission (value schema hex, all-entries emission, coexistence with
  signature-parameter declarations, no key when empty), sidecar transcription.
- testenv integration test (`required-features`): a hook writes through a keyed and a
  singleton SI declaration; the raw stored bytes are asserted equal to the spec vector
  key/value.
- Example `examples/20_state-interface` + e2e test verifying the live on-ledger
  `HookStateKey`/`HookStateData` bytes and installing the generated template
  declarations verbatim.
- All pre-existing example artifacts must stay byte-identical (feature default-off).
