//! Off-chain unit-test harness for `rshooks` Xahau Hooks.
//!
//! [`TestEnv`] runs a `#[hooks]` chain entry as plain native Rust — no wasm
//! build, no node, no Docker — by installing a mock [`rshooks_core::backend::HostBackend`]
//! for the duration of one [`TestEnv::invoke`] call. It answers the Hook API
//! surface listed in `.claude/design/TESTENV_DESIGN.md` §6 ("Phase 1")
//! against an in-memory world model (state, the originating transaction,
//! hook parameters, ledger fields, grants, and emitted transactions), and
//! captures `accept!`/`rollback!` exits as ordinary Rust values instead of
//! process exits.
//!
//! # Positioning
//!
//! `cargo test` against this crate answers "is my hook's logic right" in
//! milliseconds. It is **not** a fidelity oracle: fee/reserve economics,
//! instruction counting, guard enforcement, ledger objects/keylets/slots,
//! float ops, signature verification, and multi-hook chain auto-execution
//! (`HookOn` trigger filtering included — every `invoke` is a direct,
//! explicit entry call) are all out of scope here and remain e2e-only
//! territory. See `.claude/design/TESTENV_DESIGN.md` §5/§6 for the complete,
//! normative list of what is and is not modeled.
//!
//! # Getting started
//!
//! ```no_run
//! use rshooks_testenv::prelude::*;
//!
//! # fn main() {
//! let env = TestEnv::new();
//! // env.invoke::<MyChain>(0);
//! # }
//! ```
//!
//! # Escape hatches and limitations, documented up front
//!
//! - **`rshooks::raw` (direct `rshooks_core` calls) bypasses this harness.**
//!   The mock backend only intercepts the `rshooks` wrapper layer; a hook
//!   that calls `rshooks_core` directly keeps hitting the real
//!   `NOT_IMPLEMENTED` host stubs under `testenv`, exactly as it would on any
//!   other native build. This is a deliberate, documented WCE-escape-hatch
//!   limitation, not a bug.
//! - **Statics outside [`rshooks::static_cell::HookStatic`] are not reset**
//!   between invocations — an ordinary `static` a hook declares by hand
//!   keeps its value across every `TestEnv::invoke` call on the same
//!   process, unlike the fresh-wasm-instance-per-invocation reality.
//!   `HookStatic` is the sanctioned pattern this harness resets per
//!   invocation (take-once-per-invocation); see [`prelude::HookChainEntries`]'s
//!   crate for the take-set mechanism.
//! - **No `HookOn` filtering.** `invoke::<C>(index)` is a direct entry call:
//!   it runs the declared entry unconditionally, even if the seeded
//!   [`Otxn`]'s type would not have triggered it on-chain. [`TestEnv::strict_can_emit`]
//!   is the one place declarations are still checked (against
//!   `NativeEntry::can_emit`).
//! - **Fees are an explicit approximation** (`base_fee × etxn_burden`, base
//!   fee 10 drops by default) — real fee calculation parses the transaction
//!   blob and runs the ledger's fee calculator; that stays e2e-only.
//! - **A bare `return code` is provisional.** No live-node evidence pins its
//!   real on-chain commit semantics yet (design §4); this harness maps it to
//!   [`ExitType::Return`] with the invocation's state snapshot **restored**
//!   (the conservative choice) and `is_success() == false`.

// `accept!`/`rollback!` reach this crate's backend via
// `panic::panic_any(HookExitSignal(..))`, caught by `TestEnv::invoke`'s
// `catch_unwind` — see `.claude/design/TESTENV_DESIGN.md` §2.2. A test
// profile compiled with `panic = "abort"` would abort the whole test
// process at the very first `accept!`/`rollback!` instead of unwinding, so
// this compile-time guard turns that into an actionable error instead of a
// silent process abort.
#[cfg(panic = "abort")]
compile_error!(
    "rshooks-testenv requires `panic = \"unwind\"` (the default Rust test \
     profile): accept!/rollback! are captured by unwinding across the mock \
     host boundary. If your workspace sets `panic = \"abort\"` for the test \
     profile, override it for tests, e.g.:\n\n\
     [profile.test]\n\
     panic = \"unwind\"\n"
);

mod backend;
mod details;
mod emit_walk;
mod env;
mod exit;
mod grant;
mod invocation;
mod otxn;
mod world;

pub use env::TestEnv;
pub use exit::{ExitType, HookExit};
pub use grant::Grant;
pub use otxn::Otxn;
pub use world::{EmitAttempt, EmitFailureReason, EmittedTxn, TraceLine};

/// Common imports for hook tests: `use rshooks_testenv::prelude::*;` pulls
/// in [`TestEnv`], [`HookExit`]/[`ExitType`], [`Otxn`], [`Grant`],
/// [`EmittedTxn`]/[`EmitAttempt`]/[`TraceLine`], and
/// [`rshooks::decl::HookChainEntries`] (the trait `invoke`'s `C` type
/// parameter is bound by — implemented automatically by every `#[hooks]`
/// impl block on a non-wasm target).
pub mod prelude {
    pub use crate::{
        EmitAttempt, EmitFailureReason, EmittedTxn, ExitType, Grant, HookExit, Otxn, TestEnv,
        TraceLine,
    };
    pub use rshooks::decl::HookChainEntries;
    pub use rshooks::tx_type::TxType;
}
