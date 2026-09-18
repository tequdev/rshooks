# The Raw Layer

`rshooks::raw` is a direct re-export of the `rshooks-core` crate — a plain
alias, not a re-exporting wrapper module, so every path under
`rshooks::raw::*` is identical to the same path under `rshooks_core::*`.
This is the lowest layer of the toolchain: `no_std`, zero-logic, 1:1
translations of the xahaud `hook/` C headers into Rust, with no `Result`
type, no typed wrappers, and no ergonomics of any kind.

Everything else in `rshooks` — `api`, `state`, `slot_obj`, the macros — is
built on top of this layer. Most hook code never needs to reach into it
directly; see [The Prelude](prelude.md) for what the ergonomic surface
covers instead.

## What's in it

| module | contents |
|---|---|
| `raw::api` (via `raw::*`) | The 75 raw Hook API function declarations (`_g` plus 74 functions), `unsafe extern "C"`, imported from wasm import module `env` — one line per `extern.h` function, parameter names and types kept verbatim (`read_ptr: u32`, `i64` returns) so C hook source and this file can be compared line by line. |
| `raw::host` | `HookHost`: the same 75 functions again, as trait methods with identical names/signatures — an indirection point for a future native test host. Not used by `rshooks` or the examples today; the flat free-function API above remains the public surface they call. |
| `raw::sfcodes` | Every `sfXxx` serialized-field code (325 fields), each a `u32` packing `(type << 16) + index`, mirrored verbatim from `sfcodes.h`. |
| `raw::tts` | Every `ttXxx` transaction-type code (`ttPAYMENT`, `ttHOOK_SET`, ...), from `tts.h`. |
| `raw::lets` | Every `ltXxx` ledger-entry-type code (`ltACCOUNT_ROOT`, `ltHOOK`, ...) — what `rshooks::ledger_entry_type::LedgerEntryType` decodes and `sfLedgerEntryType` carries — from the vendored `ledger_entries.macro`. |
| `raw::consts` | `KEYLET_*` and `COMPARE_*` constants from `hookapi.h`, plus `tfCANONICAL`, the `atACCOUNT` family, and the `amAMOUNT` family from `macro.h`. |
| `raw::ls_flags` | Every `lsfXxx` ledger-entry flag from `ls_flags.h`, flattened from the header's per-ledger-entry-type C enums into one list (no name collides across enums). |
| `raw::tx_flags` | Every `tfXxx` transaction flag and `asfXxx` account flag from `tx_flags.h`, flattened the same way. A few `MPTokenIssuanceCreateFlags` members alias `ls_flags` values in the C header and are kept as references to the `ls_flags` const rather than re-typed literals, so the two stay in sync by construction. |
| `raw::error` | Every Hook API error code (`SUCCESS = 0`, `OUT_OF_BOUNDS = -1`, ..., `NOT_IMPLEMENTED = -14`, `INVALID_FLOAT = -10024`), kept verbatim from `error.h`. |
| `raw::backend` | `#[doc(hidden)]`, native-only: the `HostBackend` trait `rshooks-testenv`'s mock host implements, plus `install()` to swap one in for the duration of a scope. An unstable internal contract between `rshooks`/`rshooks-core`/`rshooks-testenv`, not a stable public API — but reachable, and occasionally useful directly in a test that needs to stub one specific host call (`float_sto`, say) without pulling in the whole `TestEnv` model; see `rshooks::sto_writer`'s own `testenv_tests` module or `examples/17_sto-writer`'s in-crate tests for the pattern. |

Every constant module is `@generated` from the vendored xahaud headers under
`crates/rshooks-core/vendor/xahaud-hook/` — not hand-maintained — so a name
or value here always matches the upstream C header it was generated from.

## When a hook author actually needs it

Two situations pull a hook out of the typed `rshooks::api`/`prelude` surface
and down into `raw`:

- **Const contexts needing a raw `u32` sfield code.** The typed `sfield`
  constants (`crate::sfield::sfSequence`, etc., what the prelude re-exports
  under the plain `sfXxx` names) are `SField<T>` values, not bare `u32`s —
  fine for ordinary calls, but unusable where a raw integer is required at
  compile time. `txn_template!`'s field tables are the main example: `sfcode`
  values there come from `rshooks::raw::sfcodes::*` (or `SField::code()`) so
  they can participate in `const fn` offset arithmetic. See [Emitting
  Transactions](../emit/emitting.md) and [Macro Reference](macros.md).
- **An API the wrapper doesn't cover.** `rshooks::api` wraps the common Hook
  API surface with `Result`-returning, panic-free functions, but if a
  specific raw host call has no typed wrapper yet, `rshooks::raw`'s
  `unsafe extern "C"` declarations are still there to call directly.

## Host builds: every raw call is a stub

On a non-`wasm32` target (an ordinary `cargo check`/`cargo test`, what
rust-analyzer runs for completion and diagnostics), the wasm import block
doesn't exist — there is no host to link against. `rshooks-core` instead
provides the same signatures as deterministic stub functions that return
`NOT_IMPLEMENTED` (`raw::error::NOT_IMPLEMENTED`, `-14`); the `_g` stub is
the one exception, returning `0` ("guard check passed"), so guarded loops
still run under host tests. **None of the stubs panic.**

This is what makes `cargo check`/`cargo test` work at all for a `no_std`
hook crate outside the wasm host, and it's also why every doctest and unit
test in this book that calls a Hook API function asserts
`Err(HookError::NotImplemented)` rather than a real value — the typed
wrappers in `rshooks::api` surface the raw stub's `NOT_IMPLEMENTED` as
`HookError::NotImplemented`, so the same assertion works whether the call
went through `rshooks::api` or `rshooks::raw` directly.

## `unsafe`, and bypassing the wrapper

Every function in `rshooks::raw::api` (and every `HookHost` method) is
`unsafe extern "C"` — these are bare FFI declarations with no argument
validation, no length checking, and no `Result` conversion. Calling into
`raw` directly means taking on everything `rshooks::api`'s typed wrappers
normally handle: buffer sizing, error-code-to-`HookError` translation, and
(for the slot API in particular) handle bookkeeping that
`slot_obj::SlotObject` otherwise manages for you. Prefer the typed surface
in [The Prelude](prelude.md) unless a hook has a specific, verified reason
to drop down here.
