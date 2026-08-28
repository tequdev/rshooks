# typed-data

## What you'll learn

How to declare a composite hook-state key/value pair and a composite Hook
API parameter name/value pair against a `#[hooks]` chain-declaration
struct's `State`/`HookParam` fields — instead of hand-packing each into a
raw byte buffer yourself — and how to confirm the generated code costs
nothing extra at the wasm level (a real worst-case-instruction-count
measurement, not just an assertion). Alongside that struct-field surface,
this hook's own per-invocation instruction (`action`/`amount`) is declared
the *other* way `rshooks` supports a Hook parameter: as plain entry-fn
arguments on `main` itself, per the Hook Parameter Signature Interface
(`docs/PARAM_SIGNATURE_DESIGN.md`) — see "Signature parameters:
`action`/`amount`" below for how the two surfaces differ.

Under the hood, each struct field's declared key/name/value type is backed
by the same four narrow, purpose-built derives: `#[derive(HookKey)]`,
`#[derive(HookData)]`, `#[derive(ParamName)]`, `#[derive(ParamValue)]`:

| role | generates | example (declared inline below) |
|---|---|---|
| hook-state **key** | `ToBytes` + `StateKeyEncode` (≤32-byte check) | `DepositKey` |
| hook-state **value** | `ToBytes` + `FromBytes` + `FixedRead` + `LEN` | `DepositValue` |
| Hook API parameter **name** | `ToBytes` (1–32-byte check) | `AdminName` |
| Hook API parameter **value** | `FromBytes` + `FixedRead` | `Config`, `PauseSwitch` |

A key/name is only ever *encoded outward* (to locate something); a
value/payload is only ever *decoded* (read back) — that read/write split is
exactly why these are four separate roles rather than one covering
everything. See `docs/MULTI_HOOK_STRUCT_DESIGN.md` for the full `#[hooks]`
field-attribute grammar (`#[state(key = ..)]`/`#[state(key_by = ..)]`,
`#[hook_param(name = ..)]`/`#[hook_param(name_by = ..)]`), and each
underlying derive's own rustdoc (`rshooks::{HookKey, HookData, ParamName,
ParamValue}`) for the codegen rationale and `compile_fail` examples pinning
misuse. `action`/`amount`'s own decode is generated differently again —
see "Signature parameters" below, and
`crates/rshooks/src/sig.rs`/`docs/PARAM_SIGNATURE_DESIGN.md` for that
surface's own design.

## The hook

A per-account deposit ledger, invoked via an `Invoke` transaction. Every
invocation attaches its own instruction as two signature parameters on the
transaction itself (`action`/`amount`, declared as `main`'s own extra
arguments — see "Signature parameters" below) — distinct from the hook's
own installed configuration (`CFG`, an `#[hook_param(..)]` field, the same
mechanism `examples/03_hook-params` uses for a single value, here extended
to a whole struct):

- `deposit` (`action = 1`): rejects (rolls back) if the deposited amount is
  below the configured minimum; otherwise adds it to the sender's balance
  and (re)starts a lock window ending `lock_ledgers` ledgers from now.
- `withdraw` (`action = 2`): rejects if the sender has no outstanding
  deposit, or if the lock window hasn't elapsed yet; otherwise **deletes**
  the sender's record, refunding the owner reserve it was holding.

Each sender's record is looked up by a **composite key** — a tag byte plus
their `AccountId` — and stored as a **composite value** — an amount, a
deadline ledger sequence, and a flags byte. `TypedData`'s `deposits` field
declares the key type via `#[state(key_by = DepositKey)]`, keyed per call
site; `DepositKey`/`DepositValue` are ordinary `#[derive(HookKey)]`/
`#[derive(HookData)]` structs declared alongside it:

```rust
#[derive(HookKey, Clone, Copy)]
struct DepositKey {
    tag: u8,
    owner: AccountId,
}

#[derive(HookData, Clone, Copy)]
struct DepositValue {
    amount: u64,
    deadline: u32,
    flags: u8,
}

#[hooks]
pub struct TypedData {
    #[state(key_by = DepositKey)]
    deposits: State<DepositValue>,
    // ...
}
```

`deposits` is the **field** — the thing this hook operates on, and what
carries the accessors once bound to a key via `.at(..)`. `main` declares a
`&self` receiver, so it reaches its own chain's fields as `self.state.deposits` —
no manual byte packing anywhere:

