# Hook and Transaction Parameters

Hooks read configuration from two distinct sources, both called
"parameters" but attached at very different times: **hook parameters**,
set once when the hook is installed (via `SetHook`), and **originating
transaction parameters**, attached fresh by whoever submits the triggering
transaction. This page covers both — the loose byte-buffer accessors, the
`#[hook_param(...)]`/`#[otxn_param(...)]` struct-field attributes that pair
a name with a value type, and composite (struct-shaped) names via
`#[derive(ParamName)]`.

## Two sources, one shape

| | Hook parameter | Otxn parameter |
|---|---|---|
| set by | the hook's operator, at `SetHook` time | whoever submits the triggering transaction |
| read with | `hook_param`/`hook_param_exact`/`hook_param_typed` | `otxn_param`/`otxn_param_exact`/`otxn_param_typed` |
| struct field | `#[hook_param(...)] field: HookParam<V>` | `#[otxn_param(...)] field: OtxnParam<V>` |
| typical use | operator-controlled configuration (a minimum amount, a blocklist entry, a pause switch) | per-invocation instructions the caller supplies |

Both mechanisms are read-only from the reading hook's own perspective — a
hook parameter is set by whoever installs the hook, not written by the hook
itself at runtime (`hook_param_set` exists, but it writes a *different*
hook's parameter, taking a raw `&[u8]`, not a typed value — out of scope
for this page). Because of that, both sides share the exact same field
attribute grammar and accessor shape; only the underlying host call
differs.

## The loose accessors

`hook_param`/`otxn_param` read a named parameter into a caller-provided
buffer, mirroring `otxn_field`'s shape (see [Reading the Originating
Transaction](otxn.md)):

```rust
use rshooks::prelude::*;

let mut buf = [0u8; 32];
let written = hook_param(&mut buf, b"CFG")?;
```

`hook_param_exact`/`otxn_param_exact` require the parameter to be exactly
`T`'s length (any `FixedRead` type), with `T` inferred from context, not a
turbofish. A typical use is a compiled-in default when the operator hasn't
configured a minimum:

```rust,ignore
const THRESH_PARAM: &[u8] = b"THRESH";
const DEFAULT_THRESHOLD: u64 = 1_000_000;

fn threshold() -> u64 {
    hook_param_exact(THRESH_PARAM)
        .map(u64::from_be_bytes)
        .unwrap_or(DEFAULT_THRESHOLD)
}
```

