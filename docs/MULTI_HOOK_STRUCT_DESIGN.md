# Multi-Hook Struct API Design Document (v0.2.0)

Status: design draft (no implementation yet — design only)

Target: rshooks v0.2.0 (breaking changes permitted)

Last updated: 2026-08-18

Revision history:

- r1: initial version
- r2: incorporated all 26 findings from Codex (gpt-5.6-sol) review round 1
- r3: incorporated all 24 findings from Codex review round 2
- r4: incorporated all 17 findings from Codex review round 3 (final).
  **Resolved the critical handle-representation flaw by "injecting a marker
  type argument into the field type"** (§5.4). Fleshed out the bidirectional
  handshake (§5.1), the accepted item-shape table (§5.1), the trigger-omission
  inheritance caveat and the addition of `on = all` (§5.3), the canonical
  absence-detection rule (§5.6), BuildPlan's cargo-metadata resolution and
  dedicated target directory (§7.1), the plain cargo/rustdoc contract
  (§7.6), the Phase reallocation (§12–§13), uppercase hex normalization
  (§9), and more.
- r5: revised to accept a `&self` receiver on entry functions (§5.7). See
  [HOOKS_SELF_RECEIVER_DESIGN.md](./HOOKS_SELF_RECEIVER_DESIGN.md) for
  details.
- r6: entries now require `&self`; see
  [HOOKS_SELF_RECEIVER_DESIGN.md](./HOOKS_SELF_RECEIVER_DESIGN.md).

## 0. Origin of the proposal (historical record)

> **Note**: what follows is a record of the earliest idea sketch. Syntax
> details that appear here — argument names, the `cbak` name argument, etc.
> — are not this design's finalized syntax. **See §8 for the normative
> skeleton** (the attribute grammar's full BNF and accessor names are
> finalized in Phase 1).

Retire the top-level `#[hook]` / `#[cbak]` definitions and move them into a
struct + impl block.

```rust
pub struct Hook {
    // state definitions
    // hook_param definitions
    // otxn_param definitions
}

impl Hook {
    #[hook(0, name = "func1", onincoming = [Payment], canemit = [])]
    // metadata can also be defined here
    pub fn func1(...) {}

    #[hook(1, "func1")]
    pub fn func2(...) {}

    #[cbak(0, "func1")]
    pub fn cbak(...) {}
}
```

Building this generates a wasm for each hook/cbak pair corresponding to an
index, along with a `SetHook` transaction template for installation. `name`
represents the on-ledger `HookName`.

## 1. Settled points

| # | Question | Decision |
|---|---|---|
| 1 | Unit of definition | struct + impl becomes the first-class citizen for "1 chain = 1 crate." The top-level `#[hook]` is retired and unified into this in v0.2 |
| 2 | Meaning of index | **The chain position (the position in the SetHook `Hooks` array) is specified directly in the source.** 0..=9, gaps allowed |
| 3 | index notation | The attribute's **leading positional argument** (`#[hook(0, ...)]`). No named form is provided. Not omittable even for a single hook |
| 4 | hook/cbak pairing | `cbak` is paired **only by index**, e.g. `#[cbak(0)]`. No re-specifying `name` |
| 5 | Standalone definitions | An index with only a hook is fine. **An index with only a `cbak` is a compile error.** A struct with zero hooks is an error |
| 6 | Outer macro name | `#[hooks]` (attached to both the struct and the impl) |
| 7 | Build strategy | Adopt **Approach A (discovery + per-index `--cfg` recompilation)** first. Approach B (compile once + split the wasm) is the ideal form and will replace A as a future optimization (§7) |
| 8 | Gas Hook (HookApiVersion 1) | **Out of scope.** v0.2 covers only the Guard type (api_version 0) |
| 9 | Relationship to the manifest | The manifest (docs/spec.md etc.) is still under discussion, so **this design does not take it into account** |
| 10 | Parameter defaults | The attribute's `default` is **a runtime fallback expression only**. Embedding the installed value into the SetHook template is not done in v0.2 (§5.6, §9.2). **Scoped exception:** the Hook Parameter Signature Interface's declared signature parameters (extra `#[hook(..)]` fn arguments, `docs/PARAM_SIGNATURE_DESIGN.md` §1) are a distinct declaration mechanism from `#[hook_param]`/`#[otxn_param]` fields and their `default` — for those, `HookParameters` declaration entries (`HookParameterValue = "00"`) ARE emitted (§9.2, realizing §10 D5). Ordinary `#[hook_param]`/`#[otxn_param]` fields are still never emitted |
| 11 | Template semantics | A SetHook template is an **owned-position patch**, not a declarative realization of the whole chain. Default is fail-closed (no override) (§9) |
| 12 | Trigger omission | Omitting the trigger entirely is legal and means "**do not place an installation override**" (for a new HookDefinition, the protocol default fires on every type except `SetHook`; reusing an existing definition inherits its value). **Guaranteed all-type firing is written explicitly as `on = all`** (§5.3) |
| 13 | Descriptive names | No descriptive `name` on the struct attribute (crate identity comes from the Cargo package name). The entry attribute's `name` is reserved for the on-ledger `HookName` |
| 14 | Generated-artifact metadata | The template JSON stays protocol-shaped; generation info goes into a **separate sidecar** (`sethook.template.meta.json`) (§9.2) |
| 15 | Handle representation | Field types are rewritten by the macro to **inject a field-specific marker type as a second type argument** (`State<V>` → `State<V, __Marker>`). The sole exception to the span contract (§5.4) |
| 16 | Generated hex | Hex in templates/sidecars (HookName / CreateCode / masks / namespace / hash) is normalized to **uppercase** |

## 2. Background: the current structure, and the problem this proposal solves

### 2.1 Current state (v0.0.x)

- 1 crate = 1 hook. Attaching `#[hook]` / `#[cbak]` to a free function
  generates an `export_name = "hook"` / `"cbak"` wrapper.
- HookOn / HookCanEmit / HookName / name / description are declared via
  `metadata!`, **separately** from the entry point. Triggers accept three
  forms: symmetric (`HookOn`), directional (`IncomingHookOn` +
  `OutgoingHookOn`), and full omission.
- state / parameters are declared with `hook_state!` / `hook_parameter!` /
  `otxn_parameter!`.
- The build is cargo (wasm32v1-none) → rshooks-build post-processing
  (cleaner / flatten / unnest / guard / validator), producing a single wasm.
- Assembling the SetHook transaction is the user's own responsibility.

### 2.2 Current pain points

1. **A chain (multiple hooks) has no project-level representation.**
   Related hook groups on the same account, like `80_reward` and
   `81_govern`, end up as separate crates and **duplicate** the state layout
   they share (seat/member keys, `V*` voting keys). Duplicated declarations
   silently drift apart.
2. **Metadata is separated from the entry point.**
3. **The hook/cbak pairing is implicit.**
4. **Chain position isn't managed.** Which position in the `Hooks` array a
   given hook occupies appears in neither the source nor the build
   artifacts.
5. **Deployment is manual.** Assembling the SetHook JSON is left to the user.

