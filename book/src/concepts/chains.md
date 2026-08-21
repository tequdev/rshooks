# Hook Chains

[Anatomy of a Hook](anatomy.md) covered a `#[hooks]` struct/impl pair
declaring a single Hook. Nothing about that shape is actually limited to
one entry: the same struct can carry more than one `#[hook(<index>, ...)]`
(and matching `#[cbak(<index>)]`) in its `impl` block, each with its own
index. This page covers what changes when it does — how index maps to
on-ledger position, why a shared struct is the model's biggest win, what a
multi-Hook build actually produces, the `SetHook` template's exact
semantics, and a real, measured limit worth knowing about before you lean
on this model too hard in one entry.

## One struct, one chain

A single `#[hooks]` struct plus its one `#[hooks]` impl is this crate's
**entire chain declaration** — not "a Hook," but everything this crate
contributes to an account's Hook chain, across however many indices it
declares. There is deliberately no way to have two chains in one crate:
each `#[hooks]` struct generates a fixed-name linker symbol, so a second
one collides and fails to link. One crate, one chain.

## Index: chain position, not just an artifact ID

`#[hook(<index>, ...)]`'s leading integer means two things at once:

1. **Which artifact this entry becomes** — its own wasm, its own
   metadata sidecar, built and validated independently of every other
   entry in the crate (see "What a chain build produces," below).
2. **Where it sits in the account's `Hooks` array** — the same array a
   `SetHook` transaction installs into. Index `0` is `Hooks[0]`, index `3`
   is `Hooks[3]`, and so on.

