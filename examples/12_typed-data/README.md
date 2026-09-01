# typed-data

## What you'll learn

How to declare a composite hook-state key/value pair and a composite Hook
API parameter name/value pair against a `#[hooks]` chain-declaration
struct's `State`/`HookParam`/`OtxnParam` fields — instead of hand-packing
each into a raw byte buffer yourself — and how to confirm the generated
code costs nothing extra at the wasm level (a real
worst-case-instruction-count measurement, not just an assertion).

Under the hood, each field's declared key/name/value type is backed by the
same four narrow, purpose-built derives: `#[derive(HookKey)]`,
`#[derive(HookData)]`, `#[derive(ParamName)]`, `#[derive(ParamValue)]`:

| role | generates | example (declared inline below) |
|---|---|---|
| hook-state **key** | `ToBytes` + `StateKeyEncode` (≤32-byte check) | `DepositKey` |
| hook-state **value** | `ToBytes` + `FromBytes` + `FixedRead` + `LEN` | `DepositValue` |
| Hook API parameter **name** | `ToBytes` (1–32-byte check) | `AdminName` |
| Hook API parameter **value** | `FromBytes` + `FixedRead` | `Config`, `Instruction`, `PauseSwitch` |

A key/name is only ever *encoded outward* (to locate something); a
value/payload is only ever *decoded* (read back) — that read/write split is
exactly why these are four separate roles rather than one covering
everything. See `docs/MULTI_HOOK_STRUCT_DESIGN.md` for the full `#[hooks]`
field-attribute grammar (`#[state(key = ..)]`/`#[state(key_by = ..)]`,
`#[hook_param(name = ..)]`/`#[hook_param(name_by = ..)]`, and their
`#[otxn_param(..)]` twin), and each underlying derive's own rustdoc
(`rshooks::{HookKey, HookData, ParamName, ParamValue}`) for the codegen
rationale and `compile_fail` examples pinning misuse.

## The hook

