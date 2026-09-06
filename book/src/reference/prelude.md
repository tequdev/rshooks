# The Prelude

`use rshooks::prelude::*;` is the standard way to bring `rshooks`'s
ergonomic surface into scope. Nearly every code sample in this book starts
with it (usually paired with `use rshooks::*;` for the macros, which live at
the crate root rather than in `prelude`). This page lists exactly what it
re-exports, grouped by area, and the two things it deliberately leaves out.

The prelude is deliberately *not* a full glob of `rshooks-core` — the raw
crate's `api::*` functions share names with `rshooks`'s own wrappers (both
define `state`, for instance), so pulling in the whole thing would create
ambiguity. Only the constant-only modules come through from the raw layer;
everything else is `rshooks`'s own typed surface.

## API wrapper modules

Every function from these `rshooks::api` submodules, **except** the numbered
slot functions (see "Deliberate absences" below):

| module | covers |
|---|---|
| `api::control` | `accept`/`rollback` (the functions `accept!`/`rollback!` call into). |
| `api::etxn` | Transaction emission: `etxn_reserve`, `etxn_fee_base`, `etxn_details`, `emit`. |
| `api::float` | XFL arithmetic host calls (`float_multiply`, `float_divide`, `mulratio`, ...). |
| `api::hook_ctx` | This hook's own context: `hook_account`, `hook_param`, `hook_param_typed`, `hook_param_exact`, `hook_param_set`, and related. |
| `api::keylet` | The 26 typed `keylet_xxx` helpers (one per `KEYLET_*` constant) plus their 26 `keylet_xxx_into` out-param twins. |
| `api::ledger` | Ledger-wide queries: `ledger_seq`, `ledger_last_hash`, `ledger_last_time`, and related. |
| `api::otxn` (partial) | `otxn_field`, `otxn_field_exact`, `otxn_field_typed`, `otxn_field_u64`, `otxn_param`, `otxn_param_exact`, `otxn_param_typed`, `otxn_type`, `otxn_id`, `otxn_id_buf`, `otxn_burden`, `otxn_generation`, `OtxnFieldValue` — listed by name rather than globbed, so a future addition upstream is a deliberate act. `otxn_slot` is excluded (a slot function). |
| `api::state` | Typed single-value state helpers (`state_u32`, `state_xfl`, `state_update_u64`, ...) alongside the composite layer below. |
| `api::sto` | STObject parsing helpers. |
| `api::trace` | The support functions backing `trace!`/`trace_num!`/`trace_float!`. |
| `api::util` | `util_keylet`, `util_accid`, `util_raddr`, `util_verify`, `util_sha512h`, and related. |

See [Reading the Originating Transaction](../data/otxn.md), [Hook
State](../data/state.md), [Hook and Transaction
Parameters](../data/parameters.md), [Emitting Transactions](../emit/emitting.md),
and [Keylets](../data/keylets.md).

## The typed slot layer

`slot_obj::{AmountBytes, CastTarget, IssueData, SlotKey, SlotObject}` — the
handle-based wrapper around the Hook API's 255 numbered slot registers,
addressed by type instead of by raw integer. See [Slots and Ledger
Objects](../data/slots.md).

## `sfield` constants

`crate::sfield::*` — the typed `SField<T>` constants (`sfAccount`,
`sfSequence`, ...), one per serialized field, each carrying its value type
as a generic parameter. These are what `SlotObject::get` and the typed
otxn/state accessors take. See [Slots and Ledger Objects](../data/slots.md).

## `buf_eq` helpers

`crate::buf_eq::*` — `buf_eq_8`/`_20`/`_32`/`_33`/`_34`/`_40`/`_48`/`_64`:
fixed-size buffer equality as straight-line word-compare code, avoiding the
compiler-generated `bcmp`-style loop a plain `==` on a `[u8; N]` can lower to
at `opt-level = "z"`. The same module also has `buf_cmp_20`, a loop-free
160-bit big-endian ordering for two 20-byte buffers (e.g. two `AccountId`s)
— XRPL/Xahau's "high"/"low" account ordering, used to canonicalize a pair of
accounts (picking the low/high side of a `RippleState` trustline keylet).
See [Guards and Loops](../concepts/guards.md).

## `no_unroll`