Valid indices are `0..=9` (a `SetHook` transaction's `Hooks` array holds at
most 10 entries), and **gaps are allowed**: a crate can declare only `0`
and `2`, leaving position `1` for something else entirely (a different
crate's Hook, installed and managed separately). What gaps mean for the
generated template is covered below.

`#[cbak(<index>)]` pairs with a `#[hook]` at the **same** index — it
doesn't take its own name or trigger, just the index it settles for. An
index can have a hook with no cbak; an index with a cbak but no hook is a
compile error (there'd be nothing for it to settle), and a crate declaring
zero hooks at all is also a compile error — a chain with nothing in it
isn't a useful chain.

Because index is written in the source, changing which position a Hook
occupies is a source change, not a deployment-time choice: reviewing a diff
to `#[hook(1, ...)]` is reviewing exactly what's moving where in the
account's chain. If you need to install the same compiled wasm at a
different position without touching source, the generated template's
`Hooks` array (below) can still be reordered by hand before submission —
the wasm itself doesn't encode its own position.

## The shared schema: why this is the model's biggest win

Every field on the `#[hooks]` struct — every `#[state]`, `#[hook_param]`,
`#[otxn_param]` — is declared exactly once and can be referenced from
**any** entry in the same `impl` block. This is the actual payoff of the
model: state or parameters two Hooks in the same chain both touch get one
Rust-level declaration, type-checked once, instead of one copy per crate
silently drifting out of sync.

`examples/80_governance` is the worked example — a Rust port of xahaud's
genesis `govern`/`reward` pair, which on the real network are installed
side by side, `Hooks[0] = govern` and `Hooks[1] = reward`. The two entries
share a state layout (the reward rate/delay, and the seat table) that
neither one exclusively owns — declaring them as one chain gives that
shared layout a single Rust-level declaration, instead of leaving it to be
duplicated (and potentially drift) across two independent crates. As one
chain:

```rust,ignore
#[hooks(description = "20-seat L1/L2 governance and reward chain")]
pub struct Governance {
    /// L1 reward rate. Written by governance; read by both governance
    /// and reward.
    #[state(key = b"RR")]
    reward_rate: State<XFL>,

    /// L1 reward delay (seconds). Same story as `reward_rate`.
    #[state(key = b"RD")]
    reward_delay: State<XFL>,

    // ... member_count, seat_forward, member_reverse, and this chain's
    // hook parameters, all declared once here.
}

#[hooks]
impl Governance {
    #[hook(0, name = "govern", on = [Invoke], can_emit = [Invoke, SetHook])]
    fn govern(&self) -> HookResult { /* ... reads and writes self.reward_rate/self.reward_delay */ }

    #[hook(1, name = "reward", on = [Invoke, ClaimReward], can_emit = [GenesisMint])]
    fn reward(&self) -> HookResult { /* ... reads self.reward_rate/self.reward_delay */ }
}
```

Both entries declare a `&self` receiver and reference
`self.reward_rate`/`self.reward_delay` directly — there is exactly one Rust
type for that state entry, so `govern`'s write and `reward`'s read can
never silently disagree about the key's shape or the value's layout. (The
real `examples/80_governance` crate's dense `govern`/setup path writes
`reward_rate`/`reward_delay` through the raw API instead — see "A real
limit," below, for why — while `reward`'s own two reads still go through
the typed `self.reward_rate`/`self.reward_delay` accessors shown here; this
sketch shows the model at its cleanest.)

One nuance worth being precise about: **the struct shares the schema, not
the values.** Both entries read/write the identical on-ledger state key —
that part genuinely is shared — but a Hook *parameter* declared on the
struct is installed independently per index. `Governance`'s `config` field
(if it had one) could be installed with one value at index `0` and a
different value at index `1`; the struct only guarantees both entries agree
on the parameter's *shape*, not that they were configured identically.

## What a chain build produces

`rshooks build` compiles a multi-Hook crate once per declared index (see
[Building a Hook](../getting-started/building.md) for the discovery-plus-
per-index pipeline), producing, for `Governance` above:

```text
out/current/
  0.govern.wasm
  0.govern.metadata.json
  1.reward.wasm
  1.reward.metadata.json
  sethook.template.json
  sethook.template.meta.json
```

Each `<index>.<fn>.wasm` is a **complete, independent** wasm module: only
the code reachable from that one entry's own `#[hook]`/`#[cbak]` functions
gets compiled in — `govern`'s logic never appears in `1.reward.wasm`, and
vice versa. The direct consequence is that the 65,535-byte SetHook size
limit, and the 32-level structural nesting limit the guard checker
enforces, apply **per index**, not to the crate as a whole. A chain of ten
entries effectively has ten times the budget of one entry, split across ten
independent artifacts, rather than one shared pool.

The output directory itself is generation-numbered
(`out/gen-<N>/`, with `out/current` a symlink to the latest complete,
validated one) so a build in progress, or one that fails partway through,
never leaves `current` pointing at a half-written result.

## The `SetHook` template: an owned-position patch, not a full chain

`sethook.template.json` is a ready-to-edit `SetHook` transaction covering
every index this crate declares — but it is deliberately **not** a
declarative statement of what the whole account's chain should look like.
It's a **patch over the positions this crate owns**:

```json
{
  "TransactionType": "SetHook",
  "Account": "<ACCOUNT>",
  "Hooks": [
    { "Hook": { "CreateCode": "<hex of 0.govern.wasm>", "...": "..." } },
    { "Hook": { "CreateCode": "<hex of 1.reward.wasm>", "...": "..." } }
  ]
}
```

The `Hooks` array is exactly as long as the highest declared index plus
one — `0..=max`, no padding beyond it. Every **gap** (a position with no
declared entry, but below the highest one) becomes an empty `{"Hook": {}}`
object: SetHook's own no-op spelling for "leave whatever's at this position
alone." That's a deliberate, load-bearing distinction — `{"Hook": {}}`
means "don't touch this slot," not "this slot is empty." Two things follow
directly from that:

- Submitting this template **never removes or overwrites** a Hook at a
  gap position, or at any position past the array's end, even if one is
  already installed there.
- The template therefore does **not** guarantee the account's chain
  matches this crate's source after submission — only that *this crate's
  own declared positions* end up matching. If something else is installed
  at a gap or beyond, it's still there afterward; reconciling the whole
  account's chain against source is outside what a generated template
  does.

The generated template is also **fail-closed by default**: it carries no
`Flags` field at all, so submitting it as-is only succeeds against
*currently-empty* declared positions — it will not silently overwrite an
existing Hook. Pass `--override` at build time to add `hsfOVERRIDE` to
every declared (non-gap) position, permitting replacement; gap objects
never receive `Flags`, since adding one would turn a no-op into a real
operation.

`Account` and `HookNamespace` are left as placeholders (`"<ACCOUNT>"`,
`"<NAMESPACE>"`) unless you pass `--account`/`--namespace` at build time —
there's no way for the build to know which account this template is meant
for. `sethook.template.meta.json`, alongside it, is generation provenance
(not itself part of the transaction): hook hashes, the declared/gap
position lists, and the amendment set the declared fields require. Both
files' exact shape — and the full per-entry attribute grammar that drives
them (`name`, `on`, `can_emit`, and so on) — are covered in [Per-Hook
Attributes](../build/metadata.md).

## A real limit: typed-accessor density inside one entry

This is a genuine, measured constraint, not a style preference, and it's
the main reason to actually read this section rather than skim it.

Every layer the typed `#[state(..)]`/`#[hook_param(..)]`/`#[otxn_param(..)]`
accessors go through — `.at(..)` → `.get()` → the underlying host call → 
decode — is zero-cost *at the Rust level* (every layer is
`#[inline(always)]`). But the Guard-type build pipeline's cleaner stage
force-inlines everything reachable into one `hook()` body, regardless of
Rust-level inlining hints, and then has to fit the result under the host's
32-level structural nesting limit. A single entry with many sequential
typed-accessor call sites in the same function accumulates nesting from
each one — `#[inline(never)]` on a helper doesn't exempt its own call sites
from this, since the constraint is call-site density within whatever
function ends up holding them, not which function that happens to be.

`examples/80_governance` hit this directly: `govern`'s setup path, with
roughly fifteen typed-accessor call sites across a few helper functions,
compiled to a post-cleaning nesting depth of **63** — the limit is **32**.
Reverting exactly those dense call sites to the underlying raw API (same
section below) brought it down to **23**.

Every other example in this book stays comfortably under budget — this
shows up specifically at `governance`'s call-site density, in one
Guard-type entry. It's worth knowing about before you assume the typed
layer scales to an arbitrarily dense entry, not something to preemptively
work around in an ordinary hook.

### The escape hatch: the raw API, same declared bytes

When one entry's typed-accessor density pushes past budget, the fix isn't
to abandon the struct's shared declaration — it's to keep the declaration
(so the schema is still centrally documented and type-checked) but read or
write the *same* key/name bytes through the lower-level free functions at
just the dense call sites:

```rust,ignore
// Governance.reward_rate's own declared key is b"RR" — this hits the
// identical ledger slot, just without going through the typed accessor.
if state_set(value, b"RR").is_err() {
    GovernError::AssertionFailed.nope(b"Governance: Assertion failed.");
}
```

Because the raw call uses the field's own declared literal, it addresses
exactly the same on-ledger entry the typed accessor would — this is a
call-site choice about which API shape to go through, not a second,
diverging declaration. See [Hook State](../data/state.md) and [Hook and
Transaction Parameters](../data/parameters.md) for the raw
`state`/`state_set`/`hook_param`/`otxn_param` layer this falls back to, and
`examples/80_governance`'s own `README.md` for the full measured numbers
behind this section.

## Where to go next

- [Per-Hook Attributes](../build/metadata.md) is the complete grammar for
  `name`/`on`/`on_incoming`+`on_outgoing`/`can_emit`/`description`, plus the
  exact shape of the generated sidecar and `SetHook` template JSON.
- [Hook State](../data/state.md) and [Hook and Transaction
  Parameters](../data/parameters.md) cover the `#[state]`/`#[hook_param]`/
  `#[otxn_param]` field declarations this page assumes.
- [The `rshooks` CLI](../build/cli.md) covers `--account`/`--namespace`/
  `--override` and every other build flag in full.
