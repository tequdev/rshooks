# rshooks — Rust Library & Toolchain for Xahau Hooks

Status: REVIEWED — rework findings from the external design review (Codex
gpt-5.5, reasoning effort high, 2026-07-23) have been incorporated; see §11.
Author: design by Claude (Fable 5); implementation delegated per phase
Date: 2026-07-23

> **Status note (post-0.2):** this document predates the `#[hooks]` chain
> model. In particular, §5.4 ("Macros & entry point") and §6.6 ("Build-only
> Hook metadata") describe the pre-0.2 architecture — the declaration
> macros (`metadata!`, `hook_state!`, `hook_parameter!`, `otxn_parameter!`)
> and top-level `#[hook]`/`#[cbak]` entry points they cover have been
> removed and superseded by the `#[hooks]` struct/impl chain model
> specified in [`docs/MULTI_HOOK_STRUCT_DESIGN.md`](MULTI_HOOK_STRUCT_DESIGN.md).
> Everything else below — the core API wrappers, XFL, the build pipeline
> passes, and the guard machinery — still applies.

## 1. Goals

Provide a Rust monorepo for developing Xahau Hooks (WebAssembly smart
contracts) end to end:

1. **rshooks-core** — zero-logic FFI layer: raw Hook API declarations and every
   constant from the xahaud `hook/` headers (`error.h`, `extern.h`,
   `ls_flags.h`, `sfcodes.h`, `tts.h`, `tx_flags.h`, keylet/compare constants
   from `hookapi.h`), translated 1:1 into Rust.
2. **rshooks** — ergonomic, Rust-idiomatic wrapper over rshooks-core
   (`Result`-based APIs, typed buffers, XFL type, guard/trace macros, panic
   handler). This is the crate Hook developers import.
3. **rshooks-build** — CLI that turns a Rust crate into a SetHook-valid WASM:
   drives `cargo build --target wasm32v1-none`, then performs the
   hook-cleaner and guard-checker steps natively in Rust.
4. **examples** — multiple working Hooks written with rshooks, buildable
   with rshooks-build.

### Non-goals (v1)

- Publishing to crates.io (names/ownership decided later; `publish = false`).
- Gas-type hook (HookApiVersion 1) *ergonomics*. The pipeline accepts
  `--api-version 1` (skips guard handling) but rshooks v1 targets
  Guard-type hooks.
- Deployment tooling (SetHook submission, faucet, networks). Out of scope;
  rshooks-build stops at a valid `.wasm` plus a fee estimate.
- WAT round-tripping, debugger, simulator.

## 2. Constraints that shape the design

These come from xahaud's SetHook validation (`SetHook.cpp`,
`validateHookSetEntry`) and prior experience building the same toolchain:

- **C1. Export set**: the WASM may export only `hook` (and optionally
  `cbak`), both `(func (param i32) (result i64))`. Rust `cdylib` output also
  exports `memory` — it must be stripped, or SetHook fails `temMALFORMED`.
- **C2. Guards**: for API version 0, every `loop` must begin with the exact
  instruction sequence `i32.const <id>; i32.const <maxiter>; call $_g`
  (result dropped). xahaud statically computes a worst-case instruction
  count from these; missing guards ⇒ rejection.
- **C3. Instruction set**: WASM 1.0 MVP only. No bulk-memory, sign-ext,
  reference types, SIMD; **no floating-point opcodes at all**. Rust target
  `wasm32v1-none` guarantees the MVP feature set but not float-freedom —
  float ops must never appear in source (XFL math is done via host calls).
- **C4. Imports**: only the documented Hook API functions plus `_g`, all
  from module `env`. Anything else ⇒ rejection.
- **C5. No recursion**: the static instruction-count analysis requires an
  acyclic call graph.
