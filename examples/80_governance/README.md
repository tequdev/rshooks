# governance

A behavior-equivalent Rust port of xahaud's genesis governance/reward
chain — [`hook/genesis/govern.c`](https://raw.githubusercontent.com/Xahau/xahaud/dev/hook/genesis/govern.c)
and [`hook/genesis/reward.c`](https://raw.githubusercontent.com/Xahau/xahaud/dev/hook/genesis/reward.c)
— declared as **one crate, two hooks**, via the `#[hooks]` multi-hook
chain model (`docs/MULTI_HOOK_STRUCT_DESIGN.md`). One shared state layout
backs both hooks, so `govern` and `reward` cannot drift out of sync with
each other.

## The chain model: one crate, two artifacts

On the real Xahau genesis account, `govern` and `reward` are installed
side by side, in that order: `Hooks[0] = govern`, `Hooks[1] = reward`.
Governance sets the reward rate/delay and the L1 seat table; reward reads
both to compute and distribute `ClaimReward` payouts. This crate declares
both as one `#[hooks]` struct plus one `#[hooks]` impl:

```rust
#[hooks(description = "20-seat L1/L2 governance and reward chain")]
pub struct Governance {
    #[state(key = b"MC")]
    member_count: State<u8>,
    #[state(key = b"RR")]
    reward_rate: State<XFL>,
    // ...
}

#[hooks]
impl Governance {
    #[hook(0, on = [Invoke], can_emit = [Invoke, SetHook])]
    fn govern(&self) -> i64 { /* ... */ }

    #[hook(1, on = [Invoke, ClaimReward], can_emit = [GenesisMint])]
    fn reward(&self) -> i64 { /* ... */ }
}
```

`rshooks build` compiles this crate **twice** — once per declared `#[hook]`
index, via `--cfg rshooks_entry="<i>"` — producing one independent wasm
binary per chain position, each containing only the code reachable from
its own entry point (govern's code never appears in `1.reward.wasm`, and
vice versa). A single build command therefore produces everything needed
to install both hooks:

```
rshooks build --manifest-path examples/80_governance/Cargo.toml --out examples/80_governance/out
```

writes, under `out/current/`:

| File | Contents |
|---|---|
| `0.govern.wasm` | governance hook, position 0 |
| `0.govern.metadata.json` | its sidecar: HookOn/HookCanEmit masks, HookName, HookHash, WCE, and the shared `"chain"` schema |
| `1.reward.wasm` | reward hook, position 1 |
| `1.reward.metadata.json` | its sidecar, same shape |
| `sethook.template.json` | a `SetHook` template covering **both** positions in one `Hooks` array (`Account`/`HookNamespace` left as placeholders) |
| `sethook.template.meta.json` | generation info: hook hashes, declared/gap positions, required amendments |

Measured this build: `0.govern.wasm` is 14851 bytes (WCE 44185, max
nesting 23/32); `1.reward.wasm` is 7710 bytes (WCE 13985, max nesting
22/32). Both stay well under the 65,535-byte SetHook `CreateCode` limit.

## Shared declaration: what's actually consolidated

Governance and reward genuinely share part of their on-ledger ABI, not
just "live on the same account":

| Field | Key | Written by | Read by |
|---|---|---|---|
| `member_count` | `"MC"` (2 bytes) | governance | governance only |
| `reward_rate` | `"RR"` (2 bytes) | governance (L1 table only) | **both** — reward falls back to its own compiled-in default when absent |
| `reward_delay` | `"RD"` (2 bytes) | governance (L1 table only) | **both** |
| `seat_forward` | 1-byte seat number | governance | **both** — reward looks up an active validator's seat's current member |
| `member_reverse` | 20-byte account | governance | **both** — reward looks up whether a validator's owning account currently holds a seat |

`"RR"`/`"RD"` (and the seat/member key shapes) are declared once, on
`Governance`, and both `#[hook]` entries reference the same fields — so
`govern` and `reward` cannot silently drift apart on these keys.

`member_count` and every vote/vote-count entry remain governance-only and
are not part of the shared story. Vote/vote-count keys in particular
(`src/keys.rs`) stay outside the declarative `#[state(..)]` field system
entirely — see the next section for why, and `src/keys.rs`'s own module
doc comment for the mechanics.

## A build-budget finding: typed accessors at high call-site density

`Governance`'s fields are fully declared (all five state entries, all four
hook parameters, both otxn parameters) — the struct is a complete,
type-checked schema of this chain's ABI, and its generated chain-carrier
JSON records that schema for tooling. **But the hot, call-site-dense code
paths in this crate (`setup`, `action_seat`, `push_l1_seat_entries`, and
`govern`'s own top-level reads) do not call those fields' `.get()`/
`.set()`/`.at()` accessors** — they read/write the identical key/name
bytes through the raw `state`/`state_set`/`otxn_param`/`hook_param_exact`
API instead.
`reward_rate`/`reward_delay` are a partial exception: `reward` *does* use
the typed `.reward_rate`/`.reward_delay` accessors, at their only 2 call
sites — both reads, `self.reward_rate`/`self.reward_delay` under
`reward`'s `&self` receiver. Governance's own setup still writes the same
`"RR"`/`"RD"` keys through raw `state_set` (`setup_initial_reward_rate_and_delay`
in `src/lib.rs`), for the same call-site-density reason as `setup`'s other
raw calls above — `govern`'s dense paths have no field accesses of their
own (see above), so its mandatory `&self` receiver goes unused there.

This is a real, measured build constraint, not a style preference. Every
layer of the typed accessor chain (`State::at` -> `StateEntry::get` ->
`state::state_get` -> `decode_read` -> `res`, all `#[inline(always)]`) is
zero-cost *at the Rust level*, but `rshooks-build`'s Guard-type (api-version
0) pipeline force-inlines every reachable function into one `hook()` body
regardless of Rust-level `#[inline(never)]` (`docs/DESIGN.md` §6.2b) —
`#[inline(never)]` only isolates a function's *own* internal branch
structure from a caller's during LLVM's stackifier pass; it does not
exempt that function from the pipeline's later mechanical inlining, nor
does it help when the *same* function already contains many sequential
typed-accessor call sites (the isolation doesn't reduce nesting *within*
that one function). Measured on this crate: with `setup`/`action_seat`/
`push_l1_seat_entries`'s combined ~15 typed-accessor call sites, the
`govern` entry's post-unnest nesting was **63** (limit: 32) — even after
extracting `govern`'s own four top-level reads into separate
`#[inline(never)]` helpers, which made no measurable difference, confirming
the cost comes from call-site density *within* whichever function holds
them, not cross-function fusion. Reverting those dense paths to raw calls
(this crate's current state) brought `govern` down to nesting 23 and
`reward` to 22.

**This is flagged as a candidate `rshooks`/`decl.rs` finding** for the
orchestrator/library maintainers: the declarative `#[state(..)]`/
`#[hook_param(..)]`/`#[otxn_param(..)]` field API, in its current
implementation, does not appear practical to use at high call-site density
in a single Guard-type (api-version 0) hook — every example migrated so
far (`01`/`02`/`03`/`12`) has few enough declared-field accesses per hook
to stay comfortably under budget; `governance` is the first crate dense
enough to hit the ceiling. A hook with `governance`'s call-site density
either needs to keep using raw calls at those sites (as this crate now
does) or the library needs a shallower/flatter accessor implementation.

## Behavior equivalence and differences from govern.c/reward.c

This crate's `"MC"`-state presence check precisely matches govern.c's own
`== DOESNT_EXIST` check: `Err(_)` from `state_u64` selects the setup path,
with no externally observable difference, since every other
`state_u64("MC")` failure is already unreachable for a well-formed table.

## Testing

`mise.toml`'s `build-examples` task and `e2e/scripts/copy-wasm.mjs` build
this crate and stage its two artifacts as `govern.wasm`/`reward.wasm`
under `e2e/build/`, from `out/current/0.govern.wasm`/`1.reward.wasm` — the
same basenames `e2e/test/govern.test.ts`/`reward.test.ts` already expect,
so those e2e tests run unchanged against the consolidated crate.