`hook_param_exact`'s return type is inferred as `[u8; 8]` from the
`.map(u64::from_be_bytes)` call — no turbofish needed. Note the
`from_be_bytes`, not this crate's `FromBytes` trait: a raw parameter byte
buffer is whatever the caller who set it chose to write, and this tier
leaves that byte convention entirely up to whatever wrote it — here
chosen (by this snippet, not the crate) to match Xahau Binary's
big-endian numeric encoding, the same convention [Reading the Originating
Transaction](otxn.md) describes for raw protocol fields.
`.unwrap_or(DEFAULT_THRESHOLD)` collapses "not configured at all" and
"configured with a value of the wrong size" into the same fallback,
without treating a malformed parameter as a hard error. This tier needs
no field declaration on the `#[hooks]` struct at all — reach for it for a
one-off read, or as the escape hatch [Hook
Chains](../concepts/chains.md#a-real-limit-typed-accessor-density-inside-one-entry)
covers when accessor density at one call site outgrows the nesting
budget. The declared-field tier below decodes every value through this
crate's `FromBytes` trait instead, so its byte convention is always
little-endian, fixed by the crate rather than chosen per call site — see
`examples/03_hook-params`'s `MIN` parameter for a worked example.

## Struct fields: `#[hook_param(...)]` / `#[otxn_param(...)]`

`hook_param_exact`/`otxn_param_exact` take the name and the value type `T`
as two *independent* arguments — nothing stops calling
`otxn_param_exact::<WrongType>(b"T")` for a name/type combination that
was never intended, as long as `WrongType: FixedRead` (true of nearly
every fixed-size type this crate provides, including some *other*
parameter's value type). A field on the `#[hooks]` struct closes that gap
by declaring a name permanently paired with one value type:

```rust,ignore
#[hooks]
pub struct Firewall {
    /// The blocked account, configured via the `BL` Hook parameter.
    #[hook_param(name = b"BL")]
    blocked: HookParam<AccountId>,
}
```

The attribute grammar is the same for both `#[hook_param(...)]` and
`#[otxn_param(...)]` — only the field's type (`HookParam<V>` vs.
`OtxnParam<V>`) picks which host call the generated accessors read
through:

| argument | meaning |
|---|---|
| `name = <byte-string literal>` | a fixed, literal name — free at runtime, since the wire encoding *is* the literal's own bytes |
| `name_by = <TypePath>` | a composite name, constructed per call site — see "Composite names" below |
| `required` | adds `.get_required()` (mutually exclusive with `default`) |
| `default = <expr>` | adds `.get_or_default()`, falling back to `<expr>` (mutually exclusive with `required`) |

### The accessors, and what "absent" actually means

Every field gets `.get() -> Result<Option<V>>` unconditionally; `required`
and `default` each add one more method, and are mutually exclusive because
they answer the same question — "what happens when this parameter isn't
set?" — two different ways:

| method | available | behavior on absence | behavior on a present-but-malformed value |
|---|---|---|---|
| `.get()` | always | `Ok(None)` | `Err` |
| `.get_or_default()` | with `default = <expr>` | `Ok(<expr>)` | `Err` — **not** silently replaced by the default |
| `.get_required()` | with `required` | `Err` (a dedicated "missing" error, distinct from a decode error) | `Err` |

The load-bearing rule, worth stating precisely: **"absent" is decided
before any decoding happens**, from the host API's own "doesn't exist"
signal — never inferred after the fact from a decode failure. A parameter
that *is* set, but to the wrong number of bytes for `V`, is a decode
failure, and `.get_or_default()` reports that as `Err` rather than quietly
substituting the default. If you want "any read failure at all, absence or
malformed, falls back to the same value," write that explicitly at the
call site instead, the same way `examples/05_firewall` does:

```rust,ignore
fn blocked_account() -> Option<AccountId> {
    Firewall.hook_param.blocked.get().ok().flatten()
}
```

`.ok()` turns `Err` into `None`, and `.flatten()` collapses the resulting
`Option<Option<AccountId>>` down to one level — deliberately masking a
decode failure the same way as absence, rather than treating a malformed
`BL` value as anything worth telling the caller apart from "no blocklist
configured."

### `required`: a required parameter

```rust,ignore
#[hooks]
pub struct StateForeign {
    /// The target account whose flag this hook reads (`ACCT`).
    #[hook_param(name = b"ACCT", required)]
    acct: HookParam<AccountId>,
}
```

```rust,ignore
let Ok(target) = self.hook_param.acct.get_required() else {
    rollback!(
        b"state-foreign: ACCT parameter not configured",
        StateForeignError::AcctNotConfigured
    )
};
```

(from `examples/09_state-foreign`'s `&self` entry.) `get_required()`
collapses "absent" and
"present but malformed" into the same `Err` at this call site — the hook
treats both as "can't proceed," and the `else` branch rolls back either
way.

### `default`: a compiled-in fallback

```rust,ignore
#[hooks]
pub struct TypedData {
    /// Install-time configuration (`CFG`). Falls back to compiled-in
    /// defaults when absent.
    #[hook_param(name = b"CFG", default = Config { min_amount: DEFAULT_MIN_AMOUNT, lock_ledgers: DEFAULT_LOCK_LEDGERS })]
    config: HookParam<Config>,
}
```

(from `examples/12_typed-data`.) `default`'s expression is a **runtime
fallback**, not baked into the deployed `SetHook` template as an installed
value — see [Per-Hook Attributes](../build/metadata.md) for why a
`HookParameters` entry has to be added to the template by hand if you want
a position to install with a concrete value at all. Contrast `required`
above: the field declaration itself (`default = ...` vs. `required`) is
where a parameter's presence policy lives, not scattered across call
sites — `config` here degrades gracefully when `CFG` is unconfigured, the
same way `examples/09_state-foreign`'s `acct` field (above) never does.

## Composite names: `#[derive(ParamName)]` and `name_by`

A Hook API parameter name isn't always a plain literal tag — per the Hook
API itself it's a genuine variable-length key of up to 32 bytes, and (like
a hook state key) can be a whole composite, struct-shaped value instead of
a byte string. `#[derive(ParamName)]` derives `ToBytes` (write-only — see
[Typed Data with Derives](typed-data.md)) for a named-field struct used
this way; `name_by = <TypePath>` is how a field references one:

```rust,ignore
#[derive(ParamName, Clone, Copy)]
struct AdminName {
    section: u8,
    field: u8,
}

#[hooks]
pub struct TypedData {
    /// Administrative deposit pause switch, addressed by [`AdminName`].
    /// Falls back to "not paused" when absent.
    #[hook_param(name_by = AdminName, default = PauseSwitch { paused: 0 })]
    admin_pause: HookParam<PauseSwitch>,
}
```

Unlike a `key = ...`/`name = ...` field, which is ready to read directly, a
`name_by` field needs its name's runtime value bound first, via `.at(...)`
— the same shape [Hook State](state.md#key_by---a-key-constructed-per-call-site)
uses for a keyed state entry:

```rust,ignore
const ADMIN_PAUSE_NAME: AdminName = AdminName { section: 0, field: 0 };

fn deposits_paused() -> Result<bool> {
    TypedData
        .hook_param
        .admin_pause
        .at(ADMIN_PAUSE_NAME)
        .get_or_default()
        .map(|s| s.paused != 0)
}
```

Returning `Result<bool>` rather than `bool` keeps a malformed switch's
`Err` visible to the caller, which rolls back on it instead of treating it
as `false`.

`.at(args)` returns a handle over that one bound name, carrying the same
`.get()`/`.get_or_default()`/`.get_required()` accessors the base field
has (gated by the same `required`/`default` declaration). `AdminName`
encodes to 2 bytes (`section` then `field`, no padding), comfortably
inside the Hook API's 1-to-32-byte parameter-name bound. Unlike an
oversized `HookData` state value (no size cap at all), `#[derive(ParamName)]`
checks this bound — both the 1-byte lower bound and the 32-byte upper
bound — **at the struct's own definition**, so an out-of-range name fails
to compile before it's ever used.

Because `AdminName` is composite rather than a fixed byte string, its
name-encoding has to actually run at runtime — laying `section` and
`field` out into a small buffer sized exactly to `AdminName::MAX_LEN`.
`examples/12_typed-data`'s README measures this directly: +29 worst-case
instructions over the same hook without the composite name, versus the
near-zero cost of the plain `CFG` tag used elsewhere in that same hook.

## Signature parameters (fn arguments)

> This section's surface requires the `unstable-param-sig-interface`
> feature: `rshooks = { version = "…", features = ["unstable-param-sig-interface"] }`.
> `unstable-*` features track draft specs and are exempt from semver —
> breaking changes may land in a minor release while the spec is a draft.

Both surfaces above pair a name with a value type through this crate's own
choices (a struct field, `default`/`required`). The **Hook Parameter
Signature Interface** is a different, protocol-level convention: a
`HookParameterName` wire format that makes a Hook's declared parameters a
machine-readable, typed function signature, and `rshooks` maps it onto the
most direct possible Rust surface — extra arguments on the entry fn itself.
The normative rules live in `docs/PARAM_SIGNATURE_DESIGN.md`; this section
covers the day-to-day surface built on it.

```rust,ignore
#[hooks]
impl Increment {
    /// increment(account: AccountID, count: UInt16)
    #[hook(0, on = [Invoke])]
    fn increment(&self, account: AccountId, count: u16) -> HookResult {
        // `account` and `count` are already decoded here.
        ...
    }
}
```

(from `examples/19_param-signature`, the interface draft's own worked
example.) Every argument after `&self` on a `#[hook(..)]` fn declares one
signature parameter, in declaration order — the argument's position (0-based)
is its wire **index**, its identifier is its display **name**, and its type
picks the wire **type byte**. `#[cbak(..)]` fns cannot declare extra
arguments: a callback's originating transaction is the emitted transaction,
not the invocation, so the interface doesn't apply there.

### The wire name

Each declared parameter's `HookParameterName` is a fixed-layout byte string,
8 to 23 bytes total:

| bytes | meaning |
|---|---|
| `0x5F 0x50 0x53` | `"_PS"` interface identifier |
| `0x00` | version |
| 1 byte | index, `0x00..=0x0F` (so at most 16 arguments per entry) |
| 1 byte | the type code (see the type table below) |
| 1 byte | name length, `0x01..=0x10` |
| 1 to 16 bytes | the display name, `[A-Za-z][A-Za-z0-9]*` (no `_`) |

`rshooks` builds this name entirely at macro/compile time — never at
runtime — and validates every one of those rules (index range, a supported
type byte, the name's charset/length) as a `const`-evaluable assert, so a
malformed declaration is a compile error, not a deploy-time or runtime
surprise. A Rust identifier containing `_` (the common case — `min_amount`,
say) therefore cannot be used as a signature-parameter argument name; rename
it, or fall back to the escape hatch below with an explicit name literal.

### Supported types

| Rust type | type code | wire payload |
|---|---|---|
| `u8` | `0x10` (`STI_UINT8`) | 1 byte |
| `u16` | `0x01` (`STI_UINT16`) | 2 bytes |
| `u32` | `0x02` (`STI_UINT32`) | 4 bytes |
| `u64` | `0x03` (`STI_UINT64`) | 8 bytes |
| `[u8; 16]` | `0x04` (`STI_UINT128`) | 16 bytes |
| `[u8; 32]` / `Hash` | `0x05` (`STI_UINT256`) | 32 bytes |
| `AmountBytes` | `0x06` (`STI_AMOUNT`) | 8 (native) or 48 (IOU) bytes |
| `Blob<N>` | `0x07` (`STI_VL`) | 1 to `min(N, 256)` bytes |
| `AccountId` | `0x08` (`STI_ACCOUNT`) | 20 bytes |
| `[u8; 20]` | `0x11` (`STI_UINT160`) | 20 bytes |
| `IssueBytes` | `0x18` (`STI_ISSUE`) | 20 (native, all-zero) or 40 (issued) bytes |
| `CurrencyCode` | `0x1A` (`STI_CURRENCY`) | 20 bytes |
| `XFL` | `0x80` (`XFL`, XAS-010d non-standard) | 8 bytes |

Every integer type here decodes **big-endian**, unlike this crate's own
`ToBytes`/`FromBytes` little-endian convention covered earlier on this page
and in [Typed Data with Derives](typed-data.md) — a signature parameter's
value crosses the same protocol boundary a raw `otxn_field`/`otxn_param`
read does (see [Reading the Originating Transaction](otxn.md)), not this
crate's own hook-private wire format. `Blob<N>`/`IssueBytes` are new types
in `rshooks::sig`; every other row is a type this page and [Typed Data with
Derives](typed-data.md) already cover. `XFL`'s payload is the Hook API's
`int64_t` XFL bit pattern, big-endian — `XFL::raw_bits()` — decoded via
`XFL::from_raw_bits` with no validity check; the type codes themselves come
from XAS-010d (Hook Type Codes), which both this interface and the Hook
State Interface reference for their type codes.

### The generated prologue and its rollback

For each declared argument, the `#[hooks]`-generated code ahead of the
entry's body reads the value via `otxn_param` (against the full declared
name above) and decodes it per the argument's type. On any failure —
absent, or the wrong length/shape for the declared type — it rolls back
immediately, with:

- message `b"rshooks: bad sig param '<name>'"`
- code = the argument's own 0-based index

before the body ever runs, so the body never sees a partially-decoded
invocation. See `examples/19_param-signature` for this rollback exercised
end-to-end (both a hand-written unit test via
`rshooks_testenv::TestEnv::invoke`, and e2e).

#### The `>= 16` convention for hook-authored rollback codes

Because the generated prologue's own rollback code is always an argument
index, `0x00..=0x0F` (`0..=15` — the interface's own index bound), any
`rollback!`/`hook_errors!` code your own entry body uses has to stay clear
of that whole range, or the two rollback sources (the generated prologue,
and your own body) become ambiguous by code alone: a caller inspecting
`HookReturnCode` in isolation can no longer tell "argument 3 was malformed"
from "my own error variant 3" without also checking the message. Every
`rshooks` example that declares signature parameters and its own
`hook_errors!` enum (currently just `examples/19_param-signature`) follows
the same fix: number every hook-authored variant from `16`, one
past the highest possible argument index, rather than the usual `1`. This
is a convention, not something the macro enforces — `hook_errors!` accepts
any `i64` discriminant — but it is the one this crate's own examples use
consistently for any signature-parameter-declaring entry, and is worth
adopting in your own hooks for the same reason.

### The escape hatch

Per the standing rule that every macro surface documents its raw
counterpart, `rshooks::sig` exposes the same name-building and decoding
directly, for a hand-rolled read outside the entry-fn-argument surface:

```rust,ignore
use rshooks::sig::otxn_sig_param;
use rshooks::sig_name;

const COUNT_NAME: [u8; 12] = sig_name!(1, u16, b"count");
let count: rshooks::error::Result<u16> = otxn_sig_param(&COUNT_NAME);
```

`sig_name!(index, Type, name)` resolves the wire name and type byte for
you at compile time; `sig_param_name` (also in `rshooks::sig`) is its
lower-level, non-macro counterpart. See `crates/rshooks/src/sig.rs`'s own
rustdoc for the full trait (`SigParamType`) and every type's decode
contract.

### Generated `SetHook` declarations

An entry with signature-parameter arguments needs its parameters
*declared*, not just read — the interface requires an on-ledger
declaration entry (`HookParameterValue = 0x00`) at `SetHook` time for every
parameter the entry's signature names. `rshooks build` generates this
automatically: any `#[hook(..)]` entry with signature-parameter arguments
gets a `HookParameters` block in `sethook.template.json`, one entry per
declared argument, in index order — see [Per-Hook Attributes and the
SetHook Template](../build/metadata.md) for the exact shape and where it
sits among that entry's other generated fields. This supersedes the
general "`HookParameters` is never generated" rule *only* for declared
signature parameters; an ordinary `#[hook_param(...)]`/`#[otxn_param(...)]`
field (covered earlier on this page) still never appears there.

## Why this prevents name/value mismatches

The loose `hook_param_exact::<T>(name)`/`otxn_param_exact::<T>(name)` calls
take `name` and `T` as two independent arguments — a typo or a copy-paste error
can pair the right name with the wrong type, or the wrong name with the right
type, and both compile fine as long as `T: FixedRead`. A
`#[hook_param(...)]`/`#[otxn_param(...)]` field removes that degree of freedom:
the field's declared name is permanently tied to exactly one value type, so
`TypedData.hook_param.config.get_or_default()` (read from a free function
outside the impl) and `self.hook_param.acct.get_required()` (read directly
inside `examples/09_state-foreign`'s `&self` entry) can never accidentally
decode one parameter's bytes as the other's struct shape — the compiler
resolves the return type from the field itself, with no
independently-chosen type left for a mismatch to hide in. This is the
identical safety property [Hook State](state.md)'s `#[state(...)]` fields
give the key/value side; see [Typed Data with Derives](typed-data.md) for
the underlying `ParamName`/`ParamValue` derives both build on, and [Hook
Chains](../concepts/chains.md) for how a field declared once here is
shared across every Hook entry in the same chain.
