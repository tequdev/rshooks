# Developer-experience roadmap (post-v0.2 TODO)

Status: planning notes — none of the items below are implemented or scheduled.

v0.2 established the `#[hooks]` chain declaration model (struct-declared
state/parameter schema, `&self` entry functions, per-index artifacts, SetHook
template generation). The items below are the remaining developer-experience
gaps, listed in recommended priority order.

**Method requirement for every item:** follow the same process that the `&self`
receiver change used — write a design note, build a measurement probe through
the real `rshooks build` pipeline, and only adopt what is **demonstrated to be
zero-cost** (or whose cost is measured, bounded and documented). Worst-case
execution (WCE), block-nesting depth (32-level guard-checker limit) and binary
size (64 KiB) are hard budgets on this target; an abstraction that looks free
in ordinary Rust can be expensive here. Two measured precedents to keep in
mind:

- Matching a specific `HookError` variant at a call site raised compiled block
  nesting from 24 to 70 (the raw-code-before-decode rule exists because of
  this).
- Dense typed-accessor call sites in one entry raised nesting from 22 to 63 in
  `examples/80_governance`; the raw-API escape hatch brought it back to 23.

Every feature below must therefore ship with probe numbers (WCE / size /
nesting, before vs. after) and must keep a raw-API escape hatch available.

## 1. Off-chain unit-test environment (priority 1) — IMPLEMENTED

A mock host (`rshooks_core::backend::HostBackend`, installed per-thread) now
lets `#[hooks]` chain entries and helpers run as plain native Rust under
`cargo test` — no wasm build, no node — with assertions on state, exit type,
and emitted transactions, via the `crates/rshooks-testenv` harness
(`TestEnv::invoke`). See the book's ["Off-Chain Unit
Tests"](../book/src/testing/unit-tests.md) chapter for the developer-facing
walkthrough and coverage table, and `docs/DESIGN.md` §7 for the shipped
mechanism. WCE/zero-cost impact is verified by `scripts/probe-testenv-parity.sh`
(Stage 6 of `.claude/design/TESTENV_IMPLEMENTATION.md`), which rebuilds every
example twice at the same commit — pristine vs. test-wired — and asserts
byte-for-byte identity of the shipped `.wasm` artifacts.

`.claude/design/TESTENV_PHASE2_DESIGN.md` (stages P2-A..P2-E) then extended
the mock backend from that initial subset to the entire `extern.h` Hook API
surface a hook can call (`_g` excepted), including `float_*`/`slot_*`/
`sto_*`/`util_*`/`keylet_*`, `hook_again`/`hook_skip`/`hook_param_set`, and
`TestEnv::invoke_cbak` for running a `#[cbak]` body directly — see the book
chapter's coverage table for the current, honest list of what remains
unmodeled.

## 2. Typed `Result`-based entry return values (priority 2) — IMPLEMENTED

A `#[hook]`/`#[cbak]` entry may now return `rshooks::exit::HookResult`
(`Result<Accept, Rollback>`) instead of `i64`, using `?` to propagate
failures — including `hook_errors!` enums, via an optional per-variant
`=> b"msg"` clause. Both forms coexist in the same `#[hooks] impl` block; the
`i64` form remains unchanged and canonical. See `docs/DESIGN.md` §10 for the
shipped mechanism, the book's [Accept, Rollback, and Errors §"Typed entry
returns: `HookResult`"](../book/src/concepts/errors.md#typed-entry-returns-hookresult)
for the developer-facing walkthrough, and
`examples/16_typed-results` for a worked example with measured numbers.

