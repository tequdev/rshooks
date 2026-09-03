# rshooks

A Rust monorepo for developing [Xahau](https://xahau.network/) Hooks
(WebAssembly smart contracts) end to end — from raw Hook API bindings to
one or more SetHook-valid `.wasm` binaries and a generated `SetHook`
transaction template.

⚠️ Alpha software — here be dragons🐲. Expect bugs and breaking changes.

See the [`book/`](book/src/introduction.md) user guide for a full
walkthrough, or [`docs/DESIGN.md`](docs/DESIGN.md) for the underlying
design.

## Crates

| crate | description |
|---|---|
| `rshooks-core` | `no_std`, zero-logic FFI layer: raw Hook API declarations and every constant from the xahaud `hook/` headers, translated 1:1 into Rust. |
| `rshooks-macros` | Procedural macros for `rshooks` (the `#[hooks]` struct/impl attribute, XFL literals, and more). |
| `rshooks` | `no_std`, ergonomic wrapper over `rshooks-core` (`Result`-based APIs, typed buffers, XFL type, guard/trace macros, panic handler). |
| `rshooks-build` | CLI that turns a Rust crate into one or more SetHook-valid WASM binaries: a discovery build plus one build per declared Hook, each cleaned and guard-checked natively in Rust. |
| `rshooks-testenv` | Off-chain unit-test harness with a mock Hook host, for testing Hook logic without WASM or a running Xahau node. |

`examples/` (a separate workspace) holds runnable Hooks built with
`rshooks`.

## Installation

Hook crates depend on [`rshooks`](https://crates.io/crates/rshooks); the
build CLI installs with `cargo install rshooks-build`, which installs a
binary named `rshooks` (run as `rshooks build`, `rshooks check`, `rshooks
clean`).

## Building

```sh
mise run build-wasm   # builds the no_std crates for wasm32v1-none
mise run lint         # cargo clippy --workspace --all-targets -- -D warnings
mise run fmt          # cargo fmt --all
mise run test         # cargo test --workspace
```

## Examples

Numbered in suggested reading order — see
[`examples/README.md`](examples/README.md) for the full walkthrough of why.

| # | example | demonstrates |
|---|---|---|
| 01 | [`accept-all`](examples/01_accept-all) | minimal hook: `accept` everything (starter template) |
| 02 | [`state-counter`](examples/02_state-counter) | `state`/`state_set` round-trip, counter in hook state |
| 03 | [`hook-params`](examples/03_hook-params) | `#[hook_param]`-configurable threshold, with a compiled-in default |
| 04 | [`errors`](examples/04_errors) | a meaningful `hook_errors!`-based rollback error-code system, matched to `HookReturnCode` |
| 05 | [`firewall`](examples/05_firewall) | read `otxn_field(sfAccount)` + a hook parameter blacklist → `rollback` |
| 06 | [`guard-patterns`](examples/06_guard-patterns) | `guard!`/`guard_m!` correctness, choosing `maxiter`, and the array-`==` memcmp-loop pitfall |
| 07 | [`xfl-math`](examples/07_xfl-math) | reading `Amount` as XFL, `mulratio`, checked XFL operators, and `XFLUnchecked`'s hot-path chain |
| 08 | [`slot-ledger`](examples/08_slot-ledger) | `otxn_slot`/`slot_subfield`/`slot`/`slot_size`: transaction field access via slots |
| 09 | [`state-foreign`](examples/09_state-foreign) | `state_foreign`: reading another account's hook state |
| 10 | [`emit-txn`](examples/10_emit-txn) | `etxn_reserve` + a user-declared `txn_template!` Payment, with a `cbak` |
| 12 | [`typed-data`](examples/12_typed-data) | `#[derive(HookData)]`/`#[derive(ParamValue)]`: composite state keys/values and parameter structs, in place of hand-packed byte buffers |
| 13 | [`keylets`](examples/13_keylets) | typed `keylet_xxx` helpers, one per `KEYLET_*` constant, in place of the single untyped `util_keylet` |
| 14 | [`account-id-macro`](examples/14_account-id-macro) | `rshooks::account_id!`: compile-time r-address → `AccountId` decode |
| 15 | [`slot-objects`](examples/15_slot-objects) | the typed slot layer's live acceptance harness: account-root walk, native-amount round-trip, parent-clear/child-read |
| 16 | [`typed-results`](examples/16_typed-results) | typed entry returns (`HookResult`): an idiomatic `?`/`Ok` entry with a `hook_errors!` message clause, alongside a raw `accept!`/`rollback!`-style entry in the same chain |
| 17 | [`sto-writer`](examples/17_sto-writer) | `rshooks::sto_writer::StoWriter`: a runtime-shaped Remit — a native `sfAmounts` entry always, an issued one when hook parameters supply it — built field-by-field and emitted via `prepare_for_emit()`/`Prepared::emit()` |
| 18 | [`typed-views`](examples/18_typed-views) | `rshooks::views`: generated, type-checked read views — an incoming-IOU gate reading `tx::Payment`, then `ledger::RippleState`'s freeze flags and `ledger::AccountRoot`'s optional `sfTransferRate`, with a per-read cost table |
| 80 | [`governance`](examples/80_governance) | a two-entry `#[hooks]` **chain** (`govern` + `reward`) porting xahaud's genesis governance hooks, sharing one state schema |

```sh
mise run build-examples   # builds every example through rshooks-build and checks the output
```

Every example declares its Hook(s) with `#[hooks]`: a struct holding the
shared `State`/`HookParam`/`OtxnParam` schema, and an inherent `impl` block
declaring one or more `#[hook(<index>, ..)]` entries (each with an optional
paired `#[cbak(<index>)]`) as plain, safe functions:

```rust
#![no_std]

use rshooks::exit::{Accept, HookResult};
use rshooks::*;

#[hooks(description = "Accepts every transaction selected by HookOn.")]
pub struct AcceptAll;

#[hooks]
impl AcceptAll {
    #[hook(0, name = "accept", on = [Invoke])]
    fn main(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}
```

Because index is just another entry in the same `impl` block, a single
crate can declare **more than one** Hook — a chain — sharing that one
schema between them (see `80_governance`, above).

`cargo run -p rshooks-build -- build --manifest-path <crate>/Cargo.toml
--out <crate>/out` writes, under `<crate>/out/current/`, one
`<index>.<fn>.wasm` and one `<index>.<fn>.metadata.json` sidecar per
declared entry, plus a `sethook.template.json` — a ready-to-edit `SetHook`
transaction covering every index the crate declares. Each sidecar's
top-level fields are deployable raw SetHook values (transaction masks, hex
`HookName`); the readable form of the same declarations is under `human`.
Sidecars also carry the final binary's `HookHash`, static worst-case
instruction count (`WCE`), and a `builder` block recording the toolchain
that produced them, for deterministic reproduction later — all carried
only through unreachable raw-wasm exports the cleaner strips, so none of
it changes the final wasm's bytes, hash, or instruction count.

See [`examples/README.md`](examples/README.md) for details, including the
compiler-generated-loop pitfall that used to require `--auto-guard` (none
of these examples need it any more).

## E2E tests

`e2e/` deploys the examples' `rshooks-build` output to a real,
standalone `xahaud` (via `SetHook`) and asserts on the resulting
transaction metadata and ledger state — proof of runtime behavior, not
just that the binaries are SetHook-valid. See
[`docs/E2E-TESTING.md`](docs/E2E-TESTING.md) for the design.

```sh
mise run e2e:node-up     # starts a standalone Xahau node (xrpld-lab; needs Docker)
mise run e2e              # builds the examples, then runs the e2e suite against it
mise run e2e:node-down   # stops it
```

`e2e/` is an isolated pnpm package (not part of any Cargo or pnpm
workspace) using the same stack as this machine's other hook repos:
vitest + `@transia/hooks-toolkit` + `xahau`.
