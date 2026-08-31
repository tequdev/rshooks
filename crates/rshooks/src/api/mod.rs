//! Ergonomic wrappers over every `rshooks-core` Hook API function (except
//! `_g`, exposed only via the `guard!`/`guard_m!` macros in `macros.rs`),
//! organized into one module per Hook API category — mirrors the grouping
//! in `hook/extern.h` and DESIGN.md §5.
//!
//! 60 of the 74 non-`_g` functions get a public wrapper here; the other 14
//! (`float_set`, `float_multiply`, `float_mulratio`, `float_negate`,
//! `float_compare`, `float_sum`, `float_invert`, `float_divide`, `float_one`,
//! `float_mantissa`, `float_sign`, `float_int`, `float_log`, `float_root`)
//! are wrapped privately as [`crate::xfl::XFL`] methods instead — see
//! `xfl.rs`.
//!
//! [`keylet`] is the one exception to "one module per Hook API function": it
//! wraps a single underlying function, [`util::util_keylet`] (one host call
//! handling all 26 `KEYLET_*` types via six untyped `u32` components), as 26
//! separate, precisely-typed functions — one per [`rshooks_core::consts`]
//! `KEYLET_*` constant — so each keylet type's own argument shape (which
//! components are pointers, which are plain integers, how many are used) is
//! encoded in its signature instead of six same-typed slots.

pub mod control;
pub mod etxn;
pub mod float;
pub mod hook_ctx;
pub mod keylet;
pub mod ledger;
pub mod otxn;
pub mod slot;
pub mod state;
pub mod sto;
pub mod trace;
pub mod util;

pub use control::*;
pub use etxn::*;
pub use float::*;
pub use hook_ctx::*;
pub use keylet::*;
pub use ledger::*;
pub use otxn::*;
pub use slot::*;
pub use state::*;
pub use sto::*;
pub use trace::*;
pub use util::*;