- **C6. Size**: ≤ 65,535 bytes; SetHook fee ≈ 5000 drops/byte, so every
  byte matters. No allocator, no `core::fmt`, no panic machinery.
  `examples/Cargo.toml`'s workspace `[profile.release]` uses
  `opt-level = 3`, not `"z"` — this looks like it trades off against the
  "every byte matters" principle above, so the reasoning is recorded here:
  - **(a) Primary motivation — DX, not speed.** `wasm32v1-none`'s LLVM
    backend only lowers a local `[0u8; N]` zero-init to plain inlined
    stores up to a fixed byte threshold; above it, LLVM instead emits a
    call to a `compiler_builtins`-style `memset` — an unguarded `loop`
    that `rshooks-build`'s guard pass hard-rejects by default (§6.3). That
    threshold is **32 bytes** at `opt-level = "z"`/`"s"` but **64 bytes**
    at `opt-level = 1`/`2`/`3` (measured directly against this repo's
    pinned toolchain by bisecting the exact byte boundary in both
    directions). Any local zero-init scratch buffer in the 33..=64 byte
    range — e.g. a 34-byte `Keylet` — becomes safe by construction at
    `opt-level = 3` with no `--auto-guard`, no hand-sized `maxiter`, and
    no `static`-buffer workaround needed. This — not raw execution speed,
    which the Hook API's static WCE metering does not reward — is why
    `opt-level = 3` is the workspace default: it removes an entire class
    of "clean Rust source, unguarded-loop build failure" surprises for
    hook authors before they ever have to reach for a toolchain flag.
  - **(b) Measured net effect, across all 13 examples** (rebuilt on the
    exact toolchain this repo pins): worst-case instruction count (WCE)
    improved in 6, stayed byte-for-byte unchanged in 3 (`02_state-counter`,
    `07_xfl-math`, `81_govern` — their compiled output didn't move at all),
    and increased only slightly (at most +12 instructions / +7%) in the
    remaining 4 (`01_accept-all`, `05_firewall`, `08_slot-ledger`,
    `10_emit-txn`) — no example regressed by more than a low double-digit
    instruction count. One example stands out as a large outlier
    (`06_guard-patterns`, whose whole point is demonstrating small
    `guard!`-bounded loops: `opt-level = 3` unrolls them, so WCE dropped
    ~54% while size grew ~109% — see that example's own README for the
    exact before/after table). Every example stayed comfortably under the
    65,535-byte limit and `rshooks check` (no unguarded loops, no
    nesting-limit violations) passed for all of them. The one-time
    `SetHook` fee delta (`bytes × 5000` drops) this causes per example is
    small in absolute terms even where size grew. `mise run build-examples`
    prints the authoritative current numbers for any given toolchain
    version; do not treat the specific figures here as pinned.
  - **(c) The threshold moves, it does not disappear.** `opt-level = 3`
    only raises the memset-inlining ceiling from 32 to 64 bytes — it does
    not remove it. A local zero-init scratch buffer larger than 64 bytes
    (e.g. `EMIT_DETAILS_MAX_LEN = 138`) still lowers to the same
    unguarded-loop `memset` call regardless of this setting, and still
    needs the `static`/`HookStatic` idiom (§6.3's "static-buffer idiom",
    `examples/README.md`'s "Statics for templates and large buffers")
    rather than relying on `opt-level` alone. `rshooks-build`'s
    `--auto-guard` escape hatch (§6.3) remains available, and remains the
    wrong default for the reasons given there, independent of this
    setting.
  - Raising `-C llvm-args`-level memset/memcpy/memmove store thresholds
    directly (rather than the whole crate's `opt-level`) was investigated
    and found to have **no effect at all** on `wasm32v1-none`:
    `--max-store-memset[-Os]`/`--max-store-memcpy[-Os]`/
    `--max-store-memmove[-Os]` are accepted by rustc but produced
    byte-identical output in both directions (raised to force everything
    inline, lowered to force everything into a libcall) — WebAssembly's
    `TargetLowering` hardcodes these thresholds and ignores the global
    LLVM `cl::opt`. `opt-level` is the only lever this toolchain actually
    exposes for this threshold.
  - **Full before/after table** (`opt-level = "z"` → `3`, all 13 examples,
    worst-case instructions / size in bytes, this repo's pinned toolchain):

    | example | WCE before → after | size before → after |
    |---|---:|---:|
    | `01_accept-all` | 14 → 15 | 173 → 174 |
    | `02_state-counter` | 58 → 58 | 374 → 374 |
    | `03_hook-params` | 178 → 177 | 616 → 613 |
    | `04_errors` | 276 → 200 | 910 → 734 |
    | `05_firewall` | 134 → 135 | 504 → 505 |
    | `06_guard-patterns` | 1341 → 615 | 775 → 1621 |
    | `07_xfl-math` | 357 → 357 | 1635 → 1635 |
    | `08_slot-ledger` | 197 → 209 | 952 → 965 |
    | `09_state-foreign` | 152 → 145 | 707 → 689 |
    | `10_emit-txn` | 322 → 331 | 1253 → 1272 |
    | `14_account-id-macro` | 365 → 294 | 1512 → 1391 |
    | `80_reward` | 13698 → 13680 | 7205 → 7175 |
    | `81_govern` | 44560 → 44560 | 14373 → 14373 |

    Every row's "after" build also passed `rshooks check` (no
    unguarded loops, nesting depth within the 32-level limit) and the full
    live e2e suite (`mise run e2e:node-up`, `pnpm --dir e2e test`) against
    a standalone Xahau node, asserting each example's live
    `HookInstructionCount` against its (now-updated) documented bound.
- **C7. Panic machinery is poison**: slice bounds checks pull in panic paths
  that add functions/calls and have historically broken validation.
  rshooks must be panic-free by construction (no indexing that can
  panic in release; caller-provided buffers; `Result` everywhere).
- **C8. Byte-exact post-processing**: the post-processor must re-encode the
  module without disturbing the guard byte pattern. Use
  `wasmparser` + `wasm-encoder` (raw section copy where possible);
  **walrus is deliberately avoided** — its IR round-trip does not preserve
  instruction sequences byte-exactly.

## 3. Repository layout

```
rshooks/
├── Cargo.toml                # workspace: crates/* (examples excluded)
├── rust-toolchain.toml       # stable channel + wasm32v1-none target
├── rustfmt.toml
├── mise.toml                 # fmt / lint / test / build-examples tasks
├── .gitignore                # target/, out/, *.wasm artifacts
├── docs/
│   └── DESIGN.md             # this file
├── crates/
│   ├── rshooks-core/           # no_std, FFI decls + constants, no logic
│   ├── rshooks-macros/         # std, proc-macro crate (#[hook]/#[cbak], txn_template! internals)
│   ├── rshooks/            # no_std, idiomatic wrapper (depends: rshooks-core, rshooks-macros)
│   ├── rshooks-build/          # std, bin+lib CLI (clap, wasmparser, wasm-encoder)
│   ├── rshooks-testenv/        # std, dev-dependency: off-chain unit-test harness (§7)
│   └── xtask/                # std, bin CLI: header → rshooks-core codegen
└── examples/
    ├── Cargo.toml            # SEPARATE workspace (no_std cdylibs)
    ├── 01_accept-all/        # numbered = suggested reading order
    ├── 02_state-counter/     # (package names are unprefixed - Cargo
    ├── 03_hook-params/       # package names can't start with a digit)
    ├── 04_errors/
    ├── 05_firewall/
    ├── 06_guard-patterns/
    ├── 07_xfl-math/
    ├── 08_slot-ledger/
    ├── 09_state-foreign/
    └── 10_emit-txn/
```

- Root workspace members: `crates/*` only. `examples/` is its own workspace:
  its crates are `no_std` cdylibs with hook-specific release profiles that
  must not leak into host crates, and they don't build for host targets.
- Edition 2024, `rust-version = "1.85"` (wasm32v1-none is stable ≥ 1.84). A
  stable toolchain is pinned via `rust-toolchain.toml` (currently `1.89.0`,
  matching `mise.toml`'s `[tools] rust` pin — see §5.5 for why no nightly
  feature is needed: `rshooks-macros`, a small hand-rolled `proc_macro` crate,
  covers what `${concat(...)}` used to); `rust-version` still tracks the
  language edition floor, not the exact pinned toolchain.
- All crates `publish = false` for now.
- All comments, docs, and identifiers in English.

## 4. rshooks-core

`#![no_std]`, zero dependencies, zero logic. A faithful, mechanical
translation of the headers. Layout:

```
src/
├── lib.rs        # crate docs, module wiring, re-exports
├── api.rs        # extern "C" declarations (the 60+ Hook API fns + _g)
├── error.rs      # error.h     → pub const SUCCESS: i64 = 0; OUT_OF_BOUNDS = -1; ...
├── sfcodes.rs    # sfcodes.h   → pub const sfAccount: u32 = ...; (325 consts)
├── tts.rs        # tts.h       → pub const ttPAYMENT: u16 = 0; ...
├── ls_flags.rs   # ls_flags.h  → pub const lsfGlobalFreeze: u32 = ...;
├── tx_flags.rs   # tx_flags.h  → pub const tfFullyCanonicalSig: u32 = ...;
└── consts.rs     # hookapi.h + macro.h constant-like defines
```

`consts.rs` covers every *constant-like* define from `hookapi.h` and
`macro.h`: `KEYLET_*` (1–26), `COMPARE_*`, `tfCANONICAL`, the `atACCOUNT`
family (amount/account offset constants), and the `amAMOUNT` family.
Function-like macros in `macro.h` (`SBUF`, `BUFFER_EQUAL`, …) are C
conveniences and are NOT ported here — their roles are covered by rshooks.

Rules:

- **Names are kept verbatim** (`sfAccount`, `ttPAYMENT`, `lsfGlobalFreeze`,
  `OUT_OF_BOUNDS`) under `#![allow(non_upper_case_globals)]` so code can be
  grepped against C hooks and the official docs. No renaming, no typing
  cleverness at this layer.
- Types: error codes `i64` (they are compared against Hook API `i64`
  returns); `sfcodes` `u32`; `tts` `u16` (matching `otxn_type`'s and
  rshooks's `TxType::code()`'s width — the only one of these four
  constant families whose width was picked to match a specific consumer
  rather than "the field is conventionally `u32`"); flags `u32`.
- The extern block mirrors `extern.h` exactly — `read_ptr`/`read_len` style
  `u32` parameters, `i64` returns:

```rust
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    pub fn _g(guard_id: u32, maxiter: u32) -> i32;
    pub fn accept(read_ptr: u32, read_len: u32, error_code: i64) -> i64;
    pub fn state(write_ptr: u32, write_len: u32, kread_ptr: u32, kread_len: u32) -> i64;
    // ... every function from extern.h, in extern.h order
}
```

- **Host builds**: the extern block is `#[cfg(target_arch = "wasm32")]`.
  For other targets, same-signature deterministic stub fns are provided so
  rshooks and its docs/tests compile *and run* on the host: every stub
  returns `NOT_IMPLEMENTED` (no panicking `unimplemented!()`). A richer
  feature-gated mock host is possible later without changing this surface.
- Each declaration carries a one-line doc comment naming the C prototype;
  a header comment records the upstream source
  (`Xahau/xahaud`, branch `release`, `hook/<file>.h`) so re-generation diffs
  are reviewable.
- **The source headers are vendored, not just referenced**: all eight
  `hook/*.h` files live verbatim in `crates/rshooks-core/vendor/xahaud-hook/`
  (own `VENDOR.md` + `SHA256SUMS`), synced by the same
  `scripts/sync-vendor.sh` / weekly drift workflow as the guard checker
  (6.5). **Parity tests** in rshooks-core parse the vendored headers at test
  time (C `#define`/enum extraction with a tiny shift-add expression
  evaluator, and `extern.h` prototype parsing) and compare complete
  name/value/signature sets against the Rust translation — so an upstream
  header change first fails the drift workflow, and after re-syncing, the
  parity tests fail until the Rust side is updated to match. The
  translation cannot silently rot.
- **The translation itself is generated**: `cargo xtask gen-core`
  (crates/xtask) parses the vendored headers and emits all of rshooks-core's
  translated sources (`error.rs`, `tts.rs`, `sfcodes.rs`, `ls_flags.rs`,
  `tx_flags.rs`, `consts.rs`, `api.rs` — everything except the hand-written
  `lib.rs`), each carrying an `@generated` marker. The same run also emits
  one file outside rshooks-core: rshooks's `tx_type.rs` (§5's `TxType`
  enum), from the identical parsed `tts.h` data `tts.rs` renders as raw
  constants — a typed mirror one layer up, not a header translation, but
  still fully mechanical (every variant name is a pure function of its
  `tt*` name), so it is generated rather than hand-maintained the same way.
  `gen-core --check` verifies every checked-in generated file (rshooks-core's
  and rshooks's `tx_type.rs` alike) matches regeneration (wired into CI),
  so the full sync flow is: `scripts/sync-vendor.sh` → `cargo xtask
  gen-core` → tests → commit. The xtask parser is deliberately independent
  from the parity tests' parser — the parity tests are the generator's
  correctness oracle, so they must not share code.
- **A second vendor group carries the protocol *formats***: xahaud's
  `sfields.macro`, `transactions.macro`, `ledger_entries.macro` and the
  three `*Formats.cpp` files live verbatim in
  `crates/rshooks-core/vendor/xahaud-protocol/` (own `VENDOR.md` +
  `SHA256SUMS`, same sync script and drift workflow). The same `gen-core`
  run parses them into a checked-in, versioned
  `crates/rshooks-core/protocol_formats.json` — the declared shape of every
  transaction, ledger entry and inner object, with each field's presence and
  wire code — under `--check` like every other generated file, and with its
  own parity test. The parse is cross-validated against the vendored
  `sfcodes.h`: a field the two groups disagree about fails generation
  naming the field, so the groups cannot drift apart silently. Three
  generators consume it: `rshooks-core`'s raw `lt*` codes, `rshooks`'
  `LedgerEntryType` enum, and the typed read views of §5.9 — whose three
  `rshooks/src/views/*.rs` modules `gen-core` renders from this artifact
  rather than from a second parse of the vendored files.

## 5. rshooks

`#![no_std]`, depends only on rshooks-core. `#![deny(missing_docs)]`.

```
src/
├── lib.rs         # prelude, panic handler (feature), re-export of rshooks-core as `raw`
├── error.rs       # HookError + Result<T>
├── types.rs       # AccountId, Hash, Keylet, ... #[repr(transparent)] fixed-size newtypes
├── convert.rs     # ToBytes/FromBytes boundary conversion traits
├── state.rs       # typed state layer (state_get/state_set_loose/state_update_loose) + state_keys!
├── buf_eq.rs      # loop-free, panic-free fixed-size buffer equality (buf_eq_8/20/32/...)
├── errors.rs      # hook_errors! user error enum -> rollback code mapping
├── xfl.rs         # XFL newtype over i64, checked Add/Sub/Mul/Div/Neg operators, compare/eq/lt/gt methods + PartialEq/PartialOrd
├── xfl_unchecked.rs # XFLUnchecked: poison-propagating hot-path counterpart to XFL
├── tx_type.rs     # @generated (§4): TxType enum mirroring rshooks-core's tts.rs, From<u16> + .code()
├── sfield.rs      # @generated (§4): typed SField<T> field constants mirroring rshooks-core's sfcodes.rs, + the 325-name parity test
├── slot_obj.rs    # typed slot layer (§5.8): SlotObject<T>, SField<T>, sealed SlotKey/CastTarget, slot_path!
├── txn.rs         # txn_template! macro + generic field-encoding primitives
├── static_cell.rs # HookStatic: take-once cell for static hook buffers
├── macros.rs      # guard!, trace!, rollback!, accept!, pad!
└── api/
    ├── mod.rs
    ├── control.rs # accept, rollback (-> !), hook_again, hook_skip, hook_pos
    ├── otxn.rs    # otxn_field, otxn_type (-> TxType), otxn_param, otxn_id, otxn_slot, ...
    ├── state.rs   # state, state_set, state_foreign(_set)
    ├── etxn.rs    # etxn_reserve, emit, etxn_details, etxn_fee_base, prepare
    ├── ledger.rs  # ledger_seq, ledger_last_time, fee_base, ledger_keylet, ...
    ├── hook_ctx.rs# hook_account, hook_hash, hook_param(_set)
    ├── slot.rs    # slot_* family, meta_slot, xpop_slot
    ├── sto.rs     # sto_subfield, sto_subarray, sto_emplace, sto_erase, sto_validate
    ├── float.rs   # thin fns backing XFL (float_sto, float_sto_set, slot_float)
    ├── util.rs    # util_accid, util_raddr, util_sha512h, util_verify, util_keylet(_buf)
    ├── keylet.rs  # one typed keylet_xxx() per KEYLET_* constant, built on util_keylet_buf
    └── trace.rs   # trace, trace_num, trace_float
```

### 5.1 Error model

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookError {
    OutOfBounds,          // -1
    InternalError,        // -2
    TooBig,               // -3
    /* ... every code from error.h ... */
    Unknown(i64),         // forward-compat for codes we don't know
}
pub type Result<T> = core::result::Result<T, HookError>;

#[inline(always)]
fn res(code: i64) -> Result<i64> { if code < 0 { Err(HookError::from(code)) } else { Ok(code) } }
```

Non-negative returns are payload (usually "bytes written"); negative maps to
`HookError`. Functions whose success value is meaningful keep it
(`Ok(len)`, `Ok(slot_no)`, …).

**Nesting-depth rule: at most one specific-`HookError`-variant match site per
function.** `res`'s `HookError::from(i64)` decode compiles to a wasm
`br_table` needing roughly one nested `block` per known error code (~40 at
the time of writing) — but only at a call site that actually inspects
*which* specific `HookError` variant a failure was (`match ... {
Err(HookError::Xxx) => ..., ... }`). A call site that only asks "did this
fail" (`.is_err()`, `Err(_) => ...`, comparing the whole `Result` against
one `Ok` value) never forces the decode, and the optimizer discards it
entirely, keeping just the "is the raw code negative" branch. `rshooks-build`'s
Guard-type pipeline inlines every function in a crate into `hook()`/`cbak()`
(§6.2c) and then must keep the merged function's block/loop/if nesting under
the vendored guard checker's 32-level limit (§6.3) — a crate with more than
one specific-variant match site pays that ~40-block decode's nesting cost
*at every one of them*, since each is inlined into the same function body.
In practice this means: default to `Err(_) => ...`/`.is_err()` at every Hook
API call site, and reserve `Err(HookError::SpecificVariant) => ...` for the
rare case that genuinely needs to distinguish one failure from another —
budget for at most one such site per crate before nesting depth becomes a
build-time concern. See `examples/80_reward` and `examples/81_govern`'s
READMEs for concrete before/after nesting-depth numbers from real crates,
each of which needs exactly one specific-variant match site.

The same mechanism applies to any large generated decode-into-enum
function, not just `HookError::from`: `TxType::from(u16)` (§5, `tx_type.rs`)
is a ~74-arm match with the identical shape, so comparing `otxn_type()`
against one specific named `TxType` variant (`otxn_type() == TxType::
ClaimReward`, as `examples/80_reward`/`examples/81_govern` both do) is the
`TxType` analogue of a specific-`HookError`-variant match site, subject to
the same one-per-crate budgeting logic — both examples still build within
the nesting limit with one of each (one `TxType`-specific comparison, plus
whatever `HookError`-specific handling each already had), but a crate
piling up several specific-variant comparisons against *either* enum adds
up against the same 32-level ceiling.

**rshooks's own internal paths must not use this pattern at all.** The
budget above ("at most one specific-variant match site per crate") is a
concession for *hook authors*, who have no cheaper alternative once they
need to branch on a particular `HookError`. `rshooks` itself is not in
that position: every one of its host-call wrappers already has the raw,
undecoded `i64` return code in hand *before* it ever calls `res`/
`HookError::from`, so comparing that code directly against a raw constant
(`rshooks_core::DOESNT_EXIST`, `rshooks_core::NOT_IMPLEMENTED`, …) needs no
enum-decode machinery at all — zero specific-variant-match sites, not one.
`crate::state::decode_read` (shared by `state_get`/`state_foreign_get`)
and `crate::api::state::value_or_absent` (shared by every
`state_update_*`) both compare the raw `code` against `rshooks_core::
DOESNT_EXIST` before any `HookError` is decoded, for exactly this reason —
the `Err(HookError::from(code))` fallback path is unaffected, since every
call site still only matches it as a bare `Err(_)`. Concretely: migrating
`examples/80_reward`'s `"RR"`/`"RD"` state reads to `hook_state!` +
`state_get_typed` needs this — without it, that migration pushes nesting
from 24 to 70 (over the limit); with it, nesting stays at 24 — see
`examples/80_reward/src/lib.rs` for the migrated call site. (One further
wrinkle: the raw-code helpers this needs — `state_raw_code`,
`state_u64_raw_code`, `state_foreign_raw_code` in
`crates/rshooks/src/api/state.rs` — are
*not* called by the existing `state`/`state_u64`/`state_foreign` public
wrappers, even though the logic is identical; routing those wrappers
through the new helpers, even with both sides `#[inline(always)]`,
measurably changed `rshooks-build`'s unnest-pass output for an unrelated
hook that never touches the new path at all. Each raw-code helper is
instead an independent duplicate of its wrapper's body — a small amount of
source duplication traded for a call-graph shape provably identical to
before the helper existed. See the doc comments on those functions.)

### 5.2 API wrapper conventions

- Caller-provided buffers, length returned — zero-copy and panic-free:

```rust
#[inline(always)]
pub fn state(out: &mut [u8], key: &[u8]) -> Result<usize>;
#[inline(always)]
pub fn hook_account(out: &mut [u8]) -> Result<usize>;
#[inline(always)]
pub fn hook_account_buf() -> Result<AccountId>;   // fixed-size convenience
```

- Every `out: &mut [u8]`/key-or-value-shaped `&[u8]` parameter across the
  `api::*` wrapper functions above (`state`, `state_set`,
  `state_foreign(_set)`, `otxn_field`, `otxn_id`, `otxn_param`,
  `hook_account`, `hook_hash`, `hook_param`, `ledger_last_hash`,
  `ledger_nonce`, `ledger_keylet`, `util_raddr`, `util_accid`,
  `util_sha512h`, `util_keylet`, `etxn_details`, `etxn_nonce`, `emit`,
  `prepare`, `slot`, `sto_emplace`, `sto_erase`, `float_sto`, ...) is
  written `&mut (impl AsMut<[u8]> + ?Sized)` / `&(impl AsRef<[u8]> +
  ?Sized)` instead of a bare `&mut [u8]`/`&[u8]`: a bare one cannot accept
  `&mut sender`/`&STATE_KEY` directly for a `types.rs` newtype (deref
  coercion to the newtype's `[u8; N]` `Deref::Target` does not chain with
  the further array-to-slice unsized coercion at one call site — see
  `types.rs`'s module doc comment), so a caller would otherwise have to
  write `sender.as_mut()`/`STATE_KEY.as_ref()` at every call. Bounding by
  `AsMut<[u8]>`/`AsRef<[u8]>` instead — every `crate::types` newtype already
  implements both — lets `otxn_field(&mut sender, sfAccount)`/
  `state(&mut raw, &STATE_KEY)` work as-is, at zero cost (monomorphized per
  call site, verified by `mise run build-examples`'s unchanged per-example
  wasm size/WCE). `_exact::<const N>` functions (`otxn_field_exact`,
  `hook_param_exact`, `slot_exact`, `state_exact`) are unaffected — they
  already return an owned `[u8; N]`, no buffer parameter to genericize.
  `state_foreign`'s `namespace`/`account` are the one pair that stay a
  *different* generic shape (`impl api::state::ForeignRef<'_>`, not a plain
  `AsRef<[u8]>` bound): they're `Option`-shaped, and a bare `None` cannot
  pin down an unconstrained `Option<K: AsRef<[u8]>>`'s `K` — `ForeignRef`
  resolves that by accepting `None` (absent) or a *bare* reference (present,
  not `Some(&value)`) instead of one `Option<K>` — see `ForeignRef`'s doc
  comment in `api/state.rs` for the full reasoning (verified against rustc,
  not just argued about).

- Every wrapper is `#[inline(always)]` (extra internal functions are both a
  size cost and a validation risk — C7).
- Buffers that have a protocol-fixed size get typed convenience wrappers
  returning `types.rs`' `#[repr(transparent)]` newtypes (`AccountId`
  wrapping `[u8; 20]`, `Hash`/`Keylet`/`Nonce` wrapping `[u8; 32]`/
  `[u8; 34]`/`[u8; 32]`, …) rather than bare arrays — same layout, size,
  and FFI-compatibility as the array, but distinct at the type level so an
  `AccountId` and a `Hash` can no longer be passed to each other's slots by
  accident (see `types.rs`'s module doc comment). The caller-buffer form keeps
  the standard name (`hook_account(out: &mut [u8], ...) -> Result<usize>`,
  matching the raw Hook API's write_ptr/write_len shape and the crate's
  other caller-buffer functions like `state`); the array-returning
  convenience is the same name with a `_buf` postfix
  (`hook_account_buf() -> Result<AccountId>`), for callers who just want
  the value. Writing directly into an existing buffer (e.g. a region of a
  larger template) uses the standard form; the host's own
  TOO_SMALL/OUT_OF_BOUNDS handling applies to whatever slice is passed. The
  `_buf` form delegates to the standard form so each raw call site exists
  once.
- **"as-int64" mode** (`state`, `state_foreign`, `otxn_field`, `slot`):
  the host treats `write_ptr = 0, write_len = 0` as a request to return
  the data itself, packed **big-endian** into the non-negative `i64`
  return — only for data of at most 8 bytes with the top bit clear, else
  `TOO_BIG` (xahaud `applyHook.cpp`, `data_as_int64`). Exposed as
  `<name>_u64(...) -> Result<u64>` variants. (`state_set` /
  `state_foreign_set` have no such mode — they carry no write buffer.)
  Emit details are variable-length — 116 bytes, or 138 when the
  module exports `cbak`
  (verified against `HookAPI::etxn_details` in xahaud) — so there is no
  fixed `EmitDetails` array alias, only `EMIT_DETAILS_MAX_LEN = 138` and a
  caller-buffer `etxn_details(out: &mut [u8]) -> Result<usize>` wrapper.
  (The initial design said 105 bytes; that number was wrong.)
- `accept`/`rollback` return `!` (call, then `unreachable` opcode — the host
  never returns from them).
- Slot/keylet numbers are plain `u32` in v1 (no newtype ceremony); field
  codes are `u32` taken from `rshooks_core::sfcodes`.
- **Pointer-direction discipline**: wrappers call the raw extern functions
  directly, spelling out `buf.as_mut_ptr() as u32` for `write_ptr` and
  `buf.as_ptr() as u32` for `read_ptr` at each call site. No generic
  "pass a slice" helper that erases direction — prior art has had bugs from
  exactly that blur (e.g. around `hook_hash`/`hook_skip`). If helpers are
  used at all they must be direction-specific (`out_buf!` vs `in_buf!`).

### 5.3 XFL

`#[derive(Clone, Copy, Debug)] pub struct XFL(u64);` — Xahau 64-bit decimal
float. The inner field is **private**: XFL host calls return negative
values as error codes, and a public field would let users smuggle an error
code in as a "value". Escape hatches are explicit: `XFL::from_raw_bits(i64)`
/ `xfl.raw_bits() -> i64` (documented as unchecked representation access) —
the public boundary still speaks `i64`, matching the Hook API's FFI
convention (every `float_*` extern function takes/returns `i64`) and the
existing persisted-state `ToBytes`/`FromBytes` encoding; only the *internal*
storage is `u64`, a bit-preserving `as` cast away at both boundaries (every
`float_*` call site) and zero-cost (same-width integer casts). This mirrors
the fact that every `XFL` obtained through the validated API is guaranteed
by the host to have bit 63 clear (see the module doc comment's bit
layout) — always non-negative when read back as `i64` — the same way Rust
represents an opaque bit pattern via `f64::to_bits() -> u64`, not `-> i64`;
the `i64` FFI type is an artifact of the Hook API's C ABI multiplexing
error codes onto XFL's return channel, not a property of the bit pattern
itself. `XFLUnchecked` (below) deliberately keeps `i64` instead — it exists
specifically to hold values that might *be* negative error codes.

`XFL` has **no panicking arithmetic**: `core::ops` is not implemented with
an infallible `Output` for any operator that can fail, since that would
force a panic (or a silently wrong answer) on the failure path. `XFL`
implements `core::ops::{Add, Sub, Mul, Div, Neg}`, all with `Output =
Result<XFL, HookError>` — every one of these, including `Neg`, is a
fallible host round trip (`float_sum`/`float_multiply`/`float_divide`/
`float_negate`; `Sub` is `self + (-rhs)?`: one `float_negate` call plus one
`float_sum` call, since there is no dedicated `float_subtract` function).
`Neg` and comparison are host round trips rather than local bit
manipulation on principle, not just for `Neg`/comparison specifically:
this crate treats the host's `float_*` implementations as the sole
authority on XFL bit-pattern semantics, and never maintains a parallel
guest-side reimplementation of them — [`XFL::exponent`]'s local bit-field
extraction is not an exception to this, since it only unpacks an
already-host-produced value's fields rather than computing a new,
independently-derived value the way negation or comparison would.
Comparison has named methods (`eq`/`lt`/`gt`/`compare`, all `Result<bool>`
via `float_compare`) *and* `PartialEq`/`PartialOrd` (`==`/`<`/`>`/...),
both backed by the same `float_compare` calls — see below for the fallback
story that makes offering both possible.

To keep multi-step arithmetic ergonomic despite `Add`/`Sub`/`Mul`/`Div`'s
fallible `Output`, rshooks additionally implements each of those traits
for `Result<XFL, HookError>` on either side of a plain `XFL` (legal here
specifically because `XFL` is local to this crate — a downstream crate
cannot replicate the trick for its own types; see `xfl.rs`'s module doc
comment for the full orphan-rule argument, plus the one combination —
`Result<XFL, HookError>` on *both* sides at once — that is *not* legal and
was confirmed unavailable by attempting it and reading rustc's own
diagnostic, not just reasoned about).

```rust
impl XFL {
    pub fn new(exponent: i32, mantissa: i64) -> Result<XFL>;      // float_set
    pub fn one() -> XFL;
    pub fn unchecked(self) -> XFLUnchecked;                        // zero-cost reinterpret, see below
    pub fn invert(self) -> Result<XFL>;
    pub fn mulratio(self, round_up: bool, num: u32, den: u32) -> Result<XFL>;
    pub fn mantissa(self) -> Result<i64>; pub fn exponent(self) -> Result<i64>;
    pub fn sign(self) -> Result<bool>;
    pub fn to_int(self, decimal_places: u32, absolute: bool) -> Result<i64>;
    pub fn compare(self, rhs: XFL, mode: u32) -> Result<bool>;     // float_compare
    pub fn eq(self, rhs: XFL) -> Result<bool>; pub fn lt(self, rhs: XFL) -> Result<bool>; pub fn gt(self, rhs: XFL) -> Result<bool>;
    pub fn log(self) -> Result<XFL>; pub fn root(self, n: u32) -> Result<XFL>;
}
impl core::ops::Add for XFL { type Output = Result<XFL>; ... }   // float_sum
impl core::ops::Sub for XFL { type Output = Result<XFL>; ... }   // self + (-rhs)?: float_negate + float_sum
impl core::ops::Mul for XFL { type Output = Result<XFL>; ... }   // float_multiply
impl core::ops::Div for XFL { type Output = Result<XFL>; ... }   // float_divide
impl core::ops::Neg for XFL { type Output = Result<XFL>; ... }   // float_negate -- a host round trip, not a bit flip
impl PartialEq for XFL { ... }   // forwards to XFL::eq (float_compare); false on failure
impl PartialOrd for XFL { ... }  // forwards to XFL::lt/XFL::gt (float_compare, up to 2 calls); None on failure
impl From<XFL> for u64 { ... }   // the native u64 shape (see above), alongside raw_bits's i64 shape -- one direction only, no From<u64> for XFL
```

`PartialEq`/`PartialOrd` are thin forwarding wrappers over the
`float_compare`-backed `eq`/`lt`/`gt` methods above, not a separate local
implementation — comparison is a fallible host call either way. Those two
traits' methods return a bare `bool`/`Option<Ordering>`, with no room for
an `Err` case, so on a `float_compare` failure the operators fall back to
`false`/`None` — the same convention `f64`'s own `PartialEq`/`PartialOrd`
use for `NaN` ("couldn't establish equality/order" represented as "not
equal"/"not comparable," not fabricated as a specific wrong answer, and not
a panic, and not a `rollback!`: `crate::api::control::rollback` loops
forever rather than returning on `not(target_arch = "wasm32")` (there is
no host to actually terminate the process on a host build), and
`float_compare`'s host stub fails *deterministically*, so routing a
comparison failure through `rollback!` would hang every host-target
test/doctest that exercises `==`/`<`/`>`). The `Result<bool>`-returning
`eq`/`lt`/`gt`/`compare` methods are the way to get the real failure
explicitly, for call sites that need to distinguish "genuinely not equal"
from "couldn't tell." `Neg`, unlike `PartialEq`/`PartialOrd`, does not need
this fallback story at all: its `Output` type isn't fixed by the trait, so
a `float_negate` failure just propagates as a real `Err`, the same as
every other arithmetic operator. Bitwise representation equality, if ever
needed, gets an explicitly named method (`bits_eq`), not `==`.

**`XFLUnchecked`** (`rshooks::xfl_unchecked`) is the poison-propagating
hot-path counterpart, for arithmetic chains where even the checked
operators' per-step `Result` branch is the measured cost problem:

```rust
pub struct XFLUnchecked(i64);   // no PartialEq/PartialOrd, unlike XFL -- the false/None-on-failure fallback that's principled for XFL (an occasional edge case, like f64's NaN) would be actively misleading here, where poisoned operands are the routine case, not the exception
impl XFLUnchecked {
    pub fn from_raw_bits(bits: i64) -> XFLUnchecked;
    pub fn raw_bits(self) -> i64;
    pub fn validate(self) -> Result<XFL>;   // float_sum(self, 0) -- a host round trip, not a guest-side check
}
impl core::ops::{Add, Sub, Mul, Div, Neg} for XFLUnchecked { type Output = XFLUnchecked; ... }  // every one a host round trip, no per-step guest check
impl From<XFLUnchecked> for i64 { ... }  // identical to raw_bits (already i64) -- idiomatic .into()/i64::from(...) alongside it
```

Its operators skip guest-side validation entirely and pass the raw `i64`
straight into the next host call — every operator, including `Neg`
(`float_negate`); there is no local-only fast path here either, for the
same reason `XFL`'s `Neg` isn't one. This is sound because xahaud's
`RETURN_IF_INVALID_FLOAT` gate (verified against `applyHook.cpp`) runs on
**every** `float_*` host function's operands **before** any arithmetic,
independent of what the guest validated — so a poisoned/invalid operand can
never produce a spuriously "valid" result from any operator; it collapses
to `INVALID_FLOAT` at the first one it passes through (see
`xfl_unchecked.rs`'s module doc comment for the full audit table, and the
one caveat: this collapsing means a specific upstream `HookError` is not
preserved through the chain — only that *some* failure occurred).
`validate()`'s `float_sum(self, 0)` fully validates `self` despite
`HookAPI::float_sum`'s own `float1 == 0`/`float2 == 0` short-circuit (which
looks, on its face, like it might let `self` through unvalidated when
`self` is nonzero): that short-circuit lives inside `HookAPI::float_sum`,
reached only *after* the `DEFINE_HOOK_FUNCTION` wrapper's
`RETURN_IF_INVALID_FLOAT` has already validated both operands.

Measured (`crates/rshooks`'s scratch WCE bench against the actual shipped
types, N=1/4/8 chained ops):

| chain | marginal cost/op |
|---|---|
| raw `float_multiply` (baseline) | +3 |
| `XFLUnchecked` `Mul` chain | +3 (matches raw exactly) |
| checked `Result`-chain `Mul` | +14 |
| raw `float_negate`+`float_sum` (baseline) | +5 |
| `XFLUnchecked` `Sub` chain | +5 (matches raw exactly) |
| checked `Result`-chain `Sub` | +27 |

`XFLUnchecked`'s marginal cost matches a hand-written raw host-call chain
exactly for both operators — its performance win over the checked operators
is real and comes entirely from skipping the per-step `Result` branch, not
from skipping any host validation a correct implementation actually needs
(see `examples/07_xfl-math/README.md`'s "Zero-cost check" section for the
N=1/4/8 breakdown behind these marginal-cost figures).

### 5.4 Macros & entry point

- `guard!(maxiter)` / `guard_m!(maxiter, n)` — match the C `GUARD`/`GUARDM`
  macros from `macro.h` **exactly, including the `+ 1`**:
  `GUARD(maxiter)` in C is `_g((1ULL << 31U) + __LINE__, (maxiter) + 1)`.
  Rust: `guard!(m)` → `_g((1u32 << 31) + line!(), (m) + 1)`;
  `guard_m!(m, n)` → `_g((1u32 << 31) + (line!() << 16) + (n), (m) + 1)`
  (same id formula as C `GUARDM`) for multiple loops on one line. All
  arithmetic explicit `u32` with `wrapping_add`-free constants.
  Guards are the developer's responsibility by default (see 6.3); the
  opt-in auto-guard pass exists mainly for compiler-generated loops.
- `trace!("msg")`, `trace!("msg", data)`, `trace_num!`, `trace_float!` —
  compiled to nothing unless **rshooks's** `trace` feature is enabled
  (traces cost bytes and execution; examples enable it in dev). The feature
  gate lives in hidden `#[inline(always)]` shim functions inside rshooks,
  NOT as a `#[cfg]` in the macro body — a cfg written in a `macro_rules!`
  body is evaluated against the *calling* crate's features, which would
  force every hook crate to re-declare a same-named feature. With the shim,
  `rshooks = { features = ["trace"] }` on the dependency line is all a
  hook crate needs.
- `accept!()/accept!(msg, code)`, `rollback!(msg, code)` — terse exits.
- `uninit_buf!()` is NOT provided: `MaybeUninit::uninit().assume_init()` for
  arrays is UB. Buffers are `[0u8; N]`; the cleaner/opt pipeline keeps the
  cost acceptable, and correctness wins.
- Entry point: `#[hook]` / `#[cbak]` (from `rshooks-macros`, re-exported as
  `rshooks::hook`/`rshooks::cbak`) turn a plain, argument-less
  `fn name() -> i64` into the required wasm export:

```rust
use rshooks::hook;

#[hook]
fn my_hook() -> i64 { ... }
```

  expands to (unchanged original function, plus):

```rust
#[unsafe(no_mangle)]
pub extern "C" fn hook(_reserved: u32) -> i64 {
    my_hook()
}
```

  `#[cbak]` is identical except it exports `cbak`. Both are hand-rolled
  `proc_macro` (no `syn`/`quote` — see `rshooks-macros`'s crate doc comment
  for why): they only ever need to recognize one token shape (a
  no-argument, `i64`-returning, non-generic, non-`async`/`unsafe`/`const`/
  `extern` `fn`), so a general Rust-item parser is unneeded weight. Every
  malformed shape is a `compile_error!` at the offending token, not a
  macro panic.
- Panic handler behind default feature `panic-handler`:
  `rollback(b"panic", ...)` then `unreachable` — examples just work; users
  embedding differently can disable it.
- `hook_state!`/`hook_parameter!`/`otxn_parameter!` — a declaration-macro
  **grammar staircase** of six forms (from a fully-fixed zero-sized
  key/name down to a fully composite, runtime-constructed one, plus an
  `Entity, existing Name = bytes => Ty` form that attaches the key/name
  impls to a type the *caller* declared, plus an `Entity, Key => Value`
  pairing form that wraps a key/name the caller declared **and** already
  gave an encoding to — local, encodable, and not already paired, since a
  second pairing for the same key is rustc's `E0119`), each declaring in one
  line what
  previously took a separate `#[derive(HookKey)]`/`#[derive(HookData)]`/
  `#[derive(ParamName)]`/`#[derive(ParamValue)]` struct plus a
  `Key => Value` pairing. See `rshooks::hook_state!`'s doc comment for the
  full grammar and worked examples.
  **Every invocation names an entity first**, and the entity — not the
  key/name — is the primary surface: `hook_state!(DepositState, DepositKey
  {tag: u8, owner: AccountId} => Deposit {amount: u64})` declares both, and
  `DepositState { tag: 1, owner }` is what carries
  `get_state`/`set_state`/`update_state`/`delete_state` (a parameter entity
  carries `get_value`, plus a `const fn get_name` on the two
  fixed-byte-string forms). Each accessor is an `#[inline(always)]` forward
  to the free function of the same name, so `entity.get_state()` and
  `state_get_typed(&entity)` are the same code and the choice is purely one
  of reading order. `delete_state` is backed by a free function,
  `state::state_delete` — the explicit, value-type-independent deletion API
  (deletion is a zero-length `state_set`, which `state_set_typed(&key,
  &Value)` can only reach by way of a `Value` that happens to encode to
  nothing, so spelling it as a call that takes only a key is both clearer
  and available to keys with no pairing at all).
  **The entity is a real key/name, not a method holder.** It implements the
  role traits itself, so it works with every free, loose and `_foreign`
  function — `state_foreign_get_typed(&entity, ..)` and the rest. How it
  encodes depends on the form and never involves building a key value: a
  fixed form encodes its declared literal, a struct/newtype form mirrors the
  key's fields and runs the *identical* per-field codegen against its own,
  and the pairing form forwards through `&self.0` (state:
  `StateKeyEncode::encode`, which is why a `state_keys!` enum with no
  `ToBytes` pairs fine; parameters: `ToBytes::write` plus
  `TypedParamName::with_name_bytes` delegated to the name's own override, so
  its exact-size buffer or `'static` literal survives). **Generated code
  never constructs a caller-owned type** — that is what lets `existing` and
  pairing work with a key that is non-`Copy`, privately built, or not
  constructible from the invocation site at all.
  The key/name type stays declared as a trait carrier with **no** inherent
  accessors: putting them there would claim method names on a type the
  caller may own, and would make the address of the thing look like the
  thing. `existing` and pairing are **module-position only** (they emit
  impls for a type the surrounding module owns; inside a fn they trip
  rustc's `non_local_definitions`) — a supported-position policy, not a
  parser rule, since a proc macro cannot see its own position.
  **Optional leading visibility.** `pub`/`pub(crate)`/... before the entity
  applies to every item the invocation declares — entity, key/name, inline
  value, their fields, and a Form 2 `const` — all-or-nothing, private by
  default. Making the generated items public does not make a caller-owned
  type public: every such type reaching a public generated field or
  associated type must be at least as visible, or rustc reports its own
  `E0446`/`E0445`. Generated items carry doc comments for the same reason
  they carry `#[allow(dead_code)]`: they land in the caller's crate, where
  `missing_docs` may well be denied.
  **Function-like `#[proc_macro]`s in `rshooks-macros`, not `macro_rules!`**
  (a change from the crate's original design): the grammar needs real
  lookahead disambiguation between forms (a `{`/bare `=`/`,`/second bare
  identifier immediately after the declared name) that `macro_rules!`
  transcribers can't express directly, and reuses the same struct-shape
  parsing/codegen `#[derive(HookKey)]`/`#[derive(HookData)]`/
  `#[derive(ParamName)]`/`#[derive(ParamValue)]` already provide (see
  `rshooks-macros`'s `decl_pair` module) rather than duplicating it in a
  macro-by-example. Still hand-rolled `proc_macro::TokenStream` parsing, no
  `syn`/`quote` (same reasoning as `#[hook]`/`#[cbak]` above): a flat,
  randomly-indexable token buffer with 2–3-token bounded lookahead is
  enough to disambiguate every form. Every declared type name — the entity,
  a key/name struct, an inline value definition — must be `UpperCamelCase`,
  and the three must be spelled differently from each other, both checked at
  the macro invocation with a `compile_error!` naming the offending
  identifier. That is the one piece of validation `macro_rules!`
  categorically cannot do: there is no way to inspect an identifier's own
  spelling from inside a `macro_rules!` matcher.

**Every composite name/key this grammar declares gets an exact-size
encode buffer, not a generic worst-case-sized one.** `TypedParamName`'s
one abstract method, `with_name_bytes<R>(&self, f: impl FnOnce(&[u8]) ->
R) -> R`, hands the caller a closure instead of writing into (or
returning a reference into) a caller-owned buffer, so each concrete name
type controls *where* its encoded bytes live: a `'static` literal for the
zero-copy plain-byte-string forms, or, for every composite form
(2/3/4), a stack buffer sized to exactly that name's own
`ToBytes::MAX_LEN` — not the full 32-byte `PARAM_NAME_MAX_LEN` the
trait's *generic* default body must fall back to. A pairing entity adds no
buffer of its own: it forwards `with_name_bytes` to the name's override,
which this same invocation just generated at exactly that name's size.
(Generic code can't spell `[0u8; Self::MAX_LEN]` on stable Rust; only a
concrete, non-generic
`impl` block can, the same restriction `FixedRead::read_exact`'s doc
comment documents for the read side). Measured impact:
`examples/81_govern`'s `IS{seat}` (a composite, runtime-varying name)
went from **+607** worst-case instructions over the raw baseline to
**0** — see `examples/81_govern/src/lib.rs`'s `IS{seat}` doc comment.
`examples/12_typed-data`'s composite `AdminName` parameter improved too
(485 → 470 worst-case instructions), confirming the fix generalizes.
Overriding `with_name_bytes` also *replaces* the trait default's own
`1..=PARAM_NAME_MAX_LEN` length assertion, so every generated override
carries a monomorphized copy of it. That matters most for the "pair two
already-declared types" form, whose name type is caller-authored and never
saw the derive-time check: without the copy, a `ToBytes` name encoding to
0 bytes (which the host rejects at runtime) or to several kilobytes (a
stack buffer well past the memset-inlining threshold) compiled with no
complaint at all.

`StateKeyEncode`/`EncodedStateKey` were checked for the identical
asymmetry and found not to have it: a hook-state key's raw `[u8; N]`
baseline *already* goes through `EncodedStateKey`'s own always-32-byte
buffer (the Hook API left-pads a short key host-side, so every key is
carried in a fixed 32-byte struct field either way), so there is no
zero-copy baseline being lost the way `hook_param_exact`'s direct
pointer-passthrough was for names — confirmed by measuring composite
state keys (`examples/02_state-counter`'s `hook_state!` Form 2), which
show no cost change.

### 5.5 Emitted-transaction templates: `txn_template!` (user-defined layouts)

Modeled on xahaud's C "Tx Builder" split, where the template bytes and
field pointers are pasted into the *hook's own source* and only generic
helpers (`SET_UINT32`, `SET_NATIVE_AMOUNT`, `COPY_20`) are shared: a
library-owned fixed template type (the first iteration's
`PaymentTemplate`) was rejected because any new field or transaction type
would require a rshooks release. Instead rshooks provides exactly two
things:

1. **Generic encoding primitives** (`txn::codec`): native-amount encoding
   (62-bit check + the `0x40` native bit), big-endian u32 writes, STObject
   field-header derivation from an `sfXxx` code (`type = code >> 16`,
   `field = code & 0xFFFF`; 1–3 prefix bytes per the canonical rules —
   verified against the C template bytes: `0x12`, `0x22`, `0x20 0x1A`,
   `0x61`, `0x73 0x21`, `0x81 0x14`), all `const fn` where layout-relevant.
2. **`txn_template!`** — a declarative macro playing the role of the C
   code generator's output. The hook author declares an ordered field list
   (kinds: `u32_field(sfXxx)`, `native_amount(sfXxx)`, `account_id(sfXxx)`,
   `empty_vl(sfXxx)`, `emit_details`, plus the leading
   `transaction_type = ttXXX`); the macro computes cumulative offsets and
   total length at compile time, bakes the field headers into a
   `const fn new()` template (⇒ data segment via `HookStatic`), and
   generates typed `set_<field>` setters plus an `emit_details_region()`
   accessor. Setter names are synthesized by splicing `set_` and the field
   name (`[<set_ $field>]`) through `rshooks-macros`'s `paste`-equivalent
   proc-macro (`$crate::__paste!`, wrapping the generated `impl` block) —
   a small, purpose-built identifier-concatenation macro that replaces
   nightly's `${concat(set_, $field)}` metavariable expression, letting
   `txn_template!` (and every crate that calls it) build on stable Rust.
   A compile-time assertion rejects field lists that violate canonical
   (type, field) ordering — a safety the C flow lacks. `emit_details`
   must be last.

The `PREPARE_TXN()` equivalent, `prepare_for_emit()`, is **generated by
the macro too**, and the emit-plumbing fields are recognized **by their
`sfXxx` code, not by special declaration syntax** — every field uses the
same uniform kinds (`u32_field(sfXxx)`, `native_amount(sfXxx)`,
`account_id(sfXxx)`, `empty_vl(sfXxx)`, `emit_details`). (An earlier
role-kind design — `sequence: sequence,` next to
`flags: u32_field(sfFlags)` — was rejected as a second declaration
dialect users had to learn.)

Mechanically, the muncher accumulates a const table of
`(sfcode, kind tag, payload offset)` per field. The base arm then emits
const-evaluated checks (all failures are named E0080 compile errors):

- **presence**: `sfSequence`, `sfFirstLedgerSequence`,
  `sfLastLedgerSequence`, `sfFee`, `sfSigningPubKey`, `sfAccount` must
  each appear in the table, and an `emit_details` field must be declared
  (last); `transaction_type` is grammar-mandatory and first. An emitted
  transaction without these is invalid at the protocol level, so the
  macro refuses to build one.
- **kind agreement**: the required codes must be declared with the right
  kind (`sfFee` as `native_amount`, `sfAccount` as `account_id`,
  `sfSigningPubKey` as `empty_vl`, the three sequence fields as
  `u32_field`) — a wrong kind would make `prepare_for_emit` corrupt the
  template, so it is rejected at compile time.

Because detection is by *value*, it is robust to how the constant is
spelled (qualified paths, aliases). `prepare_for_emit(&mut self) ->
Result<Prepared<'_, Self>>` is generated unconditionally, resolving the six
offsets by const lookup in the same table (`ledger_seq()+1`→FLS, FLS+4→LLS,
`hook_account()`→account, `etxn_details` into the region — its returned
length fixes the real blob length — then `etxn_fee_base` over the actual
blob→fee). `Prepared<'a, T>` (`rshooks::txn::Prepared`) is a typestate
wrapper — `{ inner: &'a mut T, len: usize }` — that is the *only* way to
reach an emit-sized slice (`Prepared::as_bytes`) or emit it
(`Prepared::emit`, wrapping `api::etxn::emit_buf`): the unprepared template
type has no `as_bytes`/`emit` method at all, so code cannot emit a buffer
whose FLS/LLS/Account/EmitDetails/Fee were never actually filled — that
mistake is now a compile error (`E0599`, no method found), not a runtime
footgun. `Prepared` borrows rather than owns `Self` (generated structs
usually live behind `HookStatic::take`'s `&'static mut T`, so an owning
typestate would need a needless `mem::replace` dance) and `Deref`/
`DerefMut`s to `Self`, so setters remain callable after preparing too (e.g.
adjust a field and call `prepare_for_emit` again). Setters are generated
uniformly for every settable field regardless of role — value-based
required-field detection cannot be reflected in which setters *exist*, only
in what a separate typestate lets you do with them — so setter existence
is unchanged; only the FLS/LLS/Account/EmitDetails/Fee values themselves
are inaccessible for emission until `prepare_for_emit` runs. Transaction
*shape* remains entirely user-declared — new fields or txn types never
require a rshooks change; only the fixed emit plumbing is canned. The
`const fn new()` template always reserves
the full `EMIT_DETAILS_MAX_LEN = 138` bytes of capacity for the
emit-details region regardless of whether the module exports `cbak`, but
those reserved zero bytes cost nothing in the emitted binary — the
cleaner's trailing-zero data-segment trim (6.2 step 3) strips them from
the baked template's data segment — and the *runtime* blob length is
whatever `etxn_details` actually returns (116 bytes without `cbak`, 138
with), so cbak-vs-not needs no declaration-time switching at all.

### 5.6 Endianness conventions

This crate straddles two independent endianness worlds. Neither is
"correct" or "wrong" — each is the native convention of the domain it
belongs to — but conflating them silently byte-swaps a value, so the rule
is formalized once here and every module that touches either side links
back to this section instead of re-explaining it.

| Domain | Endianness | Concrete evidence | Lives in |
|---|---|---|---|
| Xahau Binary (the protocol's own STObject/tx wire format) | **Big-endian** | `txn.rs`'s `txn_template!`-generated setters write every multi-byte field with an explicit big-endian encoding (`u32`/`u16` field values, the `tts` transaction-type code, native-amount drops — see e.g. the `.to_be_bytes()` calls building setter bodies and the `tts::$tt as u16).to_be_bytes()` STObject field header); `examples/80_reward/src/mint_txn.rs` and `examples/81_govern/src/txn.rs` (hand-rolled "Tx Builder" equivalents for the genesis hooks) do the same by hand throughout | `crates/rshooks/src/txn.rs`, `examples/80_reward`, `examples/81_govern` |
| Xahau Binary — the Hook API host's "as-int64" mode | **Big-endian** | `state`/`state_foreign`/`otxn_field`/`slot` called with `write_ptr = 0, write_len = 0` return the entry's raw bytes packed big-endian into the non-negative `i64` result (xahaud `applyHook.cpp`, `data_as_int64`) | `api::state::state_u64`/`state_foreign_u64`, `api::otxn::otxn_field_u64` |
| Xahau Binary — keylets | **Big-endian** | A keylet's first two bytes are the ledger-entry-type tag, big-endian, per xahaud's own keylet construction. rshooks never assembles keylet bytes itself — every `keylet_xxx` helper (`api/keylet.rs`) calls the host's `util_keylet` and receives an already-built, opaque `Keylet`/`[u8; 34]` back — this row documents the host's own convention, not code in this crate | xahaud host (`util_keylet`); wrapped opaquely by `crates/rshooks/src/api/keylet.rs` |
| Xahau Binary — short state/param keys | **Big-endian-flavored zero-padding**: a key shorter than the fixed key width is **left**-padded with zero bytes by the host (the value's bytes end up at the *end* of the fixed-width key, not the front) | rshooks' `StateKeyEncode` layer (`[u8; N]`, `state_keys!`, `#[derive(HookKey)]`) sends a short key at its own real length and relies on this host-side left-pad directly — see §5.7 for the full rule; `pad_left!` (`crates/rshooks/src/macros.rs`) reproduces this same left-pad *locally*, for the rarer case of needing the already-padded bytes themselves as a value, not as a `state`/`state_set` argument | host left-pad: xahaud; local equivalent: `pad_left!` (`crates/rshooks/src/macros.rs`) |
| Hook-private data: state values, param values | **Little-endian** (the guest's own native memory image — LE on `wasm32v1-none`) | The C hook idiom `state(&native_int64, 8, key, klen)` — a raw pointer to a native `int64_t`, read/written in whatever the guest's own endianness is; `crates/rshooks/src/convert.rs`'s `ToBytes`/`FromBytes` traits (and every `rshooks::types` newtype, plus the `#[derive(HookKey)]`/`#[derive(HookData)]` macros built on them) encode/decode this way; `api::state::state_u32`/`state_i64`/`state_xfl` (+ their `state_set_*`/`state_update_*` twins) read/write this convention via the ordinary (non-as-int64) buffer path | `crates/rshooks/src/convert.rs`, `crates/rshooks/src/types.rs`, `api::state::state_u32`/`state_i64`/`state_xfl`/`state_u64_le`/`state_foreign_u64_le` |
| Raw byte sequences (`AccountId`, `Hash`, ...) | **Neutral** — not a byte-order question | An `AccountId`/`Hash` is an opaque sequence of bytes with no numeric interpretation; scalar Hook API return values, `sfcode`s, and an `XFL`'s raw bit pattern are likewise "values," not multi-byte integers subject to a byte-order convention | — |

**The one API surface that deliberately offers both conventions side by
side** is `api::state`'s `_u64`-suffixed family, precisely because state
entries can originate from either world:

- [`state_u64`](crate::api::state::state_u64) / [`state_foreign_u64`](crate::api::state::state_foreign_u64)
  — the host's as-int64 mode, **big-endian**. Read a state entry whose
  bytes originated from Xahau Binary itself — e.g. a value mirroring a
  protocol field like `Tx.Sequence`, or interop with a C hook that wrote
  the entry with explicit big-endian bytes to match protocol convention.
  Reading an entry that was instead written by this crate's LE typed layer
  comes back byte-swapped — that is the documented behavior, not a bug.
- [`state_u64_le`](crate::api::state::state_u64_le) / [`state_foreign_u64_le`](crate::api::state::state_foreign_u64_le)
  — the ordinary buffer path, **little-endian**. Read a state entry
  written by this crate's own typed layer (`ToBytes`/`FromBytes`,
  `state_set_loose`/`state_set_typed`) or by hand with `to_le_bytes` — the
  same convention as `state_u32`/`state_i64`/`state_xfl`, just unsigned
  and 64-bit.

Picking the wrong one of the pair does not fail loudly — both succeed and
return a `u64`, just byte-swapped relative to what was intended — so the
choice has to be made deliberately, by knowing which world wrote the
bytes being read.

**Decoding a protocol-BE field read via the raw layer** (`otxn_field_exact`,
`hook_param_exact`, `slot_exact`, ...) follows the same rule: a Xahau
Binary field (an `Amount`, a `Sequence`, ...) must be decoded with an
explicit `u64::from_be_bytes(...)`, never through this crate's `FromBytes`
trait (which is little-endian) — see `crates/rshooks/src/api/otxn.rs`'s
module doc comment and `examples/03_hook-params`/`examples/04_errors`
(`u64::from_be_bytes(raw) & !NATIVE_AMOUNT_FLAG_BITS`) for the existing,
now-documented-as-normative pattern.

**`pad!` vs. `pad_left!`**: both are const, compile-time zero-padding
helpers over a short byte string, padding on opposite sides — `pad!`
right-pads (value first, zero bytes after), `pad_left!` left-pads (zero
bytes first, value last), mirroring the host's own left-pad convention for
a short key. **Neither is needed to build an ordinary hook-state key**
(see §5.7 — a plain `[u8; N]`, `state_keys!` variant, or `#[derive(HookKey)]`
struct is sent at its own real length and the host left-pads it, no local
padding involved at all); reach for one of these two macros only when a
hook genuinely needs the *already-padded* 32 bytes themselves as a value —
e.g. a full, already-32-byte `StateKey`/`NameSpace` constant (`pad!`), or
reproducing what the host's left-pad of a given short key would look like,
byte-for-byte, for some purpose other than passing it to `state`/
`state_set` (`pad_left!`).

See [`crate::convert`]'s and [`crate::state`]'s module doc comments for
how the little-endian `ToBytes`/`FromBytes` convention flows through the
typed storage layer built on top of it.

### 5.7 Hook state key encoding: real length, not local zero-padding

`state`/`state_set`/`state_foreign(_set)` accept any key from 1 to 32 bytes
(`hook_api.h`: `TOO_SMALL` below 1, `TOO_BIG` above 32) and **left-pad** a
shorter key internally to the host's own fixed-width storage slot (see
§5.6's "short state/param keys" row) — this is the C hook idiom
`state(&v, 8, "RR", 2)`: a 2-byte literal key, handed straight to the
host, unpadded.

rshooks' typed key layer (`crate::state::StateKeyEncode` — the trait behind
`state_get`/`state_set_loose`/`state_update_loose` and their `_foreign`
twins, `state_keys!`, and `#[derive(HookKey)]`) matches this exactly: every
`encode()` call returns the key's own **real** encoded length, never
locally zero-padded up to 32 bytes. Concretely:

| key type | real encoded length | notes |
|---|---|---|
| `[u8; N]` (`1 <= N <= 32`) | `N` | direct counterpart to a C hook's short literal key; `N` is a compile-time-checked const generic (monomorphized assert) |
| `state_keys!` unit variant | 1 (just the discriminant) | previously 32 (discriminant + zero pad) |
| `state_keys!` tuple variant | `1 + Payload::MAX_LEN` | previously 32 (discriminant + payload + zero pad) |
| `#[derive(HookKey)]` struct | the struct's own `ToBytes::MAX_LEN` (`<= 32`, checked at derive time) | previously always 32 (fields + zero pad) |
| `crate::types::StateKey` | 32 (unchanged) | already a full key, nothing to shorten |

This is a **breaking change** in what bytes reach the host for every key
shorter than 32 bytes: previously, this crate right-padded a short key to a
full 32 bytes locally (value first, zero bytes after) and sent all 32 to
the host; now it sends only the key's own real bytes, and the host's own
left-pad (zero bytes first, value at the end) determines the actual
storage slot. **A key shorter than 32 bytes now lands on a different
on-ledger slot than it did before this change** — existing state written
under the old right-padded scheme is not reachable through the new
left-padded one. New deployments are unaffected; a hook upgrading across
this change would need a one-time state migration if it has existing
short-keyed entries (full 32-byte keys, including every `crate::types::StateKey`
use, are unaffected either way).

The rationale for not padding locally: it makes "how many of these 32
bytes does this key actually use" explicit at the call site (`b"RR"` is
visibly a 2-byte key, not silently a 32-byte one under the hood), and it
matches the host's own convention exactly — a Rust hook and a C hook
passing the same short literal key now land on the exact same slot, with
no distinct-encoding-scheme footgun between the two.

Neither `pad!` nor `pad_left!` (see §5.6's "`pad!` vs. `pad_left!`"
paragraph) is needed to build a hook-state key anymore: reach for a plain
`[u8; N]` (e.g. `b"counter"`) directly instead, and let the host's own
left-pad do the rest. Both macros remain useful for other fixed-size
buffer needs unrelated to `StateKeyEncode` keys.

### 5.8 Typed slot layer: `SlotObject<T>` / `SField<T>`

The Hook API's slot family is 255 numbered registers holding deserialized
ledger objects and transactions. `api::slot` mirrors it one function per
host call; `slot_obj` puts a type on the handle:

```rust
let account = SlotObject::from_keylet(&keylet_account(&accid)?)?;
let seq: u32 = account.get(sfSequence)?.value()?;
let bal: XFL = account.get(sfBalance)?.as_xfl()?;
```

Slot numbers never appear in hook source. The decisions behind it:

- **Generated typed field constants.** `sfield.rs` is generated by
  `cargo xtask gen-core` from the same `sfcodes.h` parse that produces
  rshooks-core's raw table (the `tx_type.rs` precedent), so `typed.code() ==
  raw` holds by construction — and is pinned anyway by a parity test that is
  itself generated into `sfield.rs`, one assertion per constant, so it covers
  all 325 names and cannot drift when upstream adds a field. The value type is a pure function of the serialized type ID packed
  into the code; IDs this layer does not model (`Blob`, `PathSet`,
  `Vector256`, `Number`, `UInt192`, `XChainBridge`, and `Hash160`, whose four
  fields carry different semantics) map to `Opaque`, which navigates and
  reads raw bytes but has no `value()`.
- **Affine handles: no `Copy`, no `Clone`, no `Drop`.** A `Copy` handle plus
  a consuming `clear` would let a stale copy read or clear a slot the host
  has since reassigned. No `Drop` because cleanup is fallible and would cost
  instructions on every exit path, including paths that touched no slot; the
  255-slot budget is per-execution and host-freed, so a leak inside one
  execution costs only the slot. Affinity is what makes that safe — a leaked
  handle is gone from the type system rather than able to alias.
- **Terminal reads consume and do not clear** (a USER decision). `value`,
  `as_xfl`, `raw`, `raw_exact` take `self`; the slot lives until the hook
  ends. That is byte-for-byte the C cost model — a C `slot_subfield` + read
  leaks identically — and an implicit clear would bill every read for a host
  call C never pays. `08_slot-ledger` measures the consequence: the typed
  rewrite is **12 instructions and 40 bytes cheaper** than the raw original,
  precisely because it drops three `slot_clear` calls the C-idiomatic version
  never needed.
- **`take_*` for loops.** `take_value`/`take_xfl`/`take_raw_exact` read *and*
  clear, on the success and failure paths both. A loop deriving one child per
  iteration needs them (or an explicit clear) or it exhausts 255 slots.
- **Parent-aware, sealed navigation.** `SlotKey<Parent>` gates keys by parent
  type: `SlotObject<STObject>::get(0)` and `SlotObject<STArray>::get(sfX)`
  are compile errors. Sealed via a private `Resolve` supertrait, because a
  downstream implementation could return an arbitrary slot number and forge
  an aliasing handle. `CastTarget` is sealed for the same reason —
  `STObject` accepts serialized type ID 14 *and* the 10001–10004 codes root
  slots report, every other target exactly its own ID; any `try_cast` failure
  consumes the handle and best-effort clears the slot.
- **`slot_path!` clears intermediates.** `root.get(a)?.get(b)?.get(c)?` leaks
  two slots. The macro emits a match ladder whose per-hop order is `let next
  = cur.get(k); let _ = cur.clear(); match next {..}` — the current handle is
  cleared *unconditionally*, so a later failing hop cannot leak it. The root
  is borrowed and never cleared. Clearing a parent after deriving a child is
  sound because the host copies the parent's storage into the child slot;
  that is pinned by a live e2e test, not assumed. Measured nesting after
  rshooks-build's unnest pass is **1** at 1, 3 and 10 hops — far under the
  guard checker's 32 — and WCE grows linearly (46 / 94 / 255 instructions).
- **MPT is out of scope** (a USER decision). MPT amounts need an amendment
  Xahau does not have. `AmountBytes`/`IssueData` classify by length and
  return `ParseError` for any unexpected size (33-byte MPT amounts and
  44-byte MPT issues included) rather than misclassifying, and `as_xfl` is a
  direct `slot_float` call with no guard — whatever the host would do with a
  hypothetical future MPT amount is the host's behavior, inherited unchanged.
  A guard would tax every ordinary read for a case that cannot arise.
- **`u64` reads its wire bytes, not as-int64.** The host rejects a value with
  bit 63 set in as-int64 mode, and legitimate 64-bit fields set it
  (`sfExchangeRate`). `u8`/`u16`/`u32` keep the as-int64 path, where bit 63
  is unreachable.
- **`field_code` returns an erased `SField<Opaque>`, not a `u32`**, so
  `slot.field_code()? == sfBalance` reads as a field comparison rather than
  arithmetic. `Opaque` because the value type genuinely is not known here —
  this is a code to compare or unwrap with `.code()`, not a licence to read.
  `SField`'s `PartialEq` is hand-written and cross-parameter (`impl<A, B>
  PartialEq<SField<B>> for SField<A>`, comparing codes alone) so the erased
  result matches a constant of any `T`, in either direction; a derived impl
  would only ever compare `SField<T>` with itself. Root slots report
  10001–10004 codes, which name no ordinary field and so compare unequal to
  every constant in `sfield`. `try_cast` keeps comparing raw `u32`s
  internally, where the serialized type ID is being extracted anyway.

**Where the types live.** `SField<T>` and the wire-type markers (`Amount`,
`Issue`, `STObject`, `STArray`, `Opaque`) are hand-written in `types.rs`, not
in `slot_obj.rs`, and the generated `sfield.rs` names only `crate::types::*`.
A field constant describes the *wire format*; making it depend on the slot
layer that happens to read it had the dependency backwards, and would have
meant a hook could not name a field type without pulling in slot machinery.

**`txn_template!` and `txn::codec` take the typed constants** (a user
decision superseding the earlier raw-`u32`-only grammar). `field_header`,
`write_field_header` and the four `*_field_size` helpers are generic over
`SField<T>` and take the constant itself — `SField` is a `u32` at runtime, so
this costs nothing, and it is the *only* signature they have: `SField::new`
is `pub(crate)`, so a raw-`u32` overload would be the one way to build a
header for a code no generated constant names. Nothing in the repo needs a
computed code; if an internal caller ever does, it gets a private helper
rather than a public `_raw` variant.

Inside the macro the same constants flow straight through to those
functions. `.code()` survives only where the expansion genuinely stores or
compares a `u32` *value* — the `order = [...]` canonical-order array and the
`FIELDS` table rows that `field_present`/`field_kind_ok`/`field_offset_or`
look codes up in — which is invisible to the declaration site. The
consequence for callers is that a bare `u32` expression no longer works as a
field code, in a template or in a const builder; both directions are pinned
by trybuild fixtures. `rshooks::raw::sfcodes` stays exported, but nothing
in the repo reads it outside tests.

**Breaking changes** this introduced: the typed `sfXxx` constants replaced
the raw `sfcodes` glob in the prelude (raw table at
`rshooks::raw::sfcodes::*`); the numbered slot functions left the prelude
(explicit `rshooks::api::slot::*` / `rshooks::api::otxn::otxn_slot`);
every runtime field-code parameter widened to `impl Into<u32>`, so a bare
integer literal there now needs a `u32` suffix; and `txn_template!` takes
typed constants only.

**Every example uses the typed layer.** No example calls a numbered slot
function. The two production hooks needed care: `80_reward` measured nesting
**68** with five typed reads inlined into its entry point (the limit is 32)
and came back to 26 via an `#[inline(never)]` extraction *plus* replacing a
4-way tuple `let (Ok(..), ..) = .. else` with sequential ones — the tuple
pattern lowers to nested matches. `81_govern`, the hook with the least
headroom in the repo, was unchanged at nesting 22 because `slot_path!`
flattens a three-hop walk into one `if let` where the raw chain was three
nested ones. Costs: 80_reward +220 instructions, 81_govern +83, both from
`Result` plumbing and the 34-byte keylet copy `from_keylet` makes where the
raw `slot_set` took a slice. `07_xfl-math` and `08_slot-ledger` got
*cheaper* (−10 and −12), having dropped `slot_clear` calls the consuming
reads make unnecessary.

### 5.9 Generated format views: `views::{tx, ledger, inner}`

Upstream declares 74 transaction types, 34 ledger entry types and 28
inner-object formats, and amendments change their field lists. Hand-writing
one read view per type does not scale, so all of them are **generated** —
the same pattern §4 already applies to `sfcodes.h → sfield.rs` and
`tts.h → tx_type.rs`, applied to a third vendor group
(`crates/rshooks-core/vendor/xahaud-protocol/`: `sfields.macro`,
`transactions.macro`, `ledger_entries.macro`, `TxFormats.cpp`,
`LedgerFormats.cpp`, `InnerObjectFormats.cpp`). Those six files are parsed
once into the checked-in `crates/rshooks-core/protocol_formats.json`, and
every renderer reads that artifact rather than re-parsing — so a future
transaction-*builder* renderer consumes a stable, versioned input.

```rust
let p = views::tx::Payment::otxn()?;            // checks otxn_type == ttPAYMENT
let dest: AccountId = p.destination()?;          // soeREQUIRED -> Result<T>
let tag: Option<u32> = p.destination_tag()?;     // soeOPTIONAL -> Result<Option<T>>

let line = views::ledger::RippleState::from_keylet(&keylet_line(a, b, cur)?)?;
let bal: AmountBytes = line.balance()?;
```

The decisions behind it:

- **This supersedes §5.5's "no library-owned shapes" rationale, narrowly.**
  That argument was about release lag on *hand-maintained* shapes;
  `scripts/sync-vendor.sh` + `cargo xtask gen-core` refreshes every shape at
  once and `gen-core --check` fails CI on drift. Hand-written
  shape-specific code stays banned. See `txn.rs`'s module doc comment.
- **Generic over a sealed `FieldSource`.** A transaction view reads either
  the originating transaction (`OtxnSource`, a ZST over `otxn_field` — one
  host call per access, no slot consumed) or an already-loaded slot
  (`SlotSource`). Both are monomorphized and every accessor is
  `#[inline(always)]`, so the abstraction compiles away to the host call it
  wraps. Ledger and inner views are slot-only and not generic. A third impl
  over parsed bytes could be added without touching a line of generated
  code.
- **Constructors verify, by raw `u16` compare.** `Payment::otxn()` checks
  `otxn_type` against `rshooks_core::ttPAYMENT`; `RippleState::from_slot`
  checks `sfLedgerEntryType` against `ltRIPPLE_STATE`. Never a `TxType`/
  `LedgerEntryType` decode — those are ~74- and ~34-arm matches with
  §5.6's nesting cost, and a view checks its type on every construction. A
  failed check consumes and clears the slot, like `try_cast`.
- **Absence is decided on the raw return code.** `soeOPTIONAL`/`soeDEFAULT`
  fields read as `Result<Option<T>>`, with `Ok(None)` decided by comparing
  the undecoded `i64` against `DOESNT_EXIST` — never by matching
  `HookError::DoesntExist`, per §5.6's rule for rshooks's own internals. A
  view emits one optional read per optional field, so a single
  specific-variant match here would blow the nesting budget on its own.
  `soeDEFAULT` reads as `Option` too: upstream encodes "may be omitted",
  not a default value.
- **Slot-backed accessors are get → read → clear.** Every one navigates to
  a child slot, performs a terminal read, and releases the child before
  returning, through the `take_*` family (`SlotObject::take_raw` was added
  for the variable-length case). A view's accessors can be called any
  number of times and consume zero slots beyond the view's own root — the
  only place a hook pays a `slot_clear` the C idiom skips, and the reason
  a thirty-field ledger view is usable at all. The `*_slot` subobject
  accessors are the documented exception: they hand the child's ownership
  to the caller.
- **Value types are keyed on the serialized type ID**, matching the
  `SField<T>` §5.8's table already carries — with the two wire markers
  reading back as values (`Amount` → `AmountBytes`, `Issue` → `IssueData`).
  Unmodeled types (`Blob`, `PathSet`, `Vector256`, `Number`, `Hash128`,
  `Hash160`, `XChainBridge`, …) get a raw `…_into` byte accessor documented
  as raw access, not typed access; `Number` is explicitly not an `XFL`.
  `STObject`/`STArray` fields get that raw accessor on every source, plus a
  `…_slot` child-slot accessor on the slot-backed views only — `otxn_field`
  cannot navigate into a container.
- **Code, not data.** The renderer emits no `static`, no export, no
  function pointer and no registration table, because §6.2's cleaner drops
  unreachable *functions* but retains active data segments regardless of
  reachability — a lookup table would land in every hook's wasm whether it
  used a view or not. An xtask test asserts the rendered text contains none
  of them. Measured: with the views unused, all 19 example binaries are
  byte-identical in size and worst-case instructions to the build before
  this layer existed.
- **All logic lives in the hand-written `views/source.rs`.** The generated
  files are declarations that call into it — one struct and one accessor
  per upstream declaration, no branching of their own — so the reviewable
  surface is one module rather than 34k lines.
- **Not generated (v1):** `STArray` iteration sugar (compose the slot API
  with `views::inner`), builders (the artifact carries what they need;
  no builder code yet), and keylet construction, which stays in
  `api::keylet` because keylet parameterization is per-type knowledge the
  format macros do not encode — `from_keylet` just composes.

### 5.10 Format availability: `active` / `pending` / `dormant`

Upstream's format tables are inherited wholesale from rippled, so
§5.9 would otherwise generate views for a great deal Xahau cannot run —
an `AMMBid`, an `XChainCommit` — offering a hook author an API no real
transaction can match. `crates/rshooks-core/format_availability.json`
classifies every declared format, and the generator follows it.

| tier | meaning | generated |
|---|---|---|
| `active` | activated on Xahau mainnet | normally |
| `pending` | supported by xahaud, not yet activated | behind the `pending-amendments` cargo feature |
| `dormant` | gated by an amendment xahaud marks `Supported::no` (or depending on one), so it cannot activate without a node upgrade | not at all |

The decisions behind it:

- **Curated, not derived, and not vendor data.** The `dormant` half is
  objective — `features.macro` (vendored alongside the format definitions,
  as evidence only) states `Supported::no` — but the active/pending split
  is a fact about ledger state that no file in this repository can answer.
  So it is a hand-maintained list, checked in beside `protocol_formats.json`
  and outside `vendor/`.
- **Verified against the ledger, not guessed.** The current tiers were
  checked against Xahau mainnet's `Amendments` object (validated ledger
  25441901, 2026-08-30). The artifact's own `doc` block carries the
  snapshot, the `sha512half(feature_name) ∈ Amendments` membership recipe
  as a runnable one-liner, and the caveat that makes it safe: a **retired**
  amendment is unconditionally on and absent from that object, so absence
  is never grounds to demote a format to `dormant`. The ledger check only
  separates `active` from `pending`; `Supported::no` remains the sole
  `dormant` criterion.
- **One automatic mutation, in the safe direction.** `gen-core` appends any
  newly declared format as `dormant` and does nothing else; moving an entry
  between tiers is a human decision. A newly vendored format is therefore
  unusable until somebody looks at it, rather than silently exposed.
  `gen-core --check` fails on an unclassified format (pointing at
  `gen-core`) and on a classification naming a format upstream no longer
  declares, so the two files cannot drift apart in either direction.
- **The ergonomic layer follows availability; the raw layers do not.** A
  `rshooks::sfield` constant exists only for a field some usable format
  references — best tier among the referencing formats, so one active
  format is enough to keep a field. `rshooks-core`'s `sfcodes`/`tts`/`lets`
  stay complete 1:1 mirrors, and `TxType`/`LedgerEntryType` stay exhaustive.
  That line is deliberate: those decode wire values rather than offer
  capability, and a decoder that cannot name a code it might receive is
  strictly worse than one that can. Nothing is unreachable either way —
  `rshooks::raw::sfcodes::sfXxx` is always there and every field-code
  parameter takes `impl Into<u32>`.
- **A field no format references stays active.** Those are structural, not
  amendment-borne: metadata fields, hash and index plumbing, the four
  container-typed pseudo-fields. The imprecision this accepts is that a
  field reachable only from inside an opaque wire type
  (`sfLockingChainIssue` inside `sfXChainBridge`) looks structural and
  survives as a typed constant no Xahau object will contain.
- **Pending views and their fields share one gate**, so a `pending` view
  compiles together with the `pending`-only constants it reads, or not at
  all. `mise run lint`/`test` each run one extra invocation with the
  feature on, because it is a genuinely different tree of code.
- **Measured:** classification removes and gates only code nothing used, so
  all 20 example binaries are byte-identical to the pre-classification
  build.

## 6. rshooks-build

`std` crate: `src/main.rs` (clap CLI) + `src/lib.rs` (pipeline as pure
`bytes → Result<bytes>` functions, unit-testable). Dependencies: `clap`,
`wasmparser`, `wasm-encoder`, `anyhow`; dev-dep `wat` for fixtures.
No walrus (C8).

### 6.1 CLI

```
rshooks build [--manifest-path <dir/Cargo.toml>] [-p <crate>]
                  [--api-version 0|1] [--auto-guard] [--default-maxiter N]
                  [--out <dir>] [--allow-oversize]
rshooks clean <in.wasm> [-o out.wasm] [--api-version 0|1]   # post-process only
rshooks check <file.wasm> [--api-version 0|1]               # validate only, no output
```

`build` =
1. `cargo build --release --target wasm32v1-none` with
   `--message-format=json` to locate the produced `.wasm` artifact (no
   guessing at target dirs).
2. Recover an optional `metadata!` declaration from its build-only carrier
   export in the raw artifact (6.6).
3. Post-process (6.2, 6.3).
4. Validate (6.4).
5. Write `<out>/<crate>.wasm` (default `out/` beside the manifest), print
   size and estimated SetHook fee (`bytes × 5000` drops). When metadata was
   declared, also write `<out>/<crate>.json` with the final HookHash and WCE.

`check` runs only 6.4 (+ guard verification instead of insertion) — usable
against any wasm, including C-built hooks.

### 6.2 Cleaner (hook-cleaner equivalent)

Input: cargo's wasm. Output: SetHook-shaped wasm.

1. Drop all custom sections (`name`, `producers`, `target_features`, …).
2. Restrict exports to exactly `hook` and (if present) `cbak`; everything
   else — `memory`, `__wasm_call_ctors`, data/table globals — is removed.
3. Reachability GC. Roots: the retained exports only (`call_indirect` is
   rejected in v1 — see 6.4 — so table element segments are never roots;
   a `start` section is rejected outright). Traversal follows direct
   `call` instructions and `global.get/set`. Then:
   - drop unreferenced functions and globals;
   - drop the table and all element segments entirely when no
     `call_indirect` survives (v1: always, given the hard error);
   - trim every active data segment's payload to end at its last non-zero
     byte (dropping the segment entirely if it is all zero) — wasm linear
     memory is zero-initialized by definition, so trailing zero bytes are
     pure dead weight at 5000 drops/byte; only the payload shrinks from
     the tail, the offset expression is untouched, and this preserves
     semantics since memory size comes from the memory section, not
     segment lengths. **Safety guard**: active segments apply in
     declaration order and may legally overlap, in which case a trailing
     zero can be a deliberate overwrite of an earlier segment's non-zero
     byte — so the trim runs only when every offset is a plain
     `i32.const` and no two segment ranges intersect (LLVM/wasm-ld never
     emit overlaps, but `clean` accepts arbitrary wasm). Segment
     *merging* remains a future optimization; a live defined memory is
     required whenever at least one (untrimmed) segment survives.
4. **Index renumbering is a whole-module concern.** One remap table per
   index space (types, functions, globals, memories, tables) is built once
   and applied everywhere that space is referenced: function section,
   export section, element segments, code bodies (`call` immediates,
   `global.get/set` immediates), and import ordering (imported functions
   occupy the low indices, so adding/removing an import — e.g. `_g` —
   shifts every defined function index). A code body may be raw-copied
   **only if no index space it references changed**; otherwise it is
   re-encoded instruction-by-instruction with immediates remapped.
   Byte-comparison tests pin the re-encoder as lossless modulo remapped
   immediates (C8).
5. Verify entry signatures: `hook`/`cbak` must be `(i32) -> i64`; error out
   otherwise (catches a missing `extern "C"` or wrong signature early).

### 6.2b Flatten pass (full inlining) — api-version 0

Two rules of the real checker (`Guard.h`, discovered by running the vendored
checker against our phase-4 artifacts, which the Rust reimplementation had
wrongly accepted):

- **R1**: every api-version-0 module must import `_g`, even if it contains
  no loop at all.
- **R2**: every entry in the type section must be the type of an import or
  the `(i32) -> i64` entry-point type. A defined helper function with any
  other signature — notably `compiler_builtins` `memset`/`memcpy`/`bcmp`
  (`(i32,i32,i32) -> i32`), which rustc emits for large buffer zero-inits
  and array comparisons — makes the whole module invalid.
  (`#![no_builtins]` does not prevent these under fat LTO; verified
  empirically. Source-level avoidance is not reliable.)

Consequently the cleaner is followed by a **flatten pass** for api-version
0: inline every defined non-entry function into its callers, bottom-up in
topological order (the call graph is acyclic — recursion is banned — so
this terminates), then drop the inlined functions and rebuild the type
section to exactly {import types} ∪ {entry type}. Inlining transform per
call site: arguments are spilled to fresh locals, the callee body is
spliced in wrapped in a `block` of the callee's result type, callee locals
are remapped to appended caller locals, and every `return` in the callee
becomes a `br` to the wrapper block (branch depths inside the body shift by
one accordingly). Multiple call sites duplicate the body — a size cost that
is acceptable and reported. `_g` is ensured present as an import for
api-version 0 (added if absent, never GC'd) per R1.

Inlining wraps a call site in a `block` only when the callee body actually
contains a non-trailing `return` (the block exists solely as the rewritten
`br` target); a trailing `return` is dropped and falls through, and
return-free bodies are spliced bare.

### 6.2c Unnest pass (ladder flattening) — api-version 0

`Guard.h` rejects modules whose block nesting exceeds **32 levels**
(`NESTING_LIMIT`, 16 before `GuardRuleDepth32`) during its worst-case
analysis. LLVM's stackifier lays out every diverging early-exit
(`rollback!`-style) as a tail after the end of a dedicated `block` wrapping
the whole remaining body — so nesting grows linearly with the number of
error paths (the "error ladder"), and a hook with a few dozen checks would
exceed the limit regardless of guards.

The unnest pass runs after flatten and exploits the fact that those tails
are **self-contained and diverging** (push constants, `call rollback`,
`unreachable` — consuming nothing from the outer stack):

1. **Diverging-tail duplication**: a `br_if` targeting a block whose
   continuation is such a tail is rewritten to `if` + the tail spliced
   inline (an unconditional `br` gets the tail spliced directly). The tail
   is verified self-contained by symbolic stack simulation (starts empty,
   never underflows, only constants / `local.get` / import calls / `drop`,
   ends in `unreachable`); only empty-blocktype blocks qualify.
2. **Unreferenced-block unwrapping**: any empty-blocktype block no longer
   targeted by any branch is removed, with branch-depth immediates inside
   it fixed up. This also erases flatten wrapper blocks whose `return`
   rewrites never materialized.
3. Iterate to fixpoint (ladders unwrap outermost-inward).
4. **Dead-code elimination**: unwrapping a block in step 2 leaves its
   original continuation in place, now sitting in straight-line code right
   after the unconditional terminator (`unreachable`/`br`/`br_table`/
   `return`) that used to end the block body — unreachable, but still
   counted by the worst-case analysis, which sums instructions
   syntactically rather than by reachability. A final linear pass drops
   every instruction following such a terminator up to the closing
   `end`/`else` at that nesting level, dropping a nested block/loop/if
   encountered while already dead as one whole unit rather than descending
   into it. No branch's `relative_depth` is affected, since every
   surviving frame is untouched and every dropped frame's branches are
   dead code too.

The local `if` costs one level only inside a short error arm, while each
removed ladder block spanned the entire function — net max-depth drops from
O(error paths) to O(real control structure). The duplicated tails cost a
few bytes per branch site. Correctness is held by the same wasmi
differential harness as flatten (identical results and host-call
sequences pre/post pass). The validator (6.4) additionally computes max
nesting depth, hard-erroring above 32 for api-version 0 (mirroring
`GuardRuleDepth32`) and warning at ≥ 28; `build` prints the final depth.

The guard pass (6.3) runs **after** flattening and unnesting, so loops that
arrive in `hook()` by inlining (memset/bcmp loops) get guards like any
other loop.
Correctness of the inliner is tested differentially: fixture modules are
executed pre- and post-flatten in a wasm interpreter (dev-dependency) with
recorded host stubs, asserting identical results and host-call sequences —
an inlining bug must fail tests, not silently change hook semantics.

### 6.3 Guard pass (guard-checker equivalent + auto-insert)

Skipped entirely when `--api-version 1`.

For every function body, scan instructions; at each `loop` opcode:

- If the body already starts with `i32.const a; i32.const b; call $_g`
  (optionally followed by `drop`) — accept it, record `(a, b)`.
- Otherwise it is a **hard error by default**, reported with function index
  and instruction offset (pure guard-checker behavior, same as `check`).
  Developers fix it with `guard!` at the top of the loop body.
- With opt-in `--auto-guard`, the missing guard is instead inserted:
  `i32.const <id>; i32.const <maxiter>; call $_g; drop` immediately after
  the `loop` blocktype, id = `(1 << 30) + n` (sequential — disjoint from
  the `(1 << 31) + …` space used by `guard!`/`guard_m!`), maxiter =
  `--default-maxiter` (default 16, deliberately small). Auto-guard exists
  primarily for **compiler-generated loops** the developer never wrote —
  `compiler_builtins` `memcpy`/`memset` loops are the known offenders on
  `wasm32v1-none` (no bulk-memory ⇒ byte loops). Whether examples can stay
  guard-clean without it is validated empirically in phase 4; if they
  cannot, revisit the default with that evidence.

Rationale for default-off (review finding): silent insertion with a small
maxiter can turn into runtime `GUARD_VIOLATION`s, and it hides the real
worst-case instruction budget that SetHook fee estimation is based on.

**Phase-4 empirical results** (2026-07-23, confirming both sides of this
trade-off):
- Compiler-generated loops are real. `firewall`'s `[u8; 20]` equality
  lowers to a bcmp-style byte-compare loop; `emit-txn`'s 320-byte buffer
  zero-init lowers to a `compiler_builtins`-style memset function with 5
  loop constructs. Neither has any loop in Rust source; both need
  `--auto-guard`.
- Straight-line hooks (`accept-all`) and hooks whose only loops are
  source-level with `guard!` (`state-counter`) build clean with no flags —
  the strict default is workable.
- **The `--default-maxiter` CLI default must not be trusted for real
  deployments**: rshooks-build validates guard *shape*, not that maxiter
  covers the loop's true runtime bound. maxiter 16 passes `check` for both
  examples above yet would raise `GUARD_VIOLATION` on-ledger (the compare
  loop runs up to 20 iterations; the memset bulk loop ~40). The examples
  size maxiter from disassembly (24 and 48 respectively) and the CLI docs
  must tell users to do the same.
- **Post-vendoring correction**: the four phase-4 artifacts, though accepted
  by the Rust reimplementation, are all rejected by the real (vendored)
  checker — see 6.2b R1/R2. The guard findings above remain valid (the
  loops still exist and still need guards), but compiler-generated loops
  additionally must be *inlined into the entry function* (flatten pass);
  their guards are then inserted at the inlined loop heads.
- **Static-buffer idiom** (added after the `emit-txn` rework): constant
  templates belong in `static`s (⇒ data segment, not a runtime chain of
  store instructions) and large zero-initialized buffers in zero-init
  `static`s (⇒ BSS: no data bytes, no code, and **no compiler-generated
  memset loop at all**). Exclusivity is sound — hooks are single-threaded
  and each invocation gets a fresh instance — and is packaged safely as
  `rshooks::static_cell::HookStatic<T>` (take-once cell: `take()` yields
  the one `&'static mut`, second call returns `None`; the only `unsafe`
  lives inside rshooks, and hook code needs no `unsafe` and no clippy
  allows). This removed emit-txn's memset entirely: no `--auto-guard`,
  WCE 6798 → 331 and 1272 bytes total (current-toolchain measurement, at
  this workspace's `opt-level = 3` default — see C6 above; exact figures
  drift with compiler versions and profile settings, `rshooks build`
  prints the authoritative numbers for any given build). The take-once
  flag costs a few dozen bytes over a raw `static mut`, which in turn
  required
  `unsafe { &mut *&raw mut }` plus a `clippy::deref_addrof` allow at
  every site). Source-level avoidance of
  *initialization* libcalls is thus reliable via statics; comparison
  libcalls (bcmp from `[u8; N]` `==`) still need `--auto-guard` (see
  firewall).

If a guard was inserted and `_g` is not imported, the import is added
(import section rewrite ⇒ function index shift ⇒ handled by the same
renumbering machinery as GC).

**After any mutation, the full guard verifier and validator (6.4) run again
on the final bytes** — `build` never emits an artifact that `check` would
reject; a bug in the insertion pass fails the build instead of shipping.
For api-version 0 the authoritative final verdict comes from the vendored
upstream checker (6.5), not from the Rust reimplementation.

`emit()` reachability note: xahaud requires hooks that `emit` to have called
`etxn_reserve`; that is runtime behavior, not validated here.

### 6.4 Validator

Hard errors (module shape — the SetHook-derived rule set; the final
authority is `SetHook.cpp`, and phase 3 includes cross-checking every rule
against xahaud source plus a known-good C-built hook fixture):
- Any export other than `hook`/`cbak`; missing `hook`; wrong signatures.
- Any import from a module other than `env`, or a function name outside the
  whitelist (the whitelist is generated from `extern.h` — single source of
  truth shared with rshooks-core; kept as a checked-in table with a test that
  it matches rshooks-core's extern block). Import signature mismatch against
  `extern.h` types. Imported memories, tables, or globals.
- A `start` section.
- Passive data/element segments, `data count` section, or any element
  segment form beyond MVP active-with-function-indices.
- More than one (or zero, when data segments exist) defined memory;
  memory initial size beyond xahaud limits.
- Any floating-point opcode; any post-MVP opcode (encoding-level check via
  `wasmparser` configured to MVP-only features).
- `call_indirect` (v1 hard error: it defeats the recursion check and
  reachability analysis; revisit only with conservative table analysis).
- Call-graph cycle (recursion) — DFS over direct calls (C5); sound because
  `call_indirect` is banned.
- For api-version 0: any unguarded `loop` (both `check` mode and the
  post-mutation re-verification in `build`).
- For api-version 0: missing `_g` import (6.2b R1), and any type-section
  entry that is not an import's type or the entry-point type (6.2b R2).
- For api-version 0: block nesting depth > 32 (`Guard.h` `NESTING_LIMIT`
  under `GuardRuleDepth32`; warning from depth 28 — see 6.2c).
- Binary > 65,535 bytes. (`build` refuses to emit; `--allow-oversize`
  writes the artifact anyway for size-debugging, clearly marked INVALID.)

Warnings:
- Mutable defined globals beyond the shadow stack pointer pattern (allowed,
  but flagged as a size/audit smell).
- Size approaching the limit (≥ 56 KiB) — printed with the fee estimate.

Validation always runs against the **final output bytes** (post-clean,
post-guard), and `check <file>` applies the identical rule set to arbitrary
external wasm (including C-built hooks).

### 6.5 Verdict authority: the vendored upstream checker

The final accept/reject verdict for API-version-0 modules comes from
**xahaud's own guard checker, compiled into rshooks-build from vendored,
byte-identical upstream source** — not from a Rust reimplementation. A port,
however careful, can diverge from what the node actually runs; the checker
is consensus logic, not a reference tool, so divergence means "rshooks-build
says valid, SetHook says `temMALFORMED`" (or worse, vice versa).

Vendored files (upstream `Xahau/xahaud`, branch `release`, kept verbatim —
never hand-edited; re-sync only via `scripts/sync-vendor.sh`, which also
regenerates the `SHA256SUMS` tripwire file; CI verifies byte-identity
against upstream on every push/PR and weekly —
`.github/workflows/vendor-sync.yml`):

- `include/xrpl/hook/Guard.h` — `validateGuards()` / `check_guard()`
- `include/xrpl/hook/Enum.h` — log codes, `APIWhitelist`,
  `getImportWhitelist()`, guard-rules versioning
- `include/xrpl/hook/hook_api.macro` — the API table behind the whitelist

Upstream explicitly supports standalone compilation via
`-DGUARD_CHECKER_BUILD` (Enum.h stubs `uint256`/`Rules` with an
"all amendments enabled" `Rules`, which also yields the current
`getGuardRulesVersion` bit set). A small `guard_shim.cpp` (the only C++ we
author) exposes one `extern "C"` entry point: bytes in → verdict, the
upstream log text (captured from `GuardLog`), and on success the
worst-case instruction counts for `hook()`/`cbak()` that `validateGuards`
computes. Built by `build.rs` via the `cc` crate (C++17); a host C++
compiler becomes a build requirement of rshooks-build.

`validateGuards` covers far more than loop guards (imports vs whitelist,
export shape, `call_indirect`, memory limits, custom sections, instruction
legality), so the division of labor is:

- **C++ vendored checker** — authoritative pass/fail for api-version 0, in
  both `check` and post-transform `build`. Its captured log is printed on
  failure verbatim; on success the instruction counts are reported (they
  are also what SetHook fee estimation derives from). Note: these are
  *syntactic* worst-case counts (a host `call` counts as 1; host-function
  work is not modeled), and the node's live `HookInstructionCount` meter
  can exceed them for tiny functions — observed live: emit-txn's `cbak`
  10 vs static 7 (see docs/E2E-TESTING.md). They are a fee-estimation
  input, not a runtime ceiling.
- **Rust pipeline (6.2–6.4)** — everything the checker does not do
  (cleaning, auto-guard insertion, the 65,535-byte size gate, fee
  estimate, api-version 1 checks) plus pre-transform diagnostics with
  precise function/offset locations, which upstream's log lacks. If the
  Rust validator and the C++ checker ever disagree, the C++ verdict wins
  and the disagreement is surfaced as a rshooks-build bug.

The cleaner remains native Rust (upstream hook-cleaner is a separate
project, and cleaning is a transform whose output the authoritative checker
then judges); `wasmparser`/`wasm-encoder` byte-exactness (C8) is unchanged.
Behavioral reference tests compare verdicts on known-good/known-bad
fixtures, including the built examples.

### 6.6 Build-only Hook metadata

A Hook crate may declare deployment metadata at module scope. Transaction
types use the unit variants from `rshooks::tx_type::TxType`; the generated
JSON uses their canonical Xahau `TransactionType` spellings.

```rust
metadata! {
    name: "emit-txn",                         // required, non-empty
    description: "Emits a payment",          // optional
    HookOn: [Invoke],                         // optional alternative 1
    HookCanEmit: [Payment],                   // optional
    HookName: "emit-tx",                     // optional, 2..=8 Unicode chars
}

// HookOnV2 alternative: both fields are required when either is present,
// and HookOn is absent.
metadata! {
    name: "directional",
    IncomingHookOn: [Payment, Invoke],
    OutgoingHookOn: [Invoke],
}
```

`HookOn` is mutually exclusive with the incoming/outgoing pair. All three
trigger fields may be omitted; the sidecar represents the all-zero raw
`HookOn` value as `null`. Duplicate fields, duplicate transaction types,
unknown variants, a half-specified directional pair, and equal
incoming/outgoing sets are compile errors; use `HookOn` for equal sets.
`HookCanEmit` being absent is distinct from an explicitly empty list.

The proc macro serializes the declaration as compact JSON, hex-encodes it,
and places it in the name of a wasm-only, unreachable export whose prefix is
`__rshooks_metadata_v1_`. It does not put metadata in a static or a custom
section: with Rust's wasm linker that payload can also become a live active
data segment. `rshooks-build` reads the carrier before cleaning; the normal
export restriction and reachability GC then remove both the extra export and
its function. Tests compare cleaned modules built with and without a carrier
byte-for-byte.

The sidecar puts SetHook-ready raw values at the top level and preserves the
source declaration under `human`. `HookOn`, `HookOnIncoming`,
`HookOnOutgoing`, and `HookCanEmit` are uppercase 32-byte bitmasks;
`HookName` is uppercase UTF-8 hex. When all trigger fields are absent, the
all-zero `HookOn` is represented as `null`; otherwise the regular `HookOn`
form and the `HookOnIncoming`/`HookOnOutgoing` form are output exclusively.
`human` follows the same choice and holds the corresponding readable
transaction-type arrays and Hook name; `HookHash` is intentionally top-level
only.

```json
{
  "name": "emit-txn",
  "HookOn": "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF7FFFFFFFFFFFFFFFFFFBFFFFF",
  "HookCanEmit": "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFBFFFFE",
  "HookName": "656D69742D7478",
  "HookHash": "DDAF35A1...64 uppercase hex characters",
  "WCE": { "hook": 4150, "cbak": 0 },
  "builder": {
    "name": "rshooks-build",
    "version": "0.1.1",
    "rustc": "rustc 1.89.0 (29483883e 2025-08-04)"
  },
  "human": {
    "HookOn": ["Invoke"],
    "HookCanEmit": ["Payment"],
    "HookName": "emit-tx"
  }
}
```

`HookHash` is SHA512-Half of the exact final cleaned WASM bytes, matching
`SetHook`'s hash of `CreateCode`. API-version-0 WCE values come from the
vendored authoritative guard verdict; API version 1 writes `null` for both
values because static WCE is not calculated for gas hooks. The final module's
reachable `env::emit` import is also cross-checked against `HookCanEmit`: a
declaration without emit usage and emit usage without a declaration both
produce build warnings.

`builder` records the toolchain provenance of the build itself: the
`rshooks-build` package name and version, and the full first line of
`rustc -V` from the toolchain that performed it (`null` if detection
fails; detection never fails the build). Because optimization behavior can
shift across toolchain updates even for unchanged source, this is what
lets a given `HookHash`/`WCE` pair be reproduced deterministically later.

The metadata schema follows the requested 2..=8 Unicode-character rule for
`HookName`. Deployment tooling must additionally account for the current
xahaud `SetHook` validator's byte-oriented 4..=16 UTF-8-byte constraint;
`rshooks-build` warns when a declared name falls outside that byte range.

## 7. rshooks-testenv

> **Supersedes:** `rshooks_core::host` (`HookHost`/`Guest`, a generated 1:1
> mirror trait with no real consumers) is `#[deprecated]` in favor of the
> mechanism below and will be removed in the next breaking release —
> `HookHost`'s methods take raw `u32` pointers, so an implementor cannot
> soundly read a caller's buffer on a 64-bit host and it could never serve
> as a native test seam.

A mock host lets a `#[hooks]` chain entry run as plain native Rust under
`cargo test` — no wasm build, no node — with assertions on state, exit
type, and emitted transactions. Full design rationale, the fidelity rules,
and the review history live in `.claude/design/TESTENV_DESIGN.md` (internal;
not shipped); this section is the shipped-mechanism summary. The book's
["Off-Chain Unit Tests"](../book/src/testing/unit-tests.md) chapter is the
developer-facing documentation.

**The seam.** `rshooks_core::backend` (`#[cfg(all(not(target_arch =
"wasm32"), feature = "testenv"))]`, `testenv` a default-off feature on
`rshooks-core`/`rshooks`) defines `#[doc(hidden)] trait HostBackend`: one
method per bridged Hook API operation, semantic Rust types instead of FFI
signatures (owned `Vec<u8>`/fixed arrays, not `u32` pointers), every
non-control method defaulting to `NOT_IMPLEMENTED` so adding a method later
never breaks an implementor. `accept`/`rollback` are `-> !` and required — an
implementor must define how execution terminates. A thread-local
`RefCell<Option<Rc<dyn HostBackend>>>` holds at most one installed backend;
`backend::install` returns an RAII guard that restores the previous value on
drop (including during unwinding) and rejects a second `install` on an
already-occupied slot (the reentrancy guard — `TestEnv::invoke` called from
inside a running hook panics through this).

**Wrapper interception.** `rshooks`'s `api/*.rs` wrapper functions are the
only call sites touched: an inventory of every direct `rshooks_core::*` call
in the bridged families (state, otxn, hook context, ledger, control, etxn,
trace) is committed to `crates/rshooks/testenv-call-sites.txt`, and each
site gets an additive, `testenv`-gated block immediately above its raw call
that consults `backend::with_backend` first. The wasm branch of every
touched function is textually unchanged. A helper that only calls another
wrapper (`_exact`/`_typed`, `state_get`, ...) gets no block of its own — it
inherits interception from the wrapper it calls; two tests keep the
inventory honest: a source-scan test asserting set-equality between a fresh
grep and the committed file, and a spy-backend audit test driving every
bridged public API and asserting each reaches the backend. `accept`/
`rollback` diverge via `panic::panic_any` before ever reaching the raw call;
`TestEnv::invoke` catches the unwind, restoring the state snapshot first for
any payload that isn't its own exit signal (a genuine test failure is never
converted into an exit). `HookStatic<T: Clone>` gains a parallel testenv
claim path: a thread-local per-invocation take-set hands out a freshly
leaked clone of the pristine static storage once per `invoke` call, never
touching the process-global take-once flag a plain (non-testenv) `take()`
uses — a process-global mutex serializes the two paths so the testenv path
never clones storage a plain `take()` may already hold `&mut` to.

**Native entry table.** `#[hooks]`'s impl macro emits each entry's one-call
body exactly once, as a plain-Rust-ABI `fn(u32) -> i64`; both the wasm
`extern "C"` wrapper and a generated, non-wasm-gated (not feature-gated —
generated code can't see downstream features)
`impl rshooks::decl::HookChainEntries for <Chain>` forward to that same
function — the wasm wrapper as a one-line `#[inline(always)]` call, the
native table (`rshooks::decl::NativeEntry`, one row per declared entry) as a
direct function pointer. `TestEnv::invoke::<C>(index)` looks the index up in
`C::ENTRIES` and calls it directly: a direct-entry call with no `HookOn`
trigger filtering.

**The harness (`crates/rshooks-testenv`).** `TestEnv` owns a persistent
`World` (state maps, otxn, hook identity, grants, ledger fields, committed
emissions/traces) plus a fresh `InvocationContext` per `invoke` call (emit
reserve/count, state-modification/namespace/nonce budgets, the
`HookStatic` take-set) — everything the design's limits table specifies,
with boundary tests per row. `invoke` snapshots persistent state, installs
the backend, runs the entry under `catch_unwind`, then commits (`accept!`)
or restores (`rollback!`, a bare `return`, provisionally) per the outcome.
The crate `compile_error!`s at `#[cfg(panic = "abort")]`, since the whole
exit-capture mechanism depends on unwinding across the mock host boundary.

**Full Hook API surface (Phase 2).** `.claude/design/TESTENV_PHASE2_DESIGN.md`
extended the mechanism above, family by family (`float_*`, `slot_*`/
`sto_*`/`otxn_slot`/`meta_slot`/`xpop_slot`, `util_*`/`keylet_*`,
`ledger_keylet`, `hook_again`/`hook_skip`/`hook_param_set`, `prepare`,
`trace_float`), plus `TestEnv::invoke_cbak` for running a declared
`#[cbak]` body directly, with the emitted transaction standing in as its
otxn. Every `extern.h` function a hook can call is now answered by the
mock backend (`_g` excepted — guard enforcement stays a build-time/e2e
concern); see the book's ["Off-Chain Unit
Tests"](../book/src/testing/unit-tests.md) chapter for the full coverage
table and the honestly-enumerated remaining gaps (fee/reserve economics as
approximations, statics outside `HookStatic`, no chain/`HookOn` model,
amendment gates assumed active, and the `rshooks::raw` escape hatch).

**Zero wasm impact.** Every new runtime code path is `testenv`-gated or
`not(target_arch = "wasm32")`-gated; the touched wrapper functions' wasm
branches are textually unchanged; `rshooks-core/src/api.rs` (the stub layer)
is untouched; `testenv` is default-off and unreachable from `rshooks
build`'s cargo invocations (dev-dependencies are inactive for a plain
`cargo build`/`cargo rustc`, and the selected build pins `--crate-type
cdylib` explicitly, so the `rlib` crate-type a test-wired example adds never
enters the artifact path). `scripts/probe-testenv-parity.sh` is the
continuous verification: it builds every example (and a synthetic probe
matrix) twice at the same commit — pristine vs. test-wired — and asserts
byte-for-byte identity of every final `.wasm`, every raw selected `cargo
rustc` artifact, and the sidecar WCE/size/nesting numbers.

## 8. examples/

Own workspace; every crate:

```toml
[lib]
crate-type = ["cdylib"]

[profile.release]
opt-level = "z"
lto = "fat"
codegen-units = 1
panic = "abort"
strip = "symbols"
```

Directory names are numbered in suggested reading order (`01_`..`10_`);
package names themselves are not (Cargo package names can't start with a
digit) — see `examples/README.md`.

| # | example | demonstrates |
|---|---|---|
| 01 | `accept-all` | minimal hook: `accept` everything (starter template) |
| 02 | `state-counter` | `state`/`state_set` round-trip, counter in hook state |
| 03 | `hook-params` | `#[hook_param]`-configurable threshold, with a compiled-in default |
| 04 | `errors` | a meaningful `hook_errors!`-based rollback error-code system |
| 05 | `firewall` | read `otxn_field(sfAccount)` + hook param blacklist → `rollback` |
| 06 | `guard-patterns` | `guard!`/`guard_m!` correctness and the array-`==` memcmp-loop pitfall |
| 07 | `xfl-math` | reading `Amount` as XFL, `mulratio`, `Result`-based comparisons |
| 08 | `slot-ledger` | transaction field access via the Slot API |
| 09 | `state-foreign` | `state_foreign`: reading another account's hook state |
| 10 | `emit-txn` | `etxn_reserve` + a user-declared `txn_template!` Payment + `cbak` |

Each README shows the exact build command:
`rshooks build --manifest-path examples/02_state-counter/Cargo.toml`
(or via mise task `mise run build-examples`, which builds all examples and
`check`s the outputs — this doubles as the end-to-end test).

Source style rules for examples (enforced by review, documented in
`examples/README.md`): no slice indexing that can panic (use fixed-size
arrays and `split_at`-free patterns), no `format!`/`fmt`, loops carry
`guard!` when the bound is known, and constant templates / large
zero-initialized buffers live in `static`s (data segment / BSS) rather
than stack locals (see §6.3's static-buffer idiom).

## 9. Code health

- `rustfmt.toml` (defaults; `edition = "2024"`), formatting enforced.
- Workspace lints in root `Cargo.toml`, inherited via `[lints] workspace = true`:
  `rust.unsafe_op_in_unsafe_fn = "deny"`, `clippy.all = "warn"`,
  `clippy.pedantic` selectively, `rust.missing_docs = "deny"` for the two
  library crates.
- **Panic-free is enforced, not promised** (review finding): rshooks and
  every example crate additionally deny `clippy::unwrap_used`,
  `clippy::expect_used`, `clippy::panic`, `clippy::indexing_slicing`, and
  `clippy::arithmetic_side_effects` is at least `warn`. The documented
  contract: rshooks wrappers are panic-free; hook crates keep that
  property only by passing these lints (checked by `mise run lint`).
- `mise.toml` tasks: `fmt`, `lint` (clippy `-D warnings`, both workspaces,
  host + wasm32v1-none targets), `test`, `build-wasm`, `build-examples`.
  Target-specific caveats: `build-wasm` scopes to `-p rshooks-core -p
  rshooks` (rshooks-build is a std CLI and must not be built for
  wasm32v1-none), and clippy for the examples workspace uses `--lib`, not
  `--all-targets` — wasm32v1-none has no `test` crate, so the implicit
  test-profile target can never build there regardless of an example's own
  `[lib] test` setting. Some examples additionally carry host-only `tests/`
  targets and dev-dependencies (§7) — those are exercised by `cargo test`,
  not by this wasm32v1-none lint pass.
- Tests: rshooks-build unit tests on `wat`-authored fixtures (cleaner strips
  exports; guard inserted at loop head byte-exactly; recursion detected;
  float opcode rejected); rshooks-core has a test asserting the whitelist
  table and extern block stay in sync; examples built+checked in
  `build-examples`.
- `.gitignore`: `/target`, `/examples/target`, `/examples/**/out`, `out/`,
  `*.wasm` outside fixtures, `.DS_Store`. Binary test fixtures live in
  `crates/rshooks-build/tests/fixtures/` and are exempted.

## 10. Typed entry return values (breaking change: typed-only)

`docs/TODO.md` item 2, designed and probed in
`.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md` (full grammar, adoption
decisions D1–D6, and every measured number — this section only summarizes
the shipped, normative shape).

A `#[hook]`/`#[cbak]` entry returns `rshooks::exit::HookResult`
(`Result<Accept, Rollback>`) — the only return type the sealed
`EntryReturn` trait implements (D6): the previous `i64` identity impl has
been removed. `accept!`/`rollback!` remain public and usable inside a
typed entry's body — they diverge, so they coerce to `HookResult` — and are
documented as the in-body escape hatch for a computed, non-`'static`
message, or a raw, zero-indirection body.

- **`rshooks::exit` module**: `Accept`/`Rollback` (private `{ msg:
  &'static [u8], code: i64 }`, `::new(msg, code)`/`::from_code(code)`
  constructors), `HookResult = Result<Accept, Rollback>`, and the sealed
  `EntryReturn` trait (`#[doc(hidden)]`, implemented for exactly
  `HookResult`).
- **Macro requirement**: `#[hooks]`'s entry-signature check requires a
  return type implementing `EntryReturn` — in practice, `HookResult` only —
  and the generated body wraps every call unconditionally:
  `::rshooks::exit::EntryReturn::finish(<Struct>::<fn>(&<Struct>))`. A
  non-conforming return type (`-> i64`, or anything else) fails to compile
  with an ordinary trait-bound diagnostic naming `EntryReturn` on the
  entry's own `-> Ty` span — the macro performs no bespoke return-type
  validation of its own.
- **`hook_errors!` From impl**: every `hook_errors!` enum gets
  `impl From<Enum> for Rollback` (clause-less enums included). The optional
  `=> b"msg"` clause fills the message (else empty); a clause-less enum uses
  `Rollback::from_code` with no match. `?` therefore propagates a
  `hook_errors!` variant — code and message both — straight into a typed
  entry's `Err` side.
- **Deliberately no `From<HookError> for Rollback`.** `HookError::code()` is
  a 46-arm re-encode match; a `?`-propagated two-hop conversion measured
  3.1x the worst-case instructions and +67% the size of a raw-code-check
  twin (design doc §5, probe P5). The supported pattern is
  `.map_err(|_| MyError::X)?`, discarding the decoded `HookError`.
- **Migration cost, measured**: `EntryReturn::finish`'s match is dead code
  on any path that always diverges through `accept!`/`rollback!`, so a
  signature-only migration (keeping an entry's raw internals, as
  `examples/80_governance`'s `govern`/`reward` do) measures near-neutral; a
  rewrite to idiomatic `Ok`/`Err`/`?` measured competitive with, or better
  than, its hand-written `accept!`/`rollback!` twin at every probed density
  (small entry, dense 15-site entry), provided every `?`-called helper is
  `#[inline(always)]` (D4) — see design doc §5–§7 and
  `examples/16_typed-results`'s/`examples/80_governance`'s `README.md`s for
  the exact per-example deltas. Every in-repo chain and snippet (examples 01–16 incl. their READMEs and the root/examples READMEs,
  `80_governance`, trybuild fixtures, testenv test chains, book snippets,
  doctests) is migrated; artifact bytes are allowed to change as part of
  this breaking release.

Book: [Accept, Rollback, and Errors §"Typed entry returns:
`HookResult`"](../book/src/concepts/errors.md#typed-entry-returns-hookresult).