This proposal resolves 1 through 5 together. Point 4 in particular is a
design decision that writing the index into the source **makes the chain's
occupied-position layout a code-review artifact**.

## 3. What gets better

### 3.1 A single shared declaration of the ABI (the biggest win)

A group of hooks sharing the same namespace/account can **share a single
declaration** of their state and parameter types. Duplicated key layouts
disappear, and the compiler guarantees that state producers and consumers —
e.g. "`govern` writes what `reward` reads" — reference the same type.

This is not merely a convenience; it's a safety improvement. A state-layout
mismatch currently only shows up at runtime, corrupting on-ledger data.
Today the only safeguard is manual review.

Note: what's shared is the **Rust-level type/layout declaration (the
schema)**. The actual value of an installed parameter is independent per
hook entry (index) on the ledger; sharing the declaration does not mean
sharing the value (end of §5.4).

### 3.2 Codifying the occupied-position layout

Because index directly represents chain position, "which hook this project
places at which position" appears in the source code, subject to diff,
review, and history tracking. Order-sensitive designs — how a preceding
hook's accept/rollback affects a later one, or handing values from a
preceding hook to a later one via `hook_param_set` — can be expressed as
code rather than a deployment runbook.

(The source only declares the positions it owns; it does not declaratively
specify the entire chain on the account. §9.3)

### 3.3 Localized metadata

With `on_incoming` / `can_emit` / `name` attached directly to the entry
function:

- Reading the function tells you its trigger conditions (better
  reviewability)
- The implicit knowledge of mapping `metadata!` to an entry point disappears
- Adding/removing a function and adding/removing its metadata are now
  syntactically linked

### 3.4 Explicit, verified hook/cbak pairs

Pairing by index lets duplicate-index and missing-pairing issues be caught
at compile time, and the consistency between emit capability and `cbak` be
verified at build time. **The normative definition of the hook/cbak/emit
validation items, their severity, and their execution phase lives in the
table in §6.2 and the truth table in §6.3** (§6.2 also covers other
syntax/shape diagnostics).

### 3.5 Automatic generation of deployment artifacts

Looking at the whole struct statically reveals "which positions this crate
occupies and each hook's configuration," so a single build command can
produce:

- a wasm per index (each independently fitting the 64 KiB limit)
- a metadata sidecar per index
- a SetHook transaction **template** for the occupied positions (§9) plus a
  generation-info sidecar

The template holds `Account` / `HookNamespace` as placeholders and is a
**draft meant to be edited before submission**, not a finished product ready
to submit as-is (§9.3). Even so, eliminating the manual work of assembling
masks, `CreateCode`, and positions is a substantial DX win.

### 3.6 A sound wasm-size strategy

Because code is shared while artifacts are split, common helpers (XFL
arithmetic, etc.) get duplicated into each wasm, but **each wasm only
contains code reachable from its own entry point**. Since the 65,535-byte
limit applies per hook, the chain as a whole effectively gets a 10x larger
budget. This is clearly better than packing everything into one wasm.

## 4. What gets worse, and the risks

### 4.1 A worse experience for the simple case (the most important risk)

Examples 01–15 today are all single-hook, with a minimal example as short as:

```rust
metadata! { name: "accept-all" }

#[hook]
fn my_hook() -> i64 { accept!(b"ok", 0) }
```

The new form's minimal shape (§8.4) adds a struct declaration, an impl
block, and a required index — **a net 4–6 lines and two new concepts (the
struct container, the index)**. Since trigger omission stays legal (§5.3),
the minimal form doesn't need `on`, but it's fair to admit the first
impression of a beginner tutorial gets somewhat worse.

Requiring an explicit index even for a single hook avoids the asymmetry of
"only writing the `0` once you add a second hook" — uniformity was
prioritized over first-touch cost, a tradeoff the book will state explicitly
in its opening chapter.

The option of "keeping the top-level `#[hook]` as sugar" was not taken,
since two coexisting definition styles would duplicate both teaching
material and implementation. v0.2 unifies on one form.

### 4.2 Relocating a chain position becomes a source change

Since index equals chain position, re-deploying the same group of hooks at a
different position layout means editing the index in the source and
rebuilding.

This is intentional (§3.2). Reusing hooks at a different position can also
be handled by hand-reordering the `Hooks` array in the generated SetHook
template (the wasm itself is position-independent). The template is "the
default that matches the source declaration" and doesn't prevent final
pre-submission edits.

### 4.3 The semantic lie of the "struct + impl" shape

Rust's struct/impl exists for instances and their methods. A hook entry is a
static export with no instance, and `&self` cannot exist. The struct's
fields also hold no real data (ZST markers). This syntax is effectively
**borrowing struct as a namespace**, and can mislead developers who bring
OO-style expectations ("state in self") to it.

This isn't fatal — there's plenty of precedent (wasm_bindgen, pymethods,
etc.) — but the docs need to state explicitly that "the struct is a
container for the chain declaration, with no runtime instance."

### 4.4 Macro complexity and IDE experience

An outer macro that rewrites the entire impl block is a step up in
implementation/maintenance cost compared to a per-method attribute. To
mitigate this, the following are **implementation requirements**:

- **Span-preservation contract** (§6.1): user-written method bodies,
  signatures, doc comments, and unrelated attributes are re-emitted verbatim
  with their spans preserved; only the consumed helper attributes are
  stripped. No reconstruction from stringification. The **sole exception**
  is the `#[hooks]` struct's field types, which are rewritten to inject the
  marker type argument (§5.4) — even there, tokens the user wrote (such as
  the value-type argument) are reused with their spans preserved.
- **Diagnostic catalog** (a Phase 1 deliverable): common mistakes — a
  missing outer `#[hooks]` on one side, leftover current-generation (v0.0.x)
  macros, a missing directional trigger side, direction-mask mismatches,
  combining `required` with `default`, `#[cfg]` on an entry, `self` on an
  entry, unsupported struct/impl shapes (§5.1) — each get their error
  message, primary span, and help text specified, and pinned down with
  trybuild UI tests.
- A rust-analyzer completion/goto-definition smoke test is included as a
  Phase 1 completion criterion.

### 4.5 "Fieldifying" state/param declarations is less natural than it looks

`hook_state!(Counter, CounterKey {...} => u64)` bundles a key literal, key
shape, and value type into one declaration. Turning this into a field
requires supplementing information the field type alone can't express (the
key's literal value, the parameter name's byte string) via a **field
attribute**. Reason: since `&'static str`/byte-slice const generics aren't
stable, type-level embedding like `HookParam<"CFG", Config>` isn't possible.
The attribute's contents get folded down into a field-specific marker type,
as in §5.4.

In other words, fieldifying still results in a two-part "type + attribute"
declaration carrying the same amount of information as the current macros.
What's gained is locality and overall visibility, not less to write.

### 4.6 Build-pipeline complexity

Going from 1 crate to N wasm files breaks out of cargo's "1 cdylib = 1
artifact" model. Fixing BuildPlan (§7.1), per-index processing order
(§7.2), artifact generation management (§7.4), and the plain-cargo contract
(§7.6) as specifications bounds this risk.

### 4.7 Migration cost

