# Per-Hook Attributes and the SetHook Template

A `#[hooks]` chain declares its descriptive and SetHook-facing metadata
directly on the struct and on each entry, rather than in a separate
top-level block. `rshooks build` reads these declarations and turns them
into a JSON sidecar per entry, plus one `SetHook` transaction template
covering the whole chain. This page is the full grammar for both attribute
forms and the exact shape of everything the build generates from them —
[Hook Chains](../concepts/chains.md) covers the concepts (index, shared
schema, the template's patch semantics) this page assumes.

## The struct-level attribute: `#[hooks(description = "...")]`

```rust,ignore
#[hooks(description = "20-seat L1/L2 governance and reward chain")]
pub struct Governance { /* ... */ }
```

`description` is the only argument `#[hooks]` accepts on a struct —
optional, free-form text carried into every entry's sidecar under its
`chain` object (below). There is deliberately no struct-level *name*: a
crate's identity for tooling purposes is its Cargo package name, and an
on-ledger `HookName` is a per-entry concern (below), since two entries in
the same chain can be named differently, the same, or not at all.

## The per-entry attribute: `#[hook(<index>, ...)]`

```rust,ignore
#[hook(0, name = "govern", on = [Invoke], can_emit = [Invoke, SetHook], description = "Governance state machine")]
fn govern(&self) -> HookResult { /* ... */ }
```

- **Leading positional argument, `0..=9`** — required, no default. This
  entry's index; see [Hook Chains](../concepts/chains.md#index-chain-position-not-just-an-artifact-id)
  for what it means.
- **`name = "..."`** — optional, this entry's on-ledger `HookName`. See
  "`HookName`" below for the length rule.
- **`on = ...` / `on_incoming` + `on_outgoing`** — optional; see "Trigger
  forms" below. Mutually exclusive with each other.
- **`can_emit = [Tx, ...]`** — optional; see "The three states of
  `can_emit`" below.
- **`description = "..."`** — optional, free-form text for this entry's
  sidecar (independent of the struct's own `description`).

`#[cbak(<index>)]` takes **only** the index — no other arguments, since a
callback doesn't get its own trigger or emit declaration; it settles for
whatever its paired `#[hook]` at the same index emitted.

## Transaction type names

Every entry in `on`, `on_incoming`, `on_outgoing`, and `can_emit` is a bare
[`TxType`](../reference/raw.md) variant name — `Payment`, not
`TxType::Payment` or `ttPAYMENT`. Because the macro resolves each name
against the real enum, a misspelling is a compile error, not a silent
no-op. Duplicate entries within one list are also rejected.

Names use Xahau's canonical `TransactionType` spellings, including some
that are easy to get wrong by guessing: `SetHook`, `SetRegularKey`, and
`AMMCreate`. `rshooks` maintains the authoritative list of every
valid name against the actual protocol transaction set.

## Trigger forms

Each entry chooses one of four trigger forms:

| form | wire output | meaning |
|---|---|---|
| omitted entirely | no trigger field at all | **No installation override.** For a brand-new `HookDefinition`, this means the protocol default: fires on every transaction type except `SetHook`, automatically tracking any type added to the protocol later. If this entry's wasm already has an existing `HookDefinition` on-ledger (an Update, not a fresh Install), the *existing* definition's trigger is inherited instead — so omission is not a portable guarantee of "fires on everything," only "don't say." |
| `on = all` | an explicit all-zero `HookOn` mask (every ordinary bit clear, the `SetHook` bit set so it alone doesn't fire) | **Guaranteed catch-all**, tracking future transaction types the same way omission's *new-definition* case does, but without depending on whether this is an Install or an Update. |
| `on = [Payment, Invoke, ...]` | a `HookOn` mask covering exactly the listed types | Fires only for the listed types. `on = []` is legal and means "never fires." |
| `on_incoming = [..]` + `on_outgoing = [..]` | `HookOnIncoming` + `HookOnOutgoing` (mutually exclusive with `HookOn`) | Direction-sensitive triggering (`HookOnV2`). **Must be declared as a pair** — one without the other is a build error. If both sets would end up identical, the build rejects it and asks for plain `on` instead, since that's what it means. |

Pick `on = all` when you need a guaranteed, future-proof catch-all
regardless of Install/Update history; pick omission only when "whatever
this position already has, if anything" is genuinely what you mean.

## The three states of `can_emit`

`can_emit` has the same "omitted vs. explicitly empty vs. a list"
three-state shape as `on`, and it matters just as much here — an omitted
`can_emit` is not the same as an empty one:

| declaration | wire output | meaning |
|---|---|---|
| omitted | no `HookCanEmit` field | No installation override — inherits an existing definition's emit permissions on Update, or (for a fresh Install) no restriction at all. |
| `can_emit = []` | an explicit deny-all `HookCanEmit` mask | This entry may emit **nothing**. |
| `can_emit = [Payment, ...]` | a `HookCanEmit` allowlist mask | This entry may emit only the listed types. |

`rshooks build` cross-checks each entry's declared `can_emit` against
whether its *own compiled wasm* actually calls `emit` (checked on the
final, per-index-cleaned binary, so unreachable code in a shared crate
never counts). Emitting without permission to, or declaring permission
never used, both surface as build-time warnings — never a build failure —
naming the specific mismatch. A `#[cbak]` declared for an entry that never
actually calls `emit` gets the same warning treatment, in the other
direction.

## `HookName`

`HookName` is a Rust UTF-8 string. The macro itself enforces **2 through 8
Unicode scalar values** — deliberately counting characters, not encoded
bytes. This is a separate rule from xahaud's own ledger-level requirement
that a `HookName` be **4 through 16 UTF-8 bytes**; a name intended for
direct on-chain submission needs to satisfy both. Because these two rules
can diverge for non-ASCII names, `rshooks build` checks the
byte-length rule too and prints a warning (not a hard error) when a
declared `HookName` doesn't fit it. Two entries in the same chain sharing
a `name` is legal protocol-wise, and produces an informational note rather
than a warning or error.

## How the declarations travel through the build

Both `#[hooks]` macros carry their declarations as compact JSON, hex-
encoded into the names of dead wasm exports that are never actually
called: the struct macro emits one (prefix `__rshooks_chain_v2_`, the
shared schema), and the impl macro emits another (prefix
`__rshooks_hooks_v2_`, every entry's per-index metadata). `rshooks build`
reads both from the *discovery* build's raw artifact (see [Building a
Hook](../getting-started/building.md)), then re-checks that each
per-index build's own carriers are byte-identical to discovery's, before
the ordinary hook-cleaner pass removes them along with every other
non-`hook`/`cbak` export. **These declarations are build-only and never
change any deployed binary**: they add no data segment, no runtime code,
no import, and no byte to the final wasm.

## The per-entry JSON sidecar

For `Governance`'s `govern` entry (index `0`, `on = [Invoke]`,
`can_emit = [Invoke, SetHook]`, `name = "govern"`), `rshooks build` writes
`out/current/0.govern.metadata.json`:

```json
{
  "index": 0,
  "hook_fn": "govern",
  "cbak_fn": null,
  "name": "govern",
  "description": "Governance state machine",
  "HookOn": "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF7FFFFFFFFFFFFFFFFFFBFFFFF",
  "HookCanEmit": "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF7FFFFFFFFFFFFFFFFFF9FFFFD",
  "HookName": "676F7665726E",
  "sig_params": [],
  "HookHash": "…64 hex chars…",
  "WCE": { "hook": 27751, "cbak": 0 },
  "builder": {
    "name": "rshooks-build",
    "version": "0.2.0",
    "rustc": "rustc 1.89.0 (29483883e 2025-08-04)"
  },
  "human": {
    "HookOn": ["Invoke"],
    "HookCanEmit": ["Invoke", "SetHook"],
    "HookName": "govern"
  },
  "chain": {
    "struct": "Governance",
    "description": "20-seat L1/L2 governance and reward chain",
    "decls": {
      "state": [
        { "field": "reward_rate", "kind": "const", "key": "b\"RR\"", "value": "XFL" }
      ],
      "hook_params": [],
      "otxn_params": []
    }
  }
}
```

Fields not covered already on this page:

- **`index`/`hook_fn`/`cbak_fn`** — this entry's declared position, its
  hook function's name, and its cbak function's name (`null` if this index
  declares none).
- **Top-level `HookOn`/`HookOnIncoming`/`HookOnOutgoing`/`HookCanEmit`** —
  the raw, deployable SetHook value: a 32-byte hex string encoding Xahau's
  transaction-type bitmask. `null` when the corresponding attribute was
  omitted (never for an explicitly empty `on = []`/`can_emit = []`, which
  still produce a real mask).
- **`HookHash`** — the uppercase hex of the first 32 bytes of *this
  index's own* final cleaned wasm's SHA-512 digest — identifies this one
  entry's code, independent of chain position or which account installs
  it.
- **`human`** — the readable, source-level form of every masked/hex field
  above. Use `human` to review what an entry declares; use the top-level
  fields when constructing an actual `SetHook` transaction.
- **`sig_params`** — this entry's declared signature parameters ([Hook and
  Transaction Parameters](../data/parameters.md#signature-parameters-fn-arguments)),
  in wire-index order: `null`-free array, empty for an entry with no
  signature-parameter fn arguments (as `govern` is here). Each element is
  `{ "field", "type_byte", "name_hex" }` — the argument's own identifier,
  its `STI_*` type byte, and the full declared `HookParameterName` as
  uppercase hex, the same value the generated `HookParameters` declaration
  entries below use verbatim.
- **`chain`** — this crate's **shared** schema, transcribed identically
  into every entry's sidecar (not filtered down to what this one entry
  actually uses — see [Hook Chains](../concepts/chains.md#the-shared-schema-why-this-is-the-models-biggest-win)
  for why "declared" and "used by this entry" are deliberately different
  things here). `decls` lists every `#[state]`/`#[hook_param]`/
  `#[otxn_param]` field on the struct, however many entries reference it.

## The `SetHook` template and its generation sidecar

Once every declared index has built and validated successfully,
`rshooks build` writes `sethook.template.json`:

```json
{
  "TransactionType": "SetHook",
  "Account": "<ACCOUNT>",
  "Hooks": [
    {
      "Hook": {
        "CreateCode": "…hex of 0.govern.wasm…",
        "HookOn": "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF7FFFFFFFFFFFFFFFFFFBFFFFF",
        "HookCanEmit": "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF7FFFFFFFFFFFFFFFFFF9FFFFD",
        "HookNamespace": "<NAMESPACE>",
        "HookApiVersion": 0,
        "HookName": "676F7665726E"
      }
    },
    {
      "Hook": {
        "CreateCode": "…hex of 1.reward.wasm…",
        "HookOn": "…",
        "HookCanEmit": "…",
        "HookNamespace": "<NAMESPACE>",
        "HookApiVersion": 0,
        "HookName": "7265776172D"
      }
    }
  ]
}
```

and `sethook.template.meta.json`:

```json
{
  "crate": "governance",
  "version": "0.2.0",
  "generated_at": "2026-08-18T09:00:00Z",
  "hook_hashes": { "0": "…", "1": "…" },
  "positions": { "declared": [0, 1], "gaps": [], "untouched_beyond": 2 },
  "required_amendments": ["Hooks", "NamedHooks", "HookCanEmit"]
}
```

Generation rules, precisely:

- Each declared index's object key order is fixed: `CreateCode`, then
  `HookOn` (or the `HookOnIncoming`/`HookOnOutgoing` pair — whichever this
  entry declared, omitted entirely if this entry omitted its trigger),
  `HookCanEmit` (omitted if this entry omitted `can_emit`; present,
  possibly deny-all, otherwise), `HookNamespace`, `HookApiVersion` (always
  `0` — chains are Guard-type only), `HookParameters` (only if this entry
  declares signature parameters — see below), `HookName` (only if this
  entry declared one), and `Flags` (only under `--override`, value `1`
  (`hsfOVERRIDE`), and only on declared, non-gap entries).
- A gap position is written as exactly `{"Hook": {}}` — no keys at all,
  ever, since adding any (including `Flags`) turns the no-op into a real
  operation. See [Hook Chains](../concepts/chains.md#the-sethook-template-an-owned-position-patch-not-a-full-chain)
  for what that no-op does and doesn't guarantee.
- `Account`/`HookNamespace` are the literal placeholder strings shown
  above unless `--account <r...>`/`--namespace <64hex>` were passed at
  build time.
- `HookParameters` (installed parameter values) are **never** generated for
  an ordinary `#[hook_param(...)]`/`#[otxn_param(...)]` field — a hook
  parameter's install-time value has no fixed representation in source
  (`default = ...` is a runtime fallback expression, not an encodable
  constant; see [Hook and Transaction
  Parameters](../data/parameters.md)). Add a `HookParameters` entry to the
  template by hand if a position needs one of those installed.
- The one exception: an entry with **signature-parameter fn arguments**
  (the Hook Parameter Signature Interface, [Hook and Transaction
  Parameters](../data/parameters.md#signature-parameters-fn-arguments) —
  requires the `unstable-param-sig-interface` feature)
  *does* get a generated `HookParameters` block — one *declaration* entry
  per declared argument, in index order, each with
  `HookParameterValue = "00"` (the interface's own placeholder for "this
  parameter exists, at this index, with this type" — not an installed
  value). For `examples/19_param-signature`'s `increment(account:
  AccountID, count: UInt16)`:

  ```json
  "HookParameters": [
    {
      "HookParameter": {
        "HookParameterName": "5F5F005F085F6163636F756E74",
        "HookParameterValue": "00"
      }
    },
    {
      "HookParameter": {
        "HookParameterName": "5F5F015F015F636F756E74",
        "HookParameterValue": "00"
      }
    }
  ]
  ```

  An entry with no signature-parameter arguments omits the key entirely,
  exactly like before this feature — this is additive, not a change to the
  general rule above.
- `sethook.template.meta.json` is generation provenance, **not** part of
  the transaction to submit: `hook_hashes` map index to `HookHash`;
  `positions` records which indices are declared, which are gaps within
  that range, and how far past the highest declared index the account's
  own chain is left untouched; `required_amendments` always includes
  `Hooks` and adds `NamedHooks`/`HookOnV2`/`HookCanEmit` only when the
  template actually used the corresponding feature — it does **not**
  attempt to infer amendment requirements from what the wasm's own Hook
  API calls need.

See [The `rshooks` CLI](cli.md) for the `--account`/`--namespace`/
`--override` flags themselves, and [Hook Chains](../concepts/chains.md)
for the conceptual model (owned-position patch, fail-closed by default,
generation directories) this page's JSON implements.
