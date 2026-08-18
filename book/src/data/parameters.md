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
turbofish. `examples/03_hook-params` uses exactly this for a compiled-in
default when the operator hasn't configured a minimum:

```rust,ignore
const MIN_PARAM: &[u8] = b"MIN";
const DEFAULT_MIN_DROPS: u64 = 1_000_000;

fn min_drops() -> u64 {
    hook_param_exact(MIN_PARAM)
        .map(u64::from_be_bytes)
        .unwrap_or(DEFAULT_MIN_DROPS)
}
```

`hook_param_exact`'s return type is inferred as `[u8; 8]` from the
`.map(u64::from_be_bytes)` call — no turbofish needed. Note the
`from_be_bytes`, not this crate's `FromBytes` trait: a raw parameter byte
buffer is whatever the caller who set it chose to write, conventionally
matching Xahau Binary's big-endian numeric encoding for a value like this,
the same convention [Reading the Originating Transaction](otxn.md)
describes for raw protocol fields. `.unwrap_or(DEFAULT_MIN_DROPS)`
collapses "not configured at all" and "configured with a value of the
wrong size" into the same fallback, without treating a malformed parameter
as a hard error. This tier needs no field declaration on the `#[hooks]`
struct at all — reach for it for a one-off read, or as the escape hatch
[Hook Chains](../concepts/chains.md#a-real-limit-typed-accessor-density-inside-one-entry)
covers when accessor density at one call site outgrows the nesting budget.

## Struct fields: `#[hook_param(...)]` / `#[otxn_param(...)]`

`hook_param_exact`/`otxn_param_exact` take the name and the value type `T`
as two *independent* arguments — nothing stops calling
`otxn_param_exact::<WrongType>(b"INS")` for a name/type combination that
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
malformed, falls back to the same value" — the pre-0.2 behavior of the
`hook_parameter!` macro's `get_value()` — write that explicitly at the call
site instead, the same way `examples/05_firewall` and
`examples/12_typed-data` both do:

```rust,ignore
fn blocked_account() -> Option<AccountId> {
    Firewall.blocked.get().ok().flatten()
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
let Ok(target) = self.acct.get_required() else {
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

    /// Per-invocation instruction (`INS`). Missing or malformed is a
    /// rollback, never a silent default.
    #[otxn_param(name = b"INS", required)]
    instruction: OtxnParam<Instruction>,
}
```

(from `examples/12_typed-data`.) `default`'s expression is a **runtime
fallback**, not baked into the deployed `SetHook` template as an installed
value — see [Per-Hook Attributes](../build/metadata.md) for why a
`HookParameters` entry has to be added to the template by hand if you want
a position to install with a concrete value at all. `config` and
`instruction` show the two ends of the same page's presence spectrum side
by side: `config` degrades gracefully when unconfigured, `instruction`
never does, and the field declaration itself (`default = ...` vs.
`required`) is where that policy lives, not scattered across call sites.

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

fn deposits_paused() -> bool {
    TypedData
        .admin_pause
        .at(ADMIN_PAUSE_NAME)
        .get_or_default()
        .map(|s| s.paused != 0)
        .unwrap_or(false)
}
```

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
near-zero cost of the plain `CFG`/`INS` tags used elsewhere in that same
hook.

## Why this prevents name/value mismatches

The loose `hook_param_exact::<T>(name)`/`otxn_param_exact::<T>(name)` calls
take `name` and `T` as two independent arguments — a typo or a copy-paste
error can pair the right name with the wrong type, or the wrong name with
the right type, and both compile fine as long as `T: FixedRead`. A
`#[hook_param(...)]`/`#[otxn_param(...)]` field removes that degree of
freedom: the field's declared name is permanently tied to exactly one value
type, so `TypedData.config.get_or_default()` (read from a free function
outside the impl) and `self.instruction.get_required()` (read directly
inside `examples/12_typed-data`'s `&self` entry) can never accidentally
decode one parameter's bytes as the other's struct shape — the compiler
resolves the return type from the field itself, with
no independently-chosen type left for a mismatch to hide in. This is the
identical safety property [Hook State](state.md)'s `#[state(...)]` fields
give the key/value side; see [Typed Data with Derives](typed-data.md) for
the underlying `ParamName`/`ParamValue` derives both build on, and [Hook
Chains](../concepts/chains.md) for how a field declared once here is
shared across every Hook entry in the same chain.