All 15 examples, plus the book, e2e tests, and templates, all need
rewriting. This is within the acceptable range for a v0.2.0 breaking
change, but should be budgeted at roughly the same order of effort as the
macro implementation itself. **The canonical migration procedure and
acceptance criteria are defined in §12 (migration plan).**

## 5. Finalized semantics

### 5.1 Struct unit, placement, and accepted shapes

- **Exactly one Hook struct per crate** (v0.2).
- An impl block carrying `#[hooks]` must be **exactly one per struct**.
  Ordinary, unattributed impl blocks (for helpers) may coexist freely, and
  placing an ordinary associated function without a helper attribute inside
  the annotated impl is **also allowed** (the macro passes it through
  unchanged).
- **Accepted item shapes** (v0.2; anything else is rejected with a
  dedicated diagnostic):

  | Target | Accepted | Rejected |
  |---|---|---|
  | struct | non-generic unit struct (`struct X;`) / named-field struct (including empty `{}`) | tuple struct; generics, lifetimes, or a `where` clause |
  | struct field | a field with **exactly one** declaration attribute (`#[state]`/`#[hook_param]`/`#[otxn_param]`) | an unattributed field; multiple declaration attributes |
  | impl | a non-generic inherent impl (whose `Self` type is the bare struct name) | trait impl; generic impl; qualified `Self` type |
  | item inside impl | associated functions (entries/helpers), associated constants | associated types |

  The book will note explicitly that migrating a unit struct to a
  named-field struct is "just replacing the `;` with a field block."