`crate::no_unroll` (re-exported at the crate root too) — routes a loop's
induction variable through `core::hint::black_box` at its comparison, so
LLVM can no longer prove the trip count at compile time and keeps the loop
as one real `loop` construct instead of fully unrolling it. Only worth
reaching for when a small, provably-bounded *outer* loop wrapping a
`guard!`-protected *inner* loop would otherwise get fully unrolled at
`opt-level = 3` — unrolling physically duplicates the inner loop, and the
guard checker (which walks compiled bytecode) then counts its worst-case
cost once per duplicate instead of once total. `while no_unroll(i) < N { ..
}` in place of `while i < N { .. }`.

## Convert traits

`crate::convert::{FixedRead, FromBytes, ToBytes, TypedParamName}` — the
traits `#[derive(HookData)]`/`#[derive(HookKey)]`/`#[derive(ParamName)]`/
`#[derive(ParamValue)]` implement, and the trait a `#[hook_param(...)]`/
`#[otxn_param(...)]` field's name carries. See [Typed Data with
Derives](../data/typed-data.md).

## The `decl` module: struct field handle types

`crate::decl::{State, HookParam, OtxnParam}` — re-exported at the crate
root *and* here in the prelude, since these are the field types a
`#[hooks]` struct's `#[state]`/`#[hook_param]`/`#[otxn_param]` fields are
written with (see [Hook State](../data/state.md) and [Hook and Transaction
Parameters](../data/parameters.md)). The rest of `decl` — `StateEntry`,
`HookParamAt`, `OtxnParamAt` (the handles `.at(args)` returns for a
keyed/named-family field) and the `*Spec`/`HookChainEntries` traits — is
the macro-generated side of the handshake, not re-exported here or at the
crate root; reach it at `rshooks::decl::StateEntry` etc. when a field's own
`.at(args)` return type needs naming (e.g. in a helper function's
signature).

## `HookError`/`Result`

`crate::error::{HookError, Result}` — the `Result<T, HookError>` alias every
typed Hook API wrapper returns, including `HookError::NotImplemented` (what
a raw call returns on a host build). See [Accept, Rollback, and
Errors](../concepts/errors.md).

## `Accept`/`Rollback`/`HookResult`

`crate::exit::{Accept, Rollback, HookResult}` — the typed entry-return
types: `HookResult` is `Result<Accept, Rollback>`, the only return type a
`#[hook]`/`#[cbak]` entry may declare (`-> i64` does not compile). The sealed
`EntryReturn` conversion trait those types compile through is not in the
prelude (or nameable at all outside its fully qualified path) — a hook
author never calls it directly. See ["Typed entry returns:
`HookResult`"](../concepts/errors.md#typed-entry-returns-hookresult).

## State functions

`crate::state::{StateKeyEncode, TypedStateKey, state_delete, state_foreign_get,
state_foreign_get_typed, state_foreign_set_loose, state_foreign_set_typed,
state_foreign_update_loose, state_foreign_update_typed, state_get,
state_get_typed, state_set_loose, state_set_typed, state_update_loose,
state_update_typed}` — the composite/typed hook-state layer a `#[state(...)]`
struct field's accessors forward to, plus the `_foreign` twins for reading
another account's state. See [Hook State](../data/state.md).

## `HookStatic`

`crate::static_cell::HookStatic` — the safe, `const`-constructible,
take-once cell for templates and large buffers that should land in a wasm
data segment/BSS rather than be materialized by runtime stores. See [Anatomy
of a Hook](../concepts/anatomy.md).

## Typed read views

`crate::views::ledger::LedgerEntryCommonFields` and
`crate::views::tx::{TransactionCommonFields, TransactionCommonSlotFields}`
— the common-field traits every generated view (`views::tx::Payment`,
`views::ledger::AccountRoot`, ...) implements, covering the fields every
transaction (`sfAccount`, `sfFee`, `sfMemos`, ...) or every ledger entry
(`sfLedgerEntryType`, `sfFlags`, ...) carries. Importing the prelude is
enough to call these common accessors on any view; the view types
themselves (one per transaction/ledger-entry format) live under
`rshooks::views::{tx, ledger}` and are not globbed into the prelude —
`use rshooks::views::tx::Payment;` explicitly. See `rshooks::views`'s own
module doc comment for the full model (originating-transaction vs.
slot-backed sources, `soeREQUIRED`/`soeOPTIONAL` field typing, the
`active-amendments`/`all-amendments` format tiers).

## `StoWriter`

`crate::sto_writer::StoWriter` — the bounded, allocation-free writer for a
runtime-sized `STObject`/`STArray` (a transaction whose shape
`txn_template!` can't describe at compile time, e.g. Remit's `sfAmounts`).
See [Emitting Transactions](../emit/emitting.md) and
[The `StoWriter` API](../emit/sto-writer.md).

## `LedgerEntryType`

`crate::ledger_entry_type::LedgerEntryType` — the typed ledger-entry-type
enum (`LedgerEntryType::AccountRoot`, ...), decoded from a ledger object's
`sfLedgerEntryType` the same way `TxType` decodes a transaction's type.

## Hook Parameter Signature Interface (`sig`), behind `unstable-param-sig-interface`

`crate::sig::{Blob, IssueBytes, SigName, SigParamType, hook_sig_param,
otxn_sig_param, otxn_sig_param_opt}` — re-exported in the prelude only when
the **unstable** `unstable-param-sig-interface` cargo feature is enabled on
`rshooks`; the `sig` module itself doesn't exist in the crate at all
otherwise. This is the support layer behind `#[hook(..)]`'s signature-
parameter fn arguments — see [Hook and Transaction
Parameters](../data/parameters.md#signature-parameters-fn-arguments). (The
module also has a `hook_sig_param_opt` function, the `hook_param` twin of
`otxn_sig_param_opt`; it is not re-exported in the prelude, so reach it at
`rshooks::sig::hook_sig_param_opt` if needed.)

## `TxType`

`crate::tx_type::TxType` — the typed transaction-type enum (`TxType::Payment`,
...), used by `otxn_type` and by `#[hook(<index>, ...)]`'s `on`/`can_emit`
lists. See [Reading the Originating Transaction](../data/otxn.md).

## Types

`crate::types::*` — protocol value newtypes: `AccountId`, `Hash`, `Keylet`,
`StateKey`, `NameSpace`, `Nonce`, `PublicKey`, `CurrencyCode`,
`IssuedAsset` (a `CurrencyCode` + issuing `AccountId` pair), the `STObject`/
`STArray`/`Amount`/`Issue`/`Opaque` marker types `SField<T>` and
`StoWriter`'s field writers take, and the length constants (`ACC_ID_LEN`,
`STATE_KEY_LEN`, `EMIT_DETAILS_MAX_LEN`, ...).