```rust
#[hook(0, on = [Invoke])]
fn main(&self, action: u8, amount: u64) -> HookResult {
    let deposit = self.state.deposits.at(DepositKey { tag: DEPOSIT_TAG, owner });
    let current = match deposit.get() {
        Ok(existing) => existing.unwrap_or(EMPTY_DEPOSIT),
        Err(_) => rollback!(
            b"typed-data: state read failed",
            TypedDataError::StateReadFailed
        ),
    };
    // ...
    if deposit.set(&next).is_err() {
        rollback!(
            b"typed-data: state_set failed",
            TypedDataError::StateSetFailed
        );
    }
}
```

`.get()`/`.set()`/`.delete()` (and `.update()`, unused here) are inherent
methods on the bound `StateEntry` `.at(..)` returns, each an
`#[inline(always)]` forward to `state::state_get`/`state_set_loose` — the
same code, written in the order it reads best. `action`/`amount` need no
accessor call at all inside the body — they're ordinary `u8`/`u64` locals
by the time `main` starts, decoded by the `#[hooks]`-generated prologue
before the body runs (see "Signature parameters" below). (`config()` below
reads `config` the same way `deposits`/`admin_pause` do, but from a free
function outside the `#[hooks] impl`, so it reaches the field by its
struct-name static instead: `TypedData.hook_param.config.get_or_default()` —
see "`self` vs. the struct-name static" below.)

### `self` vs. the struct-name static

`&self` and `TypedData` name the exact same value — the single, zero-sized
instance the `#[hooks]` struct macro generates as `static TypedData:
TypedData`. An entry or helper declared *inside* the `#[hooks] impl` gets
that instance handed to it as `&self` and writes `self.state.deposits`; code
*outside* the impl — `config()`/`deposits_paused()` below are free
functions, not impl members — has no `self` to borrow, so it names the
same static directly: `TypedData.hook_param.config`. Both forms are permanently legal
and measure byte-identical wasm (a reference to a zero-sized value
optimizes away entirely); this crate's examples use `&self` inside the
annotated impl and the struct-name static everywhere else.

## Pairing a key/name with its value type

`state_get`/`state_set_loose` take the key and the value type as two
*independent* generic parameters — nothing stops calling
`state_get::<SomeOtherValue>(&key)` for a `key`/`SomeOtherValue` combination
that was never meant to go together, as long as `SomeOtherValue: FromBytes`
(true of nearly every fixed-size type, including some *other* key's value
type). The same shape of bug existed for the loose `hook_param_exact`: the
parameter name and the value type are independent arguments, so nothing
stops decoding `CFG` as `PauseSwitch` by mistake. `action`/`amount` don't
have this bug either, for a different reason: an entry-fn signature
argument's name and type are the *same* declaration (`action: u8`), not two
independently-chosen values that could drift apart — see "Signature
parameters" below.

A `#[hooks]` struct field closes the loose-accessor gap the same way: its
declared type (`State<DepositValue>`, `HookParam<Config>`) fixes the value
type at the field's own definition, so every accessor on it resolves that
one type — never a turbofish, never a second, independently-chosen `T` for
a mismatch to hide in. Passing the wrong value type for `DepositKey` is a
type error at the field's `.at(..)`/`.get()`/`.set()` call sites, not a
silent bug waiting to be discovered on a live node — see
`rshooks::state::TypedStateKey`'s and `rshooks::convert::TypedParamName`'s
doc comments for the full rationale (the machinery the `#[hooks]`-generated
marker types build on), and `rshooks::HookKey`'s doc comment for a
`compile_fail` example pinning the mismatch case. The typed layer costs
nothing beyond the loose functions it replaces, *for a plain-tag
parameter name* — measured at 441 worst-case instructions either way, the
same as this hook's logic minus the `AdminName` pause switch covered next
(see that section for the one place this hook's cost *does* go up, and
why).