- **Bidirectional struct/impl handshake**:
  - The struct macro generates (a) a per-field marker type and handle static
    (§5.4/§5.5), (b) `impl Vault { #[doc(hidden)] pub const __RSHOOKS_STRUCT: () = (); }`,
    and (c) an assertion equivalent to
    `const _: () = { fn assert<T: HookChainImpl>() {} let _ = assert::<Vault>; };`
    (requiring the impl side's trait implementation).
  - The impl macro generates (a) code that references
    `Self::__RSHOOKS_STRUCT` (**requiring the struct side to be annotated**
    — attaching an annotated impl to a bare struct produces an "undefined
    associated constant" error), (b) an implementation of the
    `#[doc(hidden)]` internal trait `HookChainImpl` (the target the struct
    side's assertion requires), and (c)
    `impl Vault { #[doc(hidden)] pub const __RSHOOKS_IMPL: () = (); }` (two
    annotated impls reliably collide with a **duplicate associated constant
    error**, without relying on trait impl's E0119).
  - The internal trait/constants are `#[doc(hidden)]` internal API, and
    hand-implementing or hand-defining them to spoof the handshake is
    documented as **unsupported (undefined behavior)**. This is not a
    defense mechanism against malicious code.
- **"1 crate 1 struct" is enforced via the linker**: the struct macro emits
  a fixed-name `#[unsafe(no_mangle)]` symbol (wasm target only,
  `#[doc(hidden)]`), so two structs produce a duplicate-symbol link error.
  Since discovery never runs once linking fails, no further diagnostic
  quality is provided beyond that (the error message's origin is documented
  in the book's troubleshooting section).
- The `#[hooks]` struct and impl are required to live **in the same
  module**. The generated value binding's (§5.5) visibility follows the
  struct's visibility; each field handle's visibility follows the field's
  visibility.
- The struct name is free-form; crate identity uses the Cargo.toml package
  name/version. The only struct attribute is `#[hooks(description = "...")]`
  (settled point #13).
- `#[cfg]` / `#[cfg_attr]` on entry methods or declared fields are
  **forbidden in v0.2** (the macro errors on them).

### 5.2 Meaning of index and name

- `index` is a **unique integer in 0..=9** that simultaneously means two
  things:
  1. The identifier of the artifact (hook/cbak pair) this crate produces
  2. The position in the generated SetHook template's `Hooks` array (i.e.
     the chain position)
- The notation is the attribute's **leading positional argument**:
  `#[hook(0, ...)]` / `#[cbak(0)]`.
- Gaps (e.g. only 0 and 2) are **allowed**. An empty template position
  becomes `{"Hook": {}}` (a position-preserving no-op). The `Hooks` array is
  generated with a length of **exactly 0..=the highest declared index**,
  with no trailing extra entries (§9.2).
- `name` is the on-ledger `HookName` (the NamedHooks amendment) and is
  **optional**. Omitting it means an unnamed hook. Length rules follow the
  protocol's normative spec (note: there has been a history of confusion
  between the current `metadata!`'s authoring rule of "2..=8 Unicode
  scalars" and a proposed byte-length-based rule. **Phase 1 settles length
  validation against the vendored xahaud implementation as the normative
  source** and retires the independent authoring rule). Sharing the same
  `name` across multiple indices is protocol-legal and therefore permitted,
  but the build emits an info diagnostic.
- **`cbak` is paired only by index**, e.g. `#[cbak(0)]`. A hook-only index
  is fine. **A cbak-only index is an error.** At most one `cbak` per index.
- **A struct with zero hooks is a compile error** (§5.1 handshake).

### 5.3 Metadata declarable via the per-hook attribute

| Argument | Corresponds to | Required |
|---|---|---|
| leading positional `0..=9` | index (artifact ID / chain position) | Yes |
| `name = "..."` | `HookName` | No |
| `on = all` / `on = [Tx, ...]` | symmetric trigger | No (see below) |
| `on_incoming = [..]` / `on_outgoing = [..]` | directional trigger. **Must always be paired.** If both directions' sets coincide, use `on` instead (error) | No (see below) |
| `can_emit = [Tx, ...]` | `HookCanEmit` (three-valued, see below) | No |
| `description = "..."` | for the sidecar | No |

**Trigger declaration forms** (the current `metadata!`'s three forms, plus
an explicit catch-all):

| Declaration | Wire output | Meaning |
|---|---|---|
| fully omitted | no trigger field emitted | **Do not place an installation override.** For a new HookDefinition, the protocol default applies (fires on every type except SetHook, tracking future types automatically). **If a HookDefinition for the same wasm already exists, this is treated as an Install and inherits that definition's trigger**, so omission is not a catch-all guarantee |
| `on = all` | `HookOn`'s all-zero mask (only the SetHook bit doesn't fire) | **A guaranteed catch-all.** Tracks future added types automatically (expressed as a mask, not an enumeration) |
| `on = [..]` | `HookOn` (64-hex mask) | Fires only for the listed types. `on = []` means "fires for no type" |
| `on_incoming` + `on_outgoing` | `HookOnIncoming` + `HookOnOutgoing` (each a 64-hex mask, mutually exclusive with `HookOn`) | HookOnV2's directional firing |

"Fires for every type" must never be reproduced by enumerating type names
(it wouldn't track future type additions). The book will clearly document
the distinction between omission (inheritance possible) and `on = all`
(guaranteed).

**`can_emit`'s three-valued semantics**:

| Declaration | Wire meaning |
|---|---|
| omitted | no `HookCanEmit` field emitted = **do not place an installation override.** For a new HookDefinition, no restriction applies (every type including SetHook can be emitted). Reusing an existing definition inherits its value. **This is not a "guaranteed no restriction"** |
| `can_emit = []` | installs a deny-all mask (**deny-all**) |
| `can_emit = [Payment]` | installs an allowlist mask permitting only the listed types |

Naming is snake_case (`on_incoming` / `can_emit`). `HookApiVersion` is fixed
at 0, with no attribute argument (Gas Hook is out of scope).

**Amendment dependencies**: `name` requires NamedHooks, directional triggers
require HookOnV2, and a present `can_emit` requires HookCanEmit. How the
derived set is handled is covered in §9.2.

### 5.4 Struct field declaration form and handle representation

Fields are declared as "a marker-carrying ZST type + attribute."

```rust
#[hooks(description = "Deposit vault with sweep")]
pub struct Vault {
    /// Per-account deposit balance.
    #[state(key(prefix = b"B", field(account: AccountId)))]
    deposits: State<DepositValue>,

    /// The cap the operator sets at SetHook time.
    #[hook_param(name = b"CFG", default = Config { max: xfl!(1000), lock: 10 })]
    config: HookParam<Config>,

    /// The instruction the calling transaction specifies.
    #[otxn_param(name = b"INS", required)]
    instruction: OtxnParam<Instruction>,
}
```

**Handle representation (settled point #15)**: the field type the user
writes, `State<DepositValue>` / `HookParam<Config>`, is **declaration
sugar**. If two fields share the same value type (e.g. two
`State<DepositValue>` fields), the type alone can't distinguish which
receiver is which, and the attribute's key/name can't be bound to method
dispatch. So the struct macro:

1. Generates a field-specific marker ZST for each field (e.g.
   `__VaultFieldDeposits`), giving it the key/name spec derived from the
   attribute (literal, shape, encoding) as an implementation of an internal
   trait (roughly `KeySpec` / `NameSpec`)
2. Rewrites the field type to **inject the marker as a second type
   argument**, e.g. `State<DepositValue, __VaultFieldDeposits>` (the
   tokens for the user-written first type argument are reused with their
   spans preserved)

This gives each field a unique receiving type, and accessors statically
resolve the key/name from the marker's trait implementation. This field-type
rewrite was already called out in §4.4 as the **sole exception** to the
span-preservation contract.

- The generated get/set semantics, `FromBytes`/`ToBytes` (prefix/exact
  decode), and key-encoding semantics are **unchanged from the current
  implementation**. This changes where the declaration lives, not the byte
  ABI.
- The existing `hook_state!` / `hook_parameter!` / `otxn_parameter!` are
  removed in v0.2. Internally, the shape parser and key encoder logic is
  called from the field-attribute parser instead.
- **Field-attribute grammar completeness**: the example above is
  representative; the actual grammar must be able to represent, 1:1, **every
  declaration form the current three macros accept** (literal keys —
  utf8/hex/bytes —, composite key shapes, existing type references, pairing
  forms, composite parameter-name patterns — every form used in examples
  12/81). Phase 1 produces a **canonical migration table** ("current
  declaration → field attribute," §12.1) whose mechanical rewritability is
  the acceptance criterion for grammar completeness. The attribute grammar's
  BNF is finalized in Phase 1.
- **What's shared is the schema, not the value**: the struct declaration
  only guarantees that every hook in the chain uses the same layout. The
  actual value of an installed parameter is set/inherited/cleared
  independently per hook entry (index) on the ledger. For instance,
  installing `config` with `max=1000` on index 0 and `max=50` on index 1 is
  perfectly valid. The per-index sidecar carries every declaration, but this
  is "a transcription of the shared schema," not "the list of declarations
  that hook actually uses" (§10 D2).

### 5.5 Generated value bindings and lint contract

- For a **struct with named fields**, the macro generates
  `static Vault: Vault` (a static with the same name as the struct), and
  it's accessed as a value, e.g. `Vault.deposits.get(&acct)`. Type names and
  value names live in separate namespaces, so there's no collision. Since
  every field is a ZST that is `Sync` and const-constructible, the static's
  requirements are trivially satisfied.
- A **unit struct** has no fields, so **no static is generated** (it would
  collide with the unit constructor of the same name, `E0428`). An empty
  named-field struct (`struct X {}`) may generate a static, but since
  there's nothing to access, the two are observationally identical either
  way.
- **Lint contract**: generated code must pass the repository's standard
  `-D warnings` build cleanly. A scoped `#[allow(non_upper_case_globals)]`
  is applied to lowercase statics, and the minimum necessary
  `#[allow(dead_code)]` to artifacts that might go unused. Every internal
  item the macro generates (wrappers, markers, handshake constants,
  carrier) is `#[doc(hidden)]`. Building the entire examples tree under
  `-D warnings` in CI serves as the acceptance test for this contract.

### 5.6 Parameter presence and accessors

`required` and `default` are mutually exclusive. Accessors **distinguish
absence from bad data**:

| Declaration | Accessor added (names are provisional, finalized in Phase 1) | Meaning |
|---|---|---|
| (always) | `get() -> Result<Option<T>, Error>` | absence = `Ok(None)`. **A decode failure or host error is `Err`** |
| `default = <expr>` | `get_or_default() -> Result<T, Error>` | the declared expression's value only on absence. A decode failure is still `Err` |
| `required` | `get_required() -> Result<T, Error>` | absence is also `Err` (a dedicated absence error variant) |

- **Canonical absence-detection rule**: "absence" is determined **solely by
  the pre-decode host API return value** (`DOESNT_EXIST`). Whatever error the
  decoder (`FromBytes`/`FixedRead`) returns is **never reinterpreted**
  (never miscoerce a decoder-originated `DoesntExist` into absence). Wherever
  the current typed helpers fold host reads and decode into a single
  `Result`, the internals must be split apart to satisfy this rule.
  `get_required()`'s absence error is a dedicated variant (name finalized in
  Phase 1), distinguishable from a decode error.
- The basic form, `get()`, is provided with the same signature regardless of
  declaration mode.
- `default = <expr>` is a **runtime, compiled fallback expression**. Since
  an arbitrary Rust expression can't be evaluated to bytes at macro-expansion
  time, **the `default` value never makes it into the SetHook template**
  (§9.2). Carrying the encoded default into the artifact is a future concern
  (§10 D5).
- A correspondence table against the current `hook_parameter!` /
  `otxn_parameter!`-generated API is produced in Phase 1 to finalize names
  and signatures (§12.1). Semantic differences from the current API
  (particularly if "the fallback is not applied on a decode failure"
  becomes the new behavior) are documented in the migration table.

### 5.7 Entry function signatures

Entries inside the impl are **associated functions that take no `self`**,
with the same signature as today, `fn() -> i64` (likewise for `cbak`).
Writing `self` by mistake produces a dedicated "Hook entrypoints are
stateless associated functions" diagnostic (§4.4).

> **r5 revision**: the paragraph above reflects the original (r4) text. r5
> revised this so that a `&self` receiver (`fn(&self) -> i64`) is also
> accepted — see [HOOKS_SELF_RECEIVER_DESIGN.md](./HOOKS_SELF_RECEIVER_DESIGN.md)
> for the details, semantics, and diagnostic wording.
>
> **r6 revision**: `&self` is now REQUIRED on every entry (and every
> `#[cbak]`); the no-receiver form is an error. This is a breaking change
> that was folded into this feature branch before its merge into v0.2.0 —
> see [HOOKS_SELF_RECEIVER_DESIGN.md](./HOOKS_SELF_RECEIVER_DESIGN.md) §1,
> §3.1, §6.4, §7, and §8 for the final decision and its rationale.

## 6. Implementation-level technical considerations

### 6.1 Macro structure: outer `#[hooks]` plus inner inert attributes

A per-method attribute macro can't detect duplicate indices, verify
hook/cbak pairing, or collect chain-wide metadata, so — as with
wasm_bindgen / pymethods — this design attaches **`#[hooks]` to the impl
block, with the inner `#[hook(...)]` / `#[cbak(...)]` as inert attributes
the outer macro consumes**. The struct side also carries `#[hooks(...)]`.
Struct and impl live in the same module and are tied together by the
bidirectional handshake in §5.1. Since the struct macro and impl macro can't
see each other's expansion, remaining consistency checks (e.g. that
referenced fields actually exist) are left to ordinary type checking.

Generated during expansion:

1. Each entry's extern wrapper,
   `#[unsafe(export_name = "__rshooks_hook_3")] extern "C" fn ...`
   (during a selective build, only the target index gets the `hook` /
   `cbak` name. §7)
2. An index → metadata table carrier (a multi-index extension of the
   current `metadata!` carrier)
3. Error-level cross-check diagnostics (§6.2)
4. Per-field marker types and the rewritten field types (§5.4)
5. The bidirectional struct/impl handshake (trait impl, associated
   constants, assertion. §5.1)
6. The fixed-name link symbol for "1 crate 1 struct" detection (§5.1)

**Span-preservation contract**: user-written tokens are re-emitted verbatim
at their original spans, with only the consumed helper attributes stripped.
The sole exception is the marker injection into field types (§5.4). No
reconstruction from stringification.

### 6.2 Where each check lives

| Check | Location | Kind |
|---|---|---|
| index duplication/range (0..=9), cbak pairing, zero hooks (within a block), trigger-form exclusivity, missing directional-trigger side, direction-mask agreement, `required`+`default` combined | macro expansion | error |
| unsupported struct/impl shape (the table in §5.1), unattributed field, `self` on an entry | macro expansion | error |
| missing annotated impl / missing annotated struct / duplicate annotated impl | type checking (bidirectional handshake, §5.1) | error |
| leftover current-generation (v0.0.x) macros (`metadata!` etc.) | name resolution (unresolved error, since removed in v0.2) + guided by the book's migration chapter | error |
| transaction-name resolution (TRANSACTION_TYPES) | macro expansion (vendored table) | error |
| `#[cfg]`/`#[cfg_attr]` on an entry/field | macro expansion | error |
| multiple chain structs | linking (symbol collision) | error |
| `HookName` length rule (Phase-1-finalized protocol norm) | macro expansion | error |
| cbak declaration vs. `cbak` export's existence (per-index, after the selective build, before cleaning) | rshooks-build | error |
| emit / can_emit / cbak consistency (§6.3) | rshooks-build (against the selectively-built wasm, per index) | as in §6.3 |
| duplicate `HookName` sharing | rshooks-build | info |
| 64 KiB limit, guard, validator | rshooks-build (per wasm, as today) | error |

**Stable Rust's proc macros can only reliably emit hard errors via
`compile_error!`**, so all warning/info-level diagnostics are reported by
rshooks-build instead.

### 6.3 The canonical emit/can_emit/cbak consistency truth table

Detecting reachable `emit` usage is done against **the wasm from the
per-index selective build** (the discovery build includes every entry and
has no per-index reachability). "The `emit` import remains in the final
wasm" is the criterion for "uses emit."

| `can_emit` declaration | `emit` usage (detected) | `cbak` declared | Verdict |
|---|---|---|---|
| omitted (no override) | yes | yes | OK |
| omitted | yes | no | warning (emits but has no cbak) |
| omitted | no | yes | warning (doesn't emit but has a cbak) |
| omitted | no | no | OK |
| `[]` (deny-all) | yes | — | warning (emit will always fail at runtime) |
| `[]` | no | yes | warning (neither emits nor is permitted to, yet has a cbak) |
| `[]` | no | no | OK |
| non-empty allowlist | yes | yes | OK |
| non-empty allowlist | yes | no | warning |
| non-empty allowlist | no | — | warning (declaration unused) |

## 7. Build strategy: 1 crate → N wasm

### Approach A: discovery build + per-index `--cfg` recompilation (adopted for v0.2)

#### 7.1 BuildPlan (fixing every invocation)

The orchestrator (rshooks-build / xtask) first constructs an **immutable
BuildPlan**, applied to **both discovery and every selective build**. The
BuildPlan fixes at least the following:

- The package ID, workspace root, and **canonical lockfile path** resolved
  via `cargo metadata` (a workspace member uses the workspace's lock,
  preventing mix-ups in repos with multiple lockfiles, like the examples
  workspace)
- The lockfile's existence (generated first if absent) and its digest.
  **The digest is re-verified before and after each invocation**; any
  mid-run change is an error
- The complete argv (`cargo rustc --release --target wasm32v1-none --locked
  -p <package-id> --crate-type cdylib` plus `--cfg` / `--check-cfg`), the
  feature set, the profile, incremental builds disabled
- The toolchain (rustc/cargo version), the canonical cwd, and a content
  digest of every involved Cargo config file (`.cargo/config.toml`)
- An allowlist of environment variables and their values (nothing else
  propagates)
- **A target directory dedicated to this orchestration run**
  (`target/rshooks-build/<run or package>`, etc.). Discovery and the
  selective builds share this dedicated directory to benefit from caching,
  but it's kept separate from the user's normal `cargo build`, **structurally
  eliminating the TOCTOU where another process overwrites cargo's artifact
  output before it's read**. The orchestration lock is held from the start
  of compilation through staging completion
- `--check-cfg=cfg(rshooks_entry, values("0","1",...,"9"))` is **supplied by
  the orchestrator itself on every invocation**. The value domain is fixed
  to the full known range 0..=9 even before discovery (it does not depend
  on discovery's result)

The cfg name `rshooks_entry` is reserved. Since it's **mechanically
impossible to detect** user code referencing `cfg!(rshooks_entry)` to vary
behavior per index, this is documented as an unsupported contract (undefined
result).

#### 7.2 Canonical per-index processing order

After discovery, **each index is completed in full before moving to the
next** (the §7.1 lock is held throughout):

1. Compile with BuildPlan + `--cfg 'rshooks_entry="<i>"'`
2. Read the raw wasm bytes into the staging area **immediately**
3. Extract the carrier from the raw wasm and run the §7.3 consistency check
   (**extraction must happen before cleaning**, since the cleaner strips
   the carrier)
4. Cross-check cbak declaration vs. export (against the pre-cleaning export
   table)
5. Run the existing rshooks-build pipeline (cleaner/flatten/unnest/
   guard/validator)
6. Confirm the final wasm's exports are exactly `hook` (plus `cbak` if
   declared) and commit it to staging

#### 7.3 Discovery/selective-build consistency verification (CanonicalRecord)

- Each selective build's carrier includes a **`CanonicalRecordV1`**,
  compared against discovery's. What V1 includes (minimum):
  - a schema version tag
  - the index set (numerically ascending)
  - per index: entry function name, whether a cbak exists, `name`, trigger
    declaration (**in a form distinguishing omission / `all` / `[]` /
    enumerated, and symmetric/directional**), `can_emit` (**in a form
    distinguishing omission from `[]`**), description
  - struct level: description, and a normalized representation of the
    shared schema (state/param declarations)
  - `default` is included as a **normalized token-string of the
    expression** (the value is never evaluated)
- Serialization uses a versioned canonical byte sequence (fixed field
  order, canonical set ordering, UTF-8, preserving the absent/empty
  distinction), digested with SHA-256. The exact byte layout is
  **finalized at the start of Phase 2** (this doesn't block starting the
  macro work).
- On mismatch, report a **structural diff** (which field of which index)
  and both builds' context, not a bare digest.
- This check only guarantees **declared-metadata agreement**. Code
  differences from build scripts or environment-dependent macros are
  bounded by fixing the BuildPlan (§7.1), but not guaranteed to be fully
  detected (documented as such).

#### 7.4 Artifact generation management (atomicity)

- Output goes to a generation directory `gen-<n>/`, and the `current`
  symlink is repointed only after that phase's **complete set of public
  artifacts** (Phase 2: wasm + per-index sidecar; Phase 3 onward: plus
  template + meta sidecar) is present and has passed verification.
- **Consumers must resolve `current` exactly once and use the resolved,
  immutable `gen-<n>/` path from then on** (opening multiple paths through
  `current/...` separately risks straddling generations). This convention is
  documented in the artifact directory's README.
- Staging lives on the same filesystem as the output destination.
  Concurrent builds are mutually excluded via a lock file. On failure,
  `current` is left untouched (preserving the previous generation). Old
  generations are pruned to a fixed count (e.g. 2) on success; pruning is
  documented as potentially racing with a consumer still using a resolved
  `gen-<n>` (hence a grace period before immediate deletion).

Advantages: the post-processing pipeline can keep assuming "1 wasm = hook +
optional cbak," as it does today. LLVM's DCE minimizes it, data segments
included. Disadvantage: N+1 compilations (dependency crates are cached, so
only the leaf crate recompiles).

### Approach B: single compile + wasm-splitting pass (ideal form, future optimization)

Build one wasm containing every suffixed export, then have a split pass
perform, per index, "rename the target exports to `hook`/`cbak` → delete the
rest → DCE → the existing pipeline." This compiles once — the fastest —
and makes discovery and the artifact the same thing, eliminating the need
for §7.3 as well, but it requires new wasm-surgery implementation (function,
table, and data-segment reachability analysis), and a bug there would affect
every hook.

**Decision: the ideal form is Approach B, but v0.2 adopts Approach A,
prioritizing simplicity and reuse of the existing pipeline.**

The **definition of equivalence** for A→B (§10 D4): byte identity, HookHash,
size, and WCE are **not expected to match**. What's required is (1) the
index → (hook, cbak) mapping and all deployment metadata agree, (2) each
index passes the validator, and (3) per-index differential execution tests
agree (matching accept/rollback, return value, and host call sequence in e2e
for the same input).

(Approach C, generating a shim crate, has no advantages and is not adopted.)

### 7.5 Artifact naming

```
target/rshooks/<crate-name>/
  current -> gen-3/
  gen-3/
    0.deposit.wasm              # <index>.<fn name>.wasm
    0.deposit.metadata.json     # per-index version of the current sidecar (includes the shared-schema transcription)
    1.sweep.wasm
    1.sweep.metadata.json
    sethook.template.json       # owned-position patch (§9)
    sethook.template.meta.json  # generation-info sidecar (§9.2)
```

### 7.6 The contract with plain cargo / rustdoc

Behavior is also specified for direct cargo invocations that bypass the
orchestrator:

- `cargo check` / `cargo doc` / docs.rs: **supported** (compiles cleanly,
  no warnings). Every generated item is `#[doc(hidden)]` (§5.5), so rustdoc
  shows only the user's public API.
- Plain `cargo build`: compiles successfully, but the artifact is a
  **discovery-equivalent, non-deployable product** carrying only
  suffixed exports (it lacks the exact `hook` export, so it can't be
  accidentally installed as-is — this is deliberately preserved as a safety
  property). Installable wasm is only produced through the orchestrator.
- Generated code's use of `cfg(rshooks_entry = ...)` gets a scoped
  `#[allow(unexpected_cfgs)]` on the generated item, so plain cargo (which
  doesn't pass `--check-cfg`) never emits an `unexpected_cfgs` warning
  (part of the lint contract in §5.5).
- Phase 1 includes contract tests for all three of plain check / build /
  doc.

## 8. Syntax (normative skeleton)

> The attribute grammar's complete BNF and accessor names are finalized in
> Phase 1 (§5.4, §5.6). The accessor names in the examples below are
> provisional. The skeleton (struct/impl, index, the attribute arguments'
> vocabulary and semantics) is settled by this document.

### 8.1 Multi-hook example

```rust
#![no_std]
use rshooks::*;

#[hooks(description = "Deposit vault with sweep")]
pub struct Vault {
    #[state(key(prefix = b"B", field(account: AccountId)))]
    deposits: State<DepositValue>,

    #[hook_param(name = b"CFG", default = Config { max: xfl!(1000), lock: 10 })]
    config: HookParam<Config>,

    #[otxn_param(name = b"INS", required)]
    instruction: OtxnParam<Instruction>,
}

#[hooks]
impl Vault {
    /// Records a deposit.
    #[hook(0, name = "deposit", on_incoming = [Payment], on_outgoing = [], can_emit = [])]
    fn deposit(&self) -> i64 {
        // On absence, the default expression's value; a decode failure is Err (§5.6)
        let Ok(cfg) = self.config.get_or_default() else {
            rollback!(b"vault: bad CFG", 1);
        };
        // ...
        accept!(b"deposited", 0)
    }

    /// Collects the balance and sends it.
    #[hook(1, name = "sweep", on = [Invoke], can_emit = [Payment])]
    fn sweep(&self) -> i64 { /* ... */ }

    #[cbak(1)]
    fn sweep_cbak(&self) -> i64 { accept!() }
}
```

### 8.2 Relationship to `hook_errors!` / `txn_template!`

`hook_errors!` and `txn_template!` are independent declarations that can be
shared across hooks, so v0.2 **leaves them unchanged, at the top level**.

### 8.3 Relationship to guard

The entry shape (export wrapper + inner fn) matches what the current
`#[hook]` generates today, so guard insertion/checking (the `_g` import,
WCE computation) continues to work per-wasm, per index, exactly as before.
No impact.

### 8.4 Minimal form of a single hook

```rust
#[hooks]
pub struct MyHook;

#[hooks]
impl MyHook {
    #[hook(0)]  // trigger omitted = no installation override (§5.3)
    fn main(&self) -> i64 {
        accept!(b"ok", 0)
    }
}
```

The difference from the current minimal form (`metadata!` + `#[hook] fn`) is
a net 4–6 lines (see the assessment in §4.1). Unit structs are allowed, and
if there's no state/param, no fields are needed (in that case no static is
generated, §5.5). The index is not omittable even for a single hook
(settled point #3; rationale in §4.1).

## 9. SetHook transaction template generation

### 9.1 Artifacts

`sethook.template.json` (an owned-position patch. **A protocol-shaped,
editable JSON**: its field layout matches the protocol, but it only becomes
a valid transaction after placeholder substitution and validation):

```json
{
  "TransactionType": "SetHook",
  "Account": "<ACCOUNT>",
  "Hooks": [
    { "Hook": {
        "CreateCode": "<hex of 0.deposit.wasm>",
        "HookOnIncoming": "<64-hex mask (Payment)>",
        "HookOnOutgoing": "<64-hex deny-all mask>",
        "HookCanEmit": "<64-hex deny-all mask>",
        "HookNamespace": "<NAMESPACE>",
        "HookApiVersion": 0,
        "HookName": "6465706F736974"
    } },
    { "Hook": {
        "CreateCode": "<hex of 1.sweep.wasm>",
        "HookOn": "<64-hex mask (Invoke)>",
        "HookCanEmit": "<64-hex allowlist mask (Payment)>",
        "HookNamespace": "<NAMESPACE>",
        "HookApiVersion": 0,
        "HookName": "7377656570"
    } }
  ]
}
```

`sethook.template.meta.json` (generation-info sidecar, not for submission):

```json
{
  "crate": "vault",
  "version": "0.2.0",
  "generated_at": "2026-08-18T09:00:00Z",
  "hook_hashes": { "0": "<sha512half>", "1": "<sha512half>" },
  "positions": { "declared": [0, 1], "gaps": [], "untouched_beyond": 2 },
  "required_amendments": ["Hooks", "NamedHooks", "HookOnV2", "HookCanEmit"]
}
```

Mask values in real generated output are complete 64-digit hex (HookOn is
active-low with a SetHook-bit special case; HookCanEmit's deny-all is "every
bit 1, except the SetHook bit is 0," not simply all-F. Derivation reuses the
existing implementation). **All generated hex is normalized to uppercase**
(settled point #16).

### 9.2 Generation rules

- **Trigger mapping** (§5.3's declaration forms carried straight to the
  wire): fully omitted → no trigger field / `on = all` → an all-zero
  `HookOn` mask / `on = [..]` → `HookOn` / directional → `HookOnIncoming` +
  `HookOnOutgoing` (mutually exclusive with `HookOn`).
- `Hooks[i]` is the hook at index i. A gap index becomes `{"Hook": {}}` (a
  position-preserving no-op). Array length is **exactly 0..=the highest
  declared index**.
- When `can_emit` is omitted, no `HookCanEmit` field is emitted (carrying
  §5.3's three-valued semantics straight through to the wire).
- `Account` / `HookNamespace` are **placeholders**. Filling them via CLI
  options (`--account`, `--namespace`) is allowed, but they can't be written
  as source attributes.
- **`Flags` is not emitted by default (fail-closed).** With `--override`,
  `hsfOVERRIDE` is applied **only to declared (non-gap) entries**. Adding
  `Flags` to a gap's `{"Hook": {}}` would make it no longer a no-op —
  interpreted and rejected as a different operation — so **gap objects
  always stay strictly empty** (gapped-layout + `--override` verification
  cases are included in Phase 3).
- **`HookParameters` is never generated for ordinary `#[hook_param]`/
  `#[otxn_param]` fields** (settled point #10). To set an installed
  parameter for one of those, add it to the template by hand. On the wire,
  the three states are "omission = inherit the HookDefinition default,"
  "name only = clear the inherited value," and "name+value = set
  explicitly"; omission does not guarantee "no parameter." This caveat is
  documented in the template's own documentation. **Scoped exception,
  realizing §10 D5:** an entry with declared signature parameters
  (`docs/PARAM_SIGNATURE_DESIGN.md` §1/§4 — extra `#[hook(..)]` fn
  arguments) DOES get a `HookParameters` array, one declaration entry per
  argument in wire-index order (`HookParameterValue` always the literal
  `"00"` placeholder — a signature parameter is always REQUIRED, so there is
  no default to embed and no "omission means inherit" ambiguity to
  preserve). An entry with no declared signature parameters still emits no
  `HookParameters` key at all.
- **Generation info goes in a separate sidecar**
  (`sethook.template.meta.json`); the template body itself stays
  protocol-shaped JSON. The sidecar includes an RFC 3339 `generated_at`,
  the HookHash, position info (`declared` / `gaps` / `untouched_beyond`),
  and `required_amendments`.
- `required_amendments` **unconditionally includes `Hooks`**, plus whatever
  can be derived from the template's fields (NamedHooks / HookOnV2 /
  HookCanEmit). **It does not cover amendment dependencies arising from
  Hook API usage inside the wasm** (this limitation is documented in the
  sidecar's docs; extending it via a feature→amendment registry is a future
  concern).

### 9.3 Template semantics: an owned-position patch

`{"Hook": {}}` means "don't touch this position" — a **positional no-op** —
not "this position is empty." Consequently this template:

- **leaves any already-installed hook at a gap position untouched**
- **leaves any existing hook at a position past the highest declared index
  untouched**
- is, in other words, not "a declarative realization of the whole chain,"
  but **"a patch against the declared owned positions" (an owned-position
  patch)**

Because of this, **successfully applying the template does not guarantee
the account's whole-chain behavior is fully determined by the source
declaration alone.** Operators should check the target account's existing
chain before submitting. The sidecar's `positions` field exists to support
that check. Converging the ledger to match the declaration (deriving
deletions/replacements for unwanted positions) is out of scope for v0.2.

Likewise, the template **never includes**: multi-account or per-network
configuration, grants, a diff against the existing chain, delete operations,
or actual parameter values/secrets.

By default (without `--override`), the template is an editable draft of "a
single transaction that installs fresh, exactly as the source declares,
into an account whose declared positions are empty." With `--override`, it
becomes a draft that also permits replacing an existing hook at a declared
position.

## 10. Open questions

| ID | Question | Recommendation | Notes |
|---|---|---|---|
| D1 | Sharing state across structs (reading the same state as a chain in another crate) | Out of scope for v0.2 | Handled via sharing the type-definition crate (an ordinary Rust mechanism) |
| D2 | Per-hook state/param usage declarations (`uses = [...]`) | Not included in v0.2 (every declaration = the shared schema) | The sidecar's declarations are labeled "a transcription of the shared schema" (§5.4). Detection-based narrowing is a future concern |
| D3 | Execution-mode attributes like weak/collect/again | Not included in v0.2 | Add the attribute once it's needed |
| D4 | The switchover condition from build strategy A to B | Once measurement shows build time is a real problem | The equivalence definition is already fixed in §7 (byte identity is not required) |
| D5 | Carrying the encoded default into the artifact | **Realized for declared signature parameters** (`docs/PARAM_SIGNATURE_DESIGN.md` §1/§4): the name/type-byte encoding is resolved at macro time and extracted via the `#[hooks] impl` wasm carrier (`EntryDecl::sig_params`, §4 of that doc), and `sethook_template.rs` emits a `HookParameters` declaration entry per argument (§9.2's scoped exception to settled point #10). Still open for ordinary `#[hook_param]`/`#[otxn_param]` fields' `default` — those have no wire-format REQUIRED-ness guarantee backing a fixed placeholder value the way a signature parameter does, so this remains a future concern for them | Either a const-evaluable encoding, or extraction via the wasm carrier — the signature-parameter case took the latter route |
| D6 | Conditional-compilation (`#[cfg]`) support | Forbidden in v0.2 (§5.1) | Define the consistency semantics with discovery once needed, then lift the ban |

## 11. Overall developer-experience assessment

| Dimension | Single-hook project | Chain (multi-hook) project |
|---|---|---|
| Amount to write | more (net 4–6 lines plus two concepts, §4.1) | much less (crate consolidation, shared declarations) |
| Correctness | about the same (slight improvement from localized metadata) | much better (compiler-guaranteed shared schema, pair verification, codified occupied positions) |
| Build | essentially unchanged | N+1x build time (leaf only), but manual integration disappears |
| Deployment | improved via template generation | much better (the owned-position patch comes out as one JSON) |
| Learning | needs the struct ritual + index explained | "1 struct = 1 chain" is, if anything, an easier concept to teach |
| IDE/debugging | risk of regression from macro complexity (mitigated by the span-preservation contract and diagnostic catalog, §4.4) | same |

Overall, **this proposal's value is maximized for chain development, while
its cost concentrates in the added ritual for a single hook and the macro
implementation's complexity**. Xahau's real products (governance, reward,
firewall groups) are mostly chain-oriented, so making the chain a
first-class citizen of the toolchain is a sound call.

## 12. Migration plan

The migration source is **the current v0.0.x API** (in this document,
"current" always means v0.0.x).

### 12.1 Canonical migration table (a Phase 1 deliverable)

The following correspondence table is produced in Phase 1, with mechanical
rewritability as the acceptance criterion:

| Old (current v0.0.x) | New (v0.2) |
|---|---|
| `metadata! { name, description }` | Cargo package name / `#[hooks(description = ...)]` |
| `metadata! { HookOn / IncomingHookOn / OutgoingHookOn / fully omitted }` | entry attribute `on` / `on_incoming` + `on_outgoing` / fully omitted (plus the new `on = all`) |
| `metadata! { HookCanEmit, HookName }` | entry attributes `can_emit` / `name` |
| `#[hook] fn` / `#[cbak] fn` | `#[hook(i, ...)]` / `#[cbak(i)]` inside a `#[hooks] impl` |
| `hook_state!` (every declaration form) | `#[state(...)]` field |
| `hook_parameter!` / `otxn_parameter!` (every declaration form) | `#[hook_param(...)]` / `#[otxn_param(...)]` field |
| each declaration macro's generated accessor | the handle's accessor (names, signatures, and semantic differences documented in the correspondence table, §5.6) |
| artifact path (single wasm + sidecar) | `target/rshooks/<crate>/current/` generation layout (§7.5) |

### 12.2 Examples / book

- Examples 01–15: stay single-hook, mechanically rewritten into the new
  form (subject to the migration table's acceptance criteria).
- **`80_reward` + `81_govern` are consolidated into a single chain example
  crate** (this consolidation is itself the proof of this proposal, so it's
  treated as a design task, not a mechanical rewrite). This unifies the
  shared key-layout types into the struct declaration and assigns indices
  (matching the genesis account's chain layout). **Phase allocation**:
  Phase 1 only goes as far as a desk proof that every declaration form in
  80/81 is expressible in the new grammar (part of the migration table's
  completeness acceptance criteria). Implementing and building the
  consolidated crate is Phase 2 (it requires multiple indices). e2e
  installation verification via the generated template is Phase 3. The old
  two-crate version is deleted once consolidation is complete.
- The book rewrites its opening tutorial around the new minimal form
  (§8.4), explaining upfront that "the struct is a container for the
  declaration, with no runtime instance" and "index is the occupied
  position."

### 12.3 Acceptance criteria

- All examples build under `-D warnings` (the lint contract, §5.5)
- The old→new rewrite can be completed using only the migration table (no
  additional word-of-mouth knowledge required)
- e2e: the consolidated govern/reward chain can be installed onto a
  standalone node via the generated template, and the existing e2e
  scenarios pass (Phase 3)

## 13. Implementation roadmap (reference)

1. **Phase 1**: the `#[hooks]` struct/impl macro (marker-type injection
   §5.4, bidirectional handshake §5.1, shape table §5.1) plus **the full
   safety foundation for Strategy A, complete for a single index**:
   BuildPlan (§7.1), per-index processing order (§7.2),
   discovery/selective-build consistency comparison (§7.3; finalizing
   CanonicalRecord's byte layout can wait until the start of Phase 2, but
   the comparison itself runs from Phase 1). Since the existing pipeline
   requires an exact `hook`/`cbak` export, the artifact only really works
   once the selective build is included. Plus the canonical migration table
   (§12.1), the 80/81 expressibility desk proof (§12.2), the accessor
   correspondence table and absence-detection rule (§5.6), the diagnostic
   catalog plus trybuild (§4.4), compile-fail/pass tests (§5.1), the lint
   contract CI (§5.5), and the plain cargo/rustdoc contract tests (§7.6).
   Migration of examples 01–15 and the book.
2. **Phase 2**: multi-index support (iterating and assembling multiple
   artifacts). Finalize CanonicalRecordV1's byte layout (§7.3), generation
   management (§7.4; the public artifact set is wasm + per-index sidecar),
   implementing the §6.3 truth table, implementing and building the 80/81
   consolidated crate.
3. **Phase 3**: SetHook template + meta sidecar generation (§9; templates
   added to the generation's public artifact set). Gap + `--override`
   verification. e2e (hooks-toolkit) verification of a real install using
   the template (including the consolidated govern/reward example).
4. **Phase 4** (optional): swap in build strategy B (the wasm-splitting
   pass) (equivalence tests from §7 are prepared first).

## 14. References

- [docs/DESIGN.md](./DESIGN.md) — the current architecture (§5.4 entry
  points, §6 rshooks-build)
- `crates/rshooks-build/src/metadata.rs` — the current implementation's
  validation of the three trigger forms and wire serialization of
  `HookOnIncoming`/`HookOnOutgoing`
- [Xahau SetHook](https://xahau.network/docs/protocol-reference/transactions/transaction-types/sethook/) — the `Hooks` array's positional semantics, empty `Hook` objects, hsfOVERRIDE, field inheritance on Install
- [Xahau HookOn](https://xahau.network/docs/hooks/concepts/hookon-field/) / HookOnV2 / NamedHooks / HookCanEmit amendments
- [Xahau Parameters](https://xahau.network/docs/hooks/concepts/parameters/) — inheritance, clearing, and explicit setting of HookParameters
