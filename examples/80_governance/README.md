# governance

A behavior-equivalent Rust port of xahaud's genesis governance/reward
chain — [`hook/genesis/govern.c`](https://raw.githubusercontent.com/Xahau/xahaud/dev/hook/genesis/govern.c)
and [`hook/genesis/reward.c`](https://raw.githubusercontent.com/Xahau/xahaud/dev/hook/genesis/reward.c)
— declared as one crate, two hooks, via the `#[hooks]` multi-hook chain
model. See [Hook Chains](../../book/src/concepts/chains.md) for the model
itself (this crate is its worked example, quoted directly there) and its
["A real limit" section](../../book/src/concepts/chains.md#a-real-limit-typed-accessor-density-inside-one-entry)
for the typed-accessor density constraint this crate hit and worked around.

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
`member_count` and every vote/vote-count entry remain governance-only.
Vote/vote-count keys (`src/keys.rs`) stay outside the declarative
`#[state(..)]` field system entirely — see that module's own doc comment
for the mechanics.

## Typed accessors at high call-site density

`Governance`'s fields are fully declared, but the hot, call-site-dense
code paths (`setup`, `action_seat`, `push_l1_seat_entries`, and `govern`'s
own top-level reads) read/write the identical key/name bytes through the
raw `state`/`state_set`/`otxn_param`/`hook_param_exact` API instead of
those fields' typed accessors: at this crate's call-site density, the
typed accessors' force-inlined nesting pushed `govern`'s setup path past
the Hook API's 32-level structural limit, and reverting just those dense
call sites to the raw API (same declared key/name bytes, so the schema
stays centrally documented) is what brought it back under budget. Splitting
the same reads into separate `#[inline(never)]` helpers was tried first
and made no difference — the nesting cost comes from call-site density in
whichever function ends up holding them after the build pipeline's
force-inlining, not from which function they started in. See the [book's
"A real limit"
section](../../book/src/concepts/chains.md#a-real-limit-typed-accessor-density-inside-one-entry)
for the general mechanism and `metrics.json` for the current numbers.
`reward_rate`/`reward_delay` are a partial exception: `reward` uses the
typed accessors at its own two (read-only) call sites; governance's setup
still writes the same keys through raw `state_set`, for the same
call-site-density reason as `setup`'s other raw calls.

## Behavior equivalence

This crate's `"MC"`-state presence check precisely matches govern.c's own
`== DOESNT_EXIST` check: `Err(_)` from `state_u64` selects the setup path,
with no externally observable difference, since every other
`state_u64("MC")` failure is already unreachable for a well-formed table.

## Build

```sh
rshooks build --manifest-path examples/80_governance/Cargo.toml --out examples/80_governance/out
```

Produces `0.govern.wasm`/`1.reward.wasm` plus their sidecars and one
`sethook.template.json` covering both positions — see [Hook
Chains](../../book/src/concepts/chains.md#what-a-chain-build-produces) for
the exact output shape. Current size, WCE, and max nesting for each
artifact live in [`metrics.json`](./metrics.json).

## Testing

`mise.toml`'s `build-examples` task and `e2e/scripts/copy-wasm.mjs` build
this crate and stage its two artifacts as `govern.wasm`/`reward.wasm`
under `e2e/build/`, from `out/current/0.govern.wasm`/`1.reward.wasm` — the
same basenames `e2e/test/govern.test.ts`/`reward.test.ts` already expect,
so those e2e tests run unchanged against the consolidated crate.