The risk this item flagged (raw-`i64` error codes, avoiding wide
enum-variant matches) is exactly what the design settled on:
`.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md`'s T-1 probe matrix (§5)
measured a small typed entry and a 15-call-site dense typed entry both at or
below their hand-written `accept!`/`rollback!` twins — provided every
`?`-called helper is `#[inline(always)]` — and confirmed that a
`?`-propagated `HookError` → `Rollback` conversion (going through
`HookError::code()`'s 45-arm re-encode match) is the one shape that *does*
regress (3.1x WCE), which is why that specific conversion is not offered at
all (`.map_err(..)` is the supported pattern instead). It ships as a
default-available, co-equal form, not opt-in — the probe found no case where
it needed to be gated behind a feature flag.

## 3. Typed entry arguments (dispatch layer) (priority 3)

Entry inputs today are read imperatively (otxn parameters, Invoke blob) inside
the body. A declarative form would move decoding to the boundary:

- Shape sketch: `#[hook(0)] fn transfer(&self, to: AccountId, amount: u64)` —
  the macro generates the decoding prologue that binds each argument from a
  declared source. The natural path is promoting the existing `#[otxn_param]`
  field declarations into argument bindings (same name/value byte contracts,
  same absence/decode-failure semantics as the `.get()` family).
- Open questions: how absence maps to entry behavior (reject vs. default vs.
  `Option<T>` parameter), and whether blob-carried payloads join the first
  version.
- WCE impact: the generated prologue is a fixed sequence of the same typed
  reads users write by hand today, so parity with hand-written code is the
  target and must be verified byte-for-byte by a probe. Argument decoding
  failures must not introduce new nesting (reuse the raw-code sentinel
  pattern).

## 4. High-level keyed-storage types (priority 4)

`#[state(key_by = ...)]` + `derive(HookKey)` requires designing the key layout
by hand. A map-style field type would remove that step:

- Shape sketch: `balances: Map<AccountId, u64>` as a field declaration — the
  macro derives the key encoding (declared prefix byte(s) + the key type's
  `ToBytes`) automatically; tuple keys (`Map<(A, B), V>`) compose the same
  way.
- Hard constraints that must stay visible rather than hidden: 32-byte encoded
  key ceiling, 256-byte value ceiling, and the nesting budget. A fully
  transparent map API risks encouraging exactly the dense accessor patterns
  that measured 63-deep nesting in `80_governance`; the design must keep the
  raw escape hatch (same declared bytes, raw calls) a first-class, documented
  companion rather than an afterthought.
- WCE impact: each map operation must compile to the same instruction sequence
  as today's `key_by` + `.at(...)` path (probe: byte parity). Prefix
  derivation must be compile-time only. Collision analysis between declared
  prefixes belongs in the macro diagnostics.

## 5. Toolchain commands (priority 5)

`rshooks build` already produces per-index artifacts, sidecars and a SetHook
template. Remaining gaps:

- `rshooks new` — project scaffolding for a chain crate (struct + impl +
  build wiring).
- `rshooks deploy` — take the generated `sethook.template.json`, fill
  account/namespace, and submit to a node (needs signing strategy and network
  configuration; the template's fail-closed/no-`Flags` default must carry
  over so replacement stays an explicit opt-in).
- Align the per-index metadata sidecar with an external, wallet/explorer
  consumable schema. This intersects the deliberately-deferred manifest work;
  revisit both together instead of inventing a third format.
- WCE impact: none (host-side tooling only) — the requirement here is that
  `deploy` never mutates artifacts, so the wasm that was validated is the
  wasm that ships.

## 6. Emission typing (priority 6)

Emitted transactions are built with `txn_template!`, but nothing ties a
template to the entry's `can_emit` declaration:

- Verify at build time (or macro time where possible) that only transaction
  types listed in `can_emit` have emission paths, and surface a diagnostic
  when a template's transaction type is absent from the declaration —
  strengthening the existing §6.3-style emit/can_emit/cbak consistency
  warnings from "emit is used at all" to "which type is emitted".
- Possible shape: typed emission handles derived from the declaration, so an
  undeclared emission does not compile.
- WCE impact: verification must stay in the build pipeline / macro layer;
  emission call sites themselves must remain byte-identical to today's
  `txn_template!` + `emit` sequence (probe for parity).