## XFL / `XFLUnchecked`

`crate::xfl::XFL` and `crate::xfl_unchecked::XFLUnchecked` — the checked and
hot-path-unchecked decimal floating-point types. See [XFL: Decimal Floating
Point](../data/xfl.md).

## The `XFL!` macro

`rshooks_macros::XFL` — re-exported here too, alongside the `xfl::XFL`
*type* of the same name. This is not a naming collision: a macro and a type
live in separate Rust namespaces, the same relationship `std::Clone` (trait)
and `#[derive(Clone)]` (macro) have.

## Constant families

`rshooks_core::{consts::*, lets::*, ls_flags::*, tts::*, tx_flags::*}` — the
C-verbatim constant tables: `KEYLET_*`/`COMPARE_*` (`consts`), `ltXxx`
ledger-entry-type codes, `lsfXxx` ledger-entry flags, `ttXxx`
transaction-type codes, and `tfXxx`/`asfXxx` transaction/account flags. See
[The Raw Layer](raw.md) for the full family list, including the ones not
re-exported here (`sfcodes`, `error`, `backend`).

## Two deliberate absences

- **The raw `sfcodes` glob.** `sfield`'s typed `SField<T>` constants take
  those same names, so `sfSequence` in the prelude is an `SField<u32>`, not
  a bare `u32`. The raw table is still available at
  `rshooks::raw::sfcodes::*` for const contexts where `Into` cannot be
  called — `txn_template!`'s field tables, or a `const` header expression.
  `SField::code()` is the other bridge between the two.
- **The numbered slot functions.** `slot_set`/`slot_clear`/`slot_subfield`/
  `otxn_slot`/... address the same 255 registers `SlotObject` manages, and
  mixing the two silently corrupts handles. They stay public at
  `rshooks::api::slot::*` (plus `rshooks::api::otxn::otxn_slot`) — reaching
  for them explicitly at least makes the escape hatch visible at the call
  site.

Workarounds: `rshooks::raw::sfcodes::*` for raw sfield codes,
`rshooks::api::slot::*` for the numbered slot API. See [The Raw Layer](raw.md)
and [Slots and Ledger Objects](../data/slots.md).