A Hook API parameter name isn't always a plain tag like `"CFG"`,
either — per the Hook API itself, it's a genuine variable-length key of up
to 32 bytes, and (exactly like a hook state key) can be a whole composite,
struct-shaped value instead of a literal byte string.
`#[hook_param(name = ..)]` covers the fixed case (`CFG` above);
`#[hook_param(name_by = ..)]` covers a struct-shaped
name constructed per call site (`AdminName` below, via
`TypedData.hook_param.admin_pause.at(ADMIN_PAUSE_NAME)`) — both read through the exact
same `get()`/`get_or_default()`/`get_required()` path. Only a *plain,
already-known-at-compile-time* name (`name = b"CFG"`) is free, though —
its generated `ParamSpec::with_name_bytes` hands over the already-`'static`
literal bytes directly, at zero runtime cost. A composite name
(`name_by = AdminName`) can't skip encoding — something has to lay its
fields out — so its generated override encodes into a
`[u8; AdminName::MAX_LEN]` buffer, sized to exactly that name and no more.
That is still cheaper than the trait's *generic* default body, which has
no way to spell `Self::MAX_LEN` as an array length and falls back to a full
32-byte `PARAM_NAME_MAX_LEN` scratch; what it cannot avoid is the encode
itself, since Rust has no stable way to run a trait method at compile
time. See `rshooks::convert::TypedParamName`'s doc comment for the full
zero-cost rationale, and the "Composite parameter names" section below for
this hook's own worked composite-name example and its measured cost.

## Before/after: what `#[derive(HookKey)]`/`#[derive(HookData)]` replace