A per-account deposit ledger, invoked via an `Invoke` transaction. Every
invocation attaches its own instruction as a Hook parameter on the
transaction itself (`INS`, an `#[otxn_param(..)]` field) — distinct from the
hook's own installed configuration (`CFG`, an `#[hook_param(..)]` field, the
same mechanism `examples/03_hook-params` uses for a single value, here
extended to a whole struct):

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
fn main(&self) -> HookResult {
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
same code, written in the order it reads best. The parameter side has the
same shape: `self.otxn_param.instruction.get_required()`, resolving `Instruction`
from the field's own declared value type — no turbofish, no independently
inferred return type. (`config()` below reads `config` the same way, but
from a free function outside the `#[hooks] impl`, so it reaches the field
by its struct-name static instead: `TypedData.hook_param.config.get_or_default()` —
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
type). The same shape of bug existed for `otxn_param_exact`/`hook_param_exact`:
the parameter name and the value type are independent arguments, so nothing
stops decoding the `INS` parameter as `Config` by mistake.

A `#[hooks]` struct field closes both gaps the same way: its declared type
(`State<DepositValue>`, `HookParam<Config>`, `OtxnParam<Instruction>`) fixes
the value type at the field's own definition, so every accessor on it
resolves that one type — never a turbofish, never a second, independently-
chosen `T` for a mismatch to hide in. Passing the wrong value type for
`DepositKey` is a type error at the field's `.at(..)`/`.get()`/`.set()` call
sites, not a silent bug waiting to be discovered on a live node — see
`rshooks::state::TypedStateKey`'s and `rshooks::convert::TypedParamName`'s
doc comments for the full rationale (the machinery the `#[hooks]`-generated
marker types build on), and `rshooks::HookKey`'s doc comment for a
`compile_fail` example pinning the mismatch case. The typed layer costs
nothing beyond the loose functions it replaces, *for a plain-tag
parameter name* — measured at identical worst-case instructions either
way, the same as this hook's logic minus the `AdminName` pause switch covered next
(see that section for the one place this hook's cost *does* go up, and
why).

A Hook API parameter name isn't always a plain tag like `"CFG"`/`"INS"`,
either — per the Hook API itself, it's a genuine variable-length key of up
to 32 bytes, and (exactly like a hook state key) can be a whole composite,
struct-shaped value instead of a literal byte string.
`#[hook_param(name = ..)]`/`#[otxn_param(name = ..)]` cover the fixed case
(`CFG`/`INS` above); `#[hook_param(name_by = ..)]` covers a struct-shaped
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
raw byte buffers — the way every hook (including this crate's `Config`/
`Instruction`, if this feature didn't exist) had to before this feature:

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

This is a real `rshooks build`/`check` measurement, at this
workspace's `opt-level = 3` default (`examples/Cargo.toml`, `docs/
DESIGN.md`'s §2 C6), of this hook's core deposit-ledger logic (the state
key/value pairing plus the plain-tag `CFG`/`INS` parameters; not yet
counting the `AdminName` composite parameter name covered below), built
twice — once with the derives (`DepositKey`/`DepositValue`/`Config`/
`Instruction`, as committed), once with all four replaced by the
hand-packed functions above (everything else byte-for-byte identical).
The derived build measures noticeably fewer worst-case instructions and
fewer bytes than the hand-packed one; current values for the committed
hook live in [`metrics.json`](./metrics.json).

(This comparison covers `DepositKey`/`DepositValue`/`Config`/`Instruction`
only — not the `AdminName` composite parameter name, and not the
delete-on-withdrawal branch added later. See "Measured cost of a composite
name" below for what each of those adds.)

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

Both `CFG` (installed at `SetHook` time, `TypedData`'s `config` field) and
`INS` (attached to each `Invoke` transaction, `TypedData`'s `instruction`
field) decode as
`#[derive(ParamValue)]` structs, so their wire layout is exactly "every
field, in declaration order, little-endian, back-to-back"
(`rshooks::convert`'s crate-wide convention — see `Config`/
`Instruction`'s generated field-layout rustdoc).

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

`Instruction { action: u8, amount: u64 }` — **9 bytes**. For a `deposit` of
6,000,000 drops (6 XAH):

```
action        (u8):    01
amount   (u64 LE): 80 8D 5B 00 00 00 00 00
INS value hex:     01808D5B0000000000
```

Attached directly to the `Invoke` transaction's own `HookParameters` array
(not the `SetHook`'s):

```json
{
  "TransactionType": "Invoke",
  "Account": "...",
  "Destination": "...",
  "HookParameters": [
    {
      "HookParameter": {
        "HookParameterName": "494E53",
        "HookParameterValue": "01808D5B0000000000"
      }
    }
  ]
}
```

A `withdraw` needs no meaningful `amount` (it always empties the whole
balance) but the field still has to be present — 9 bytes total, e.g.
`02` + 8 zero bytes = `020000000000000000`.

## Composite (struct-shaped) parameter names: `AdminName`/`PauseSwitch`

`CFG` and `INS` above are both plain byte-string tags — the common case,
but per the Hook API itself a parameter name is really a variable-length
key of up to 32 bytes, and (exactly like a hook state key) can be a whole
composite, struct-shaped value instead of a literal string. This hook's
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
`Config`/`Instruction`): a name is only ever **written**, to locate a
value, never read back and decoded as itself — see `rshooks::ParamName`'s
doc comment for the full rationale, and its `compile_fail` examples
pinning that a `ParamName`-shaped type can't be read back as a value.
Because `AdminName` is composite (not a fixed byte string like
`CFG`/`INS` above), the field's generated `ParamSpec::with_name_bytes`
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
declaration order, back-to-back" convention, same as `Config`/
`Instruction` above): `0000`.

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

Unlike the plain `CFG`/`INS` tags (measured identical to the loose API in
the "Pairing a key with its value type" section above), a **composite**
parameter name isn't free: `TypedParamName::with_name_bytes`'s
composite-form override has to actually run `AdminName::write(..)` at
runtime (Rust has no stable way to run a trait method at compile time —
see `rshooks::convert::TypedParamName`'s doc comment). Measured by
building this exact hook twice — once as committed (with the
`AdminName`/`PauseSwitch` pause switch), once with that whole feature
removed (everything else byte-for-byte identical). The pause switch adds
a few dozen instructions and about a hundred bytes over the no-`AdminName`
baseline — the unavoidable cost of one composite-name-keyed `hook_param`
lookup (the struct encode itself, plus the extra branch/rollback path
checking it).

The delete-on-withdrawal step is a **behavior** change, not an abstraction cost: the
withdraw branch calls `deposit.delete()` and accepts from inside the
branch instead of falling through to the shared `deposit.set(&next)`,
so the hook now carries two distinct terminating state writes rather than
one. A few dozen more instructions and bytes buy the reserve refund a deleted entry
gets and an all-zero stored entry does not. (Both earlier rows were
measured before that change, on otherwise byte-identical sources; they
remain a valid A/B for the `AdminName` question they were built to answer.)

Still guard-clean at the source level throughout: no `--auto-guard`/
`--default-maxiter` needed for any of the three.

## Build

```sh
cargo run -p rshooks-build -- build --manifest-path examples/12_typed-data/Cargo.toml --out examples/12_typed-data/out
```

No extra flags — see "Zero-cost: measured, not assumed" above.

## Expected behavior

- No `INS` parameter (or the wrong size) on the `Invoke` → rollback
  (`"typed-data: INS parameter missing or malformed"`, code `2`).
- `deposit` below the configured (or default) minimum → rollback
  (`"typed-data: deposit below configured minimum"`, code `4`).
- `deposit` at or above the minimum → accept; the account's stored
  `DepositValue.amount` increases by the deposited amount and the lock
  window resets.
- `withdraw` with no outstanding deposit → rollback
  (`"typed-data: nothing to withdraw"`, code `5`).
- `withdraw` before the lock window elapses → rollback
  (`"typed-data: deposit still locked"`, code `6`).
- `withdraw` after the lock window elapses → accept; the account's
  `DepositValue` entry is **deleted** from hook state (not zeroed in
  place), refunding its owner reserve. A subsequent read finds nothing and
  decodes as `EMPTY_DEPOSIT`, so the next `withdraw` rolls back with
  `nothing to withdraw`.
- `action` anything other than `1`/`2` → rollback
  (`"typed-data: unknown INS action"`, code `3`).

## Error codes

`TypedDataError` (`rshooks::hook_errors!`, see `src/lib.rs`) is the
`rollback!` code for each failure this hook can exit with:

| variant | code | meaning |
|---|---|---|
| `AccountFieldMissing` | 1 | the originating transaction has no `sfAccount` field (unreachable in practice) |
| `InstructionMissing` | 2 | the `INS` Hook parameter is missing, or not exactly 9 bytes |
| `UnknownAction` | 3 | `Instruction::action` is neither `1` (deposit) nor `2` (withdraw) |
| `BelowMinimum` | 4 | a `deposit` instruction's amount fell below the configured minimum |
| `NothingToWithdraw` | 5 | a `withdraw` instruction, but the account has no outstanding deposit |
| `StillLocked` | 6 | a `withdraw` instruction, but the deposit's lock window hasn't elapsed yet |
| `StateReadFailed` | 7 | reading the account's `DepositValue` failed with something other than "no entry" |
| `StateSetFailed` | 8 | writing the updated `DepositValue` back failed |