Without them, `DepositKey`/`DepositValue` would have to be hand-packed into
raw byte buffers — the way every hook (including this crate's `Config`, if
this feature didn't exist) had to before this feature:

```rust
// Key: tag (1 byte) || owner (20 bytes) - 21 bytes total, sent to the
// host exactly as-is: the host itself left-pads a key shorter than its
// fixed 32-byte storage width (see `rshooks::state`'s module doc
// comment, "Key length and padding") - no local zero-padding here.
fn make_key(owner: &AccountId) -> [u8; 21] {
    let mut out = [0u8; 21];
    if let Some(b) = out.get_mut(0) {
        *b = DEPOSIT_TAG;
    }
    if let Some(dst) = out.get_mut(1..21) {
        dst.copy_from_slice(owner.as_ref());
    }
    out
}

// Value: amount (8 bytes LE) || deadline (4 bytes LE) || flags (1 byte).
fn encode_value(v: &DepositValue) -> [u8; 13] {
    let mut out = [0u8; 13];
    if let Some(dst) = out.get_mut(0..8) {
        dst.copy_from_slice(&v.amount.to_le_bytes());
    }
    if let Some(dst) = out.get_mut(8..12) {
        dst.copy_from_slice(&v.deadline.to_le_bytes());
    }
    if let Some(b) = out.get_mut(12) {
        *b = v.flags;
    }
    out
}

fn decode_value(buf: &[u8; 13]) -> Option<DepositValue> {
    let amount = buf.get(0..8)?;
    let deadline = buf.get(8..12)?;
    let flags = *buf.get(12)?;
    // ... four more lines assembling `u64`/`u32` from LE bytes ...
}
```

— every field's offset counted by hand, every reader kept in sync with
every writer by hand, and every one of those `.get()`/`.get_mut()` calls
(required by this crate's `indexing_slicing` deny — see `docs/DESIGN.md`
§2/§8) repeated per field. `#[derive(HookData)]` generates the equivalent
of the above (the same fixed, compile-time offsets, the same
`.get_mut()`-guarded fixed-size copies) once, from the struct definition
itself, and keeps `ToBytes`/`FromBytes` in sync automatically as fields are
added, removed, or reordered.

## Zero-cost: measured, not assumed

`docs/DESIGN.md` and this crate's own doc comments repeatedly warn that a
"clean-looking" abstraction can silently introduce an unguarded loop
(`memcpy`/`memset`/`bcmp` lowering — see `examples/06_guard-patterns` and
the root README's "`--auto-guard`" section). `#[derive(HookKey)]`/
`#[derive(HookData)]`/`#[derive(ParamValue)]` avoid that by construction
(every offset is a compile-time constant, every per-field copy an inlined
`ToBytes::write`/`FromBytes::read` call — see `HookData`'s doc comment's
"Zero-cost by construction" section, which all three derives share) — but
the only way to *prove* that is to build both versions through
`rshooks-build` and compare `rshooks check`'s reported worst-case
instruction count.

This table is a real `rshooks build`/`check` measurement, at this
workspace's `opt-level = 3` default (`examples/Cargo.toml`, `docs/
DESIGN.md`'s §2 C6), of this hook's core deposit-ledger logic (the state
key/value pairing plus the plain-tag `CFG` parameter; not yet counting the
`AdminName` composite parameter name or the `action`/`amount` signature
parameters, covered below), built twice — once with the derives
(`DepositKey`/`DepositValue`/`Config`, as committed), once with all three
replaced by the hand-packed functions above (everything else byte-for-byte
identical). Predates this hook's conversion to `action`/`amount` signature
parameters (`docs/PARAM_SIGNATURE_DESIGN.md`) — the derived-vs-hand-packed
*delta* this table exists to show is unaffected by that conversion, since
`action`/`amount` decode identically (via the `#[hooks]`-generated
prologue) in both the derived and hand-packed variants:

| version | worst-case instructions | wasm size |
|---|---|---|
| derived (this crate, as committed) | 441 | 1504 bytes |
| hand-packed (`.get()`/`.get_mut()` per field, as most hooks write it today) | 525 | 1674 bytes |

(This table covers `DepositKey`/`DepositValue`/`Config` only — not the
`AdminName` composite parameter name, not the `action`/`amount` signature
parameters, and not the delete-on-withdrawal branch added later. See
"Measured cost of a composite name" below for the full hook's numbers, and
"Build" below for this hook's current, as-committed WCE/size.)

The derive isn't just *as cheap as* hand-packing here — it measures
**cheaper**: the generated `write`/`read` check the struct's total length
**once** (`buf.get_mut(..Self::MAX_LEN)`), then copy every field through
already-proven-in-bounds fixed offsets, whereas the naive hand-packed
version above re-checks bounds with a separate `.get()`/`.get_mut()` call
per field — extra branches the derive's single up-front check avoids. (A
hand-written version that also front-loads one length check could match
the derive's number; the point isn't that hand-packing can't be made this
cheap, it's that the derive *always* generates that shape, by construction,
without a hook author having to discover and apply the trick themselves.)

No `--auto-guard`/`--default-maxiter` flags are needed for either version —
`rshooks check` reports both as guard-clean at the source level (see
`examples/README.md`'s "On `--auto-guard`" section for what that means and
why it's the idiom this crate prefers).

## Hook parameter hex encoding

`CFG` (installed at `SetHook` time, `TypedData`'s `config` field) decodes as
a `#[derive(ParamValue)]` struct, so its wire layout is exactly "every
field, in declaration order, little-endian, back-to-back"
(`rshooks::convert`'s crate-wide convention — see `Config`'s generated
field-layout rustdoc).

`Config { min_amount: u64, lock_ledgers: u32 }` — **12 bytes**. For
`min_amount = 5,000,000` drops (5 XAH) and `lock_ledgers = 20`:

```
min_amount  (u64 LE): 40 4B 4C 00 00 00 00 00
lock_ledgers(u32 LE): 14 00 00 00
CFG value hex:        404B4C000000000014000000
```

```json
{
  "HookParameter": {
    "HookParameterName": "434647",
    "HookParameterValue": "404B4C000000000014000000"
  }
}
```

(`HookParameterName` is `CFG` in ASCII hex.) Omitting `CFG` entirely falls
back to the compiled-in default (1 XAH minimum, a 10-ledger lock).

## Signature parameters: `action`/`amount`

`action`/`amount` are declared *directly on `main`'s own signature* —
`fn main(&self, action: u8, amount: u64)` — instead of a hand-rolled
`#[otxn_param(..)]` struct like `Config` above. This is the Hook Parameter
Signature Interface (`docs/PARAM_SIGNATURE_DESIGN.md`): each extra argument
after `&self` on a `#[hook(..)]` fn becomes one declared, typed,
machine-readable parameter, and the `#[hooks]`-generated prologue decodes
both before `main`'s body ever runs.

Unlike `Config`'s wire layout above (this crate's own little-endian
`ParamValue` convention), a signature parameter's value is decoded
**big-endian** — it crosses the same protocol boundary a raw
`otxn_field`/`otxn_param` read does (see `crates/rshooks/src/sig.rs`'s
module doc comment, "Why big-endian"). Each parameter's `HookParameterName`
is a fixed 7..=22-byte wire encoding — `0x5F 0x5F | index | 0x5F |
type_byte | 0x5F | name` — resolved by the macro from the argument's own
position and type, never written out by hand:

| arg | index | type | `STI_*` byte | name | `HookParameterName` (hex) |
|---|---|---|---|---|---|
| `action` | 0 | `u8` | `0x10` (`STI_UINT8`) | `action` | `5F5F005F105F616374696F6E` |
| `amount` | 1 | `u64` | `0x03` (`STI_UINT64`) | `amount` | `5F5F015F035F616D6F756E74` |

For a `deposit` of 6,000,000 drops (6 XAH), attached directly to the
`Invoke` transaction's own `HookParameters` array (not the `SetHook`'s):

```json
{
  "TransactionType": "Invoke",
  "Account": "...",
  "Destination": "...",
  "HookParameters": [
    {
      "HookParameter": {
        "HookParameterName": "5F5F005F105F616374696F6E",
        "HookParameterValue": "01"
      }
    },
    {
      "HookParameter": {
        "HookParameterName": "5F5F015F035F616D6F756E74",
        "HookParameterValue": "00000000005B8D80"
      }
    }
  ]
}
```

`amount`'s value (`00000000005B8D80`) is `6,000,000` as 8 big-endian bytes
— contrast `Config`'s `min_amount` above, the same numeric type but
little-endian, because it's this crate's own `ParamValue` wire format
rather than a signature-parameter value. A `withdraw` needs no meaningful
`amount` (it always empties the whole balance) but the parameter still has
to be present, since every declared signature parameter is required — e.g.
`0000000000000000` (all-zero) works.

`action` and `amount` are also declared, not just invoked: the `#[hook(..)]`
fn gets one `HookParameters` block in the generated `sethook.template.json`,
with one declaration entry per signature parameter (`HookParameterValue =
"00"`) — see "Build" below.

### The low-level escape hatch

`main`'s generated prologue is built on the same `crates/rshooks/src/sig.rs`
primitives available directly, for a hand-rolled read outside the `#[hooks]`
fn-argument surface:

```rust,ignore
use rshooks::sig::otxn_sig_param;
use rshooks::sig_name;

// Exactly the declared name `main`'s prologue builds for `amount` (index 1,
// `u64`, `STI_UINT64`) — usable directly with `otxn_param_exact` or, as
// here, `otxn_sig_param`, which also does the big-endian decode.
const AMOUNT_NAME: [u8; 12] = sig_name!(1, u64, b"amount");

let amount: rshooks::error::Result<u64> = otxn_sig_param(&AMOUNT_NAME);
```

See `crates/rshooks-testenv/tests/sig_params.rs` for the same primitives
driven end-to-end through `TestEnv::invoke`.

## Composite (struct-shaped) parameter names: `AdminName`/`PauseSwitch`

`CFG` above is a plain byte-string tag — the common case, but per the Hook
API itself a parameter name is really a variable-length key of up to 32
bytes, and (exactly like a hook state key) can be a whole composite,
struct-shaped value instead of a literal string. This hook's
operator-controlled pause switch is named that way:

```rust
#[derive(ParamName, Clone, Copy)]
struct AdminName {
    section: u8,
    field: u8,
}

#[derive(ParamValue)]
struct PauseSwitch {
    paused: u8,
}

const ADMIN_PAUSE_NAME: AdminName = AdminName { section: 0, field: 0 };

#[hooks]
pub struct TypedData {
    // ...
    #[hook_param(name_by = AdminName, default = PauseSwitch { paused: 0 })]
    admin_pause: HookParam<PauseSwitch>,
}
```

`TypedData.hook_param.admin_pause` is a **keyed** `HookParam` field: `name_by =
AdminName` means each call site binds its own name value via `.at(..)`
(here always the one fixed `ADMIN_PAUSE_NAME`, since this name scheme is
meant to accommodate *multiple* future administrative parameters, not just
one canonical instance) — `TypedData.hook_param.admin_pause.at(ADMIN_PAUSE_NAME)
.get_or_default()`, `PauseSwitch`'s type inferred from the field's own
declared value type, no annotation.

`AdminName` gets `ParamName`-equivalent codegen (`ToBytes` only — no
`FromBytes`, no `FixedRead`, no inherent `LEN` const), never
`HookData`-equivalent codegen: a Hook parameter *name* is a genuinely
different concept from a hook-state key/value or a parameter *payload*
(`PauseSwitch`, which — being something this hook actually reads back and
decodes — gets `ParamValue`-equivalent codegen instead, same as
`Config`): a name is only ever **written**, to locate a
value, never read back and decoded as itself — see `rshooks::ParamName`'s
doc comment for the full rationale, and its `compile_fail` examples
pinning that a `ParamName`-shaped type can't be read back as a value.
Because `AdminName` is composite (not a fixed byte string like
`CFG` above), the field's generated `ParamSpec::with_name_bytes`
override does a genuine encode into a `[u8; AdminName::MAX_LEN]` (2-byte)
buffer, rather than the fixed forms' zero-copy hand-off of a `'static`
literal — see the "Measured cost of a composite name" section below for
what that costs. (It is still an override: the trait's generic *default*
body would use a full 32-byte scratch buffer, since generic code cannot
spell `Self::MAX_LEN` as an array length.)

### The 1–32-byte constraint

A Hook API parameter name must be **1 to 32 bytes** (`hook_api.h`:
`TOO_SMALL` below 1, `TOO_BIG` above 32 — see
`rshooks::convert::PARAM_NAME_MAX_LEN`). `AdminName` encodes to
`section` (1 byte) + `field` (1 byte) = **2 bytes**, comfortably inside
that range. Unlike an oversized `HookData` struct (which has no such bound
at all — a state *value* has no fixed size cap), `#[derive(ParamName)]`
checks this bound **unconditionally, at the struct's own definition** — a
`ParamName` struct that encoded to, say, 40 bytes would fail to compile
right there, before anything tried to use it as a parameter name at all
(the same derive-time-check idea `#[derive(HookKey)]` applies to a
33+-byte state key, just with an added *lower* bound a key doesn't have).
See `rshooks::ParamName`'s doc comment for the `compile_fail` example
pinning exactly that case.

### Hex encoding

`AdminName { section: 0, field: 0 }` — **2 bytes** (`section`, then
`field`, no padding — `rshooks::convert`'s crate-wide "every field, in
declaration order, back-to-back" convention, same as `Config` above):
`0000`.

`PauseSwitch { paused: 1 }` — **1 byte**: `01`.

Installed at `SetHook` time (an administrative control, not something a
depositor sets per transaction — hence `hook_param`, not `otxn_param`):

```json
{
  "HookParameter": {
    "HookParameterName": "0000",
    "HookParameterValue": "01"
  }
}
```

Omitting this `HookParameter` entirely (or setting `HookParameterValue`
to `00`) leaves deposits unpaused — `deposits_paused()` treats "absent, or
the wrong size" the same as `paused == 0`.

### Measured cost of a composite name

Unlike the plain `CFG` tag (measured identical to the loose API in
the "Pairing a key with its value type" section above), a **composite**
parameter name isn't free: `TypedParamName::with_name_bytes`'s
composite-form override has to actually run `AdminName::write(..)` at
runtime (Rust has no stable way to run a trait method at compile time —
see `rshooks::convert::TypedParamName`'s doc comment). Measured by
building this exact hook multiple times, each with one more piece added
(everything else byte-for-byte identical):

| version | worst-case instructions | wasm size |
|---|---|---|
| without the `AdminName` pause switch | 441 | 1504 bytes |
| with the `AdminName` pause switch | 470 | 1611 bytes |
| + deleting the record on a full withdrawal | 504 | 1686 bytes |

+29 instructions, +107 bytes for `AdminName` over the no-`AdminName`
baseline — the unavoidable cost of one composite-name-keyed `hook_param`
lookup (the struct encode itself, plus the extra branch/rollback path
checking it).

The third row is a **behavior** change, not an abstraction cost: the
withdraw branch calls `deposit.delete()` and accepts from inside the
branch instead of falling through to the shared `deposit.set(&next)`,
so the hook now carries two distinct terminating state writes rather than
one. +34 instructions, +74 bytes buys the reserve refund a deleted entry
gets and an all-zero stored entry does not. (Both earlier rows were
measured before that change, on otherwise byte-identical sources; they
remain a valid A/B for the `AdminName` question they were built to answer.)

Still guard-clean at the source level throughout: no `--auto-guard`/
`--default-maxiter` needed for any of the three.

### Before/after: the `action`/`amount` signature-parameter conversion

Per docs/TODO.md's standing rule for any change touching the generated
prologue (probe numbers, before vs. after, through the real `rshooks
build`/`check` pipeline): this hook, built once with the old `INS`
`Instruction` `#[otxn_param(..)]` struct (the same source as this table's
own third row above; a fresh rebuild of it against today's crates measures
1686 bytes, one byte over that row's older figure), once as committed with
`action`/`amount` as signature parameters instead — everything else
byte-for-byte identical:

| version | worst-case instructions | wasm size | max nesting depth |
|---|---|---|---|
| before: `INS` `Instruction` (`#[otxn_param(name = b"INS", required)]`, one 9-byte struct read) | 504 | 1686 bytes | 14 |
| after: `action`/`amount` signature parameters (as committed) | 560 | 1841 bytes | 15 |

+56 instructions, +155 bytes, +1 nesting level — the generated
decode-and-rollback prologue for *two* independently-typed, independently
BE-decoded arguments costs more than the single hand-rolled 9-byte
`Instruction` struct read it replaces (one `otxn_param_exact` call, one
length check, one LE-decode of two packed fields). The single extra
nesting level (14 → 15) comes from the second argument's own
`match { Ok(..) => .., Err(_) => rollback!(..) }` arm; well within the
32-level guard-checker budget either way. Not a like-for-like efficiency
comparison of the interface itself — see `examples/18_param-signature` for
a hook with *no* prior otxn-param struct to compare against, and
`examples/16_typed-results` for a case where converting cost *nothing* —
its `deposit` entry's WCE moved 307 → 306 (one instruction *lower*)
replacing a `read_amount` helper and an `#[otxn_param]` field with the
generated prologue.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/12_typed-data/Cargo.toml --out examples/12_typed-data/out
```

No extra flags — see "Zero-cost: measured, not assumed" above. Current
`rshooks check` numbers for this hook as committed (state key/value
pairing, `CFG`/`AdminName` Hook parameters, `action`/`amount` signature
parameters, and the delete-on-withdrawal branch, all together): worst-case
instructions `560`, size `1841` bytes, max nesting depth `15`.

## Expected behavior

- The `action` or `amount` signature parameter is missing, or the wrong
  size, on the `Invoke` → rollback (`"rshooks: bad sig param 'action'"` or
  `"rshooks: bad sig param 'amount'"`, code `0`/`1` — see "Signature
  parameters" above; `main`'s body never runs).
- `deposit` below the configured (or default) minimum → rollback
  (`"typed-data: deposit below configured minimum"`, code `18`).
- `deposit` at or above the minimum → accept; the account's stored
  `DepositValue.amount` increases by the deposited amount and the lock
  window resets.
- `withdraw` with no outstanding deposit → rollback
  (`"typed-data: nothing to withdraw"`, code `19`).
- `withdraw` before the lock window elapses → rollback
  (`"typed-data: deposit still locked"`, code `20`).
- `withdraw` after the lock window elapses → accept; the account's
  `DepositValue` entry is **deleted** from hook state (not zeroed in
  place), refunding its owner reserve. A subsequent read finds nothing and
  decodes as `EMPTY_DEPOSIT`, so the next `withdraw` rolls back with
  `nothing to withdraw`.
- `action` anything other than `1`/`2` → rollback
  (`"typed-data: unknown action"`, code `17`).

## Error codes

`TypedDataError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback!` code for each failure this hook's own body can exit with — a
missing/malformed `action`/`amount` signature parameter rolls back earlier,
from the generated prologue, with its own message/code (`"rshooks: bad sig
param '<name>'"`, code = the argument's index, `0` or `1` — see "Signature
parameters" above), not one of these. Every variant here is numbered from
`16` rather than `1`, precisely so it can never collide with a signature
parameter's own index-as-code (`0x00..=0x0F` — see
`book/src/data/parameters.md`'s "the `>= 16` convention" for the rule this
follows):

| variant | code | meaning |
|---|---|---|
| `AccountFieldMissing` | 16 | the originating transaction has no `sfAccount` field (unreachable in practice) |
| `UnknownAction` | 17 | `action` is neither `1` (deposit) nor `2` (withdraw) |
| `BelowMinimum` | 18 | a `deposit` instruction's amount fell below the configured minimum |
| `NothingToWithdraw` | 19 | a `withdraw` instruction, but the account has no outstanding deposit |
| `StillLocked` | 20 | a `withdraw` instruction, but the deposit's lock window hasn't elapsed yet |
| `StateReadFailed` | 21 | reading the account's `DepositValue` failed with something other than "no entry" |
| `StateSetFailed` | 22 | writing the updated `DepositValue` back failed |
| `DepositsPaused` | 23 | a `deposit` instruction, but the `AdminName` pause switch is currently set |
