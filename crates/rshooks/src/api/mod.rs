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
//!
//! # `_into` twins
//!
//! Every by-value fixed-size read in this module (`hook_account_buf`,
//! `hook_hash_buf`, `otxn_id_buf`, `ledger_last_hash_buf`, `etxn_nonce_buf`,
//! `util_accid_buf`, `util_sha512h_buf`, `otxn_field_exact`,
//! `otxn_field_typed`) has an `_into(out: &mut T, ...) -> Result<()>` twin
//! that writes straight into caller-owned `out` instead of returning `T` by
//! value. Reach for the twin when the result is about to be borrowed into
//! another call right away: the by-value form's own local has its address
//! taken by the host call, which stops the optimizer from eliding the copy
//! into the caller's actual destination on return. [`keylet`]'s own
//! "`_into` twins" section covers the keylet family's version of the same
//! split.

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

/// Emits a `xxx_buf`/`xxx_into` pair around a caller-buffer host wrapper
/// `raw(out: &mut B, ...) -> Result<usize>` — the by-value/`_into` split
/// `keylet.rs`'s `keylet_fn!` does for the keylet family, applied here to a
/// concrete, non-generic `raw`/return-type pair instead of a whole
/// `KEYLET_*` table (see this module's "`_into` twins" doc section for when
/// to reach for the twin). The `_into` twin writes straight into
/// caller-owned `out`; the by-value form is just that twin plus a local,
/// since `raw` always writes the whole fixed-size `out` on success (the `?`
/// above it already propagated any error) — the local's own initial value
/// is either fully overwritten or, on error, discarded by `?` before ever
/// being returned.
macro_rules! fixed_buf_fn {
    (
        $(#[$doc:meta])*
        fn $name:ident($($arg:ident : $aty:ty),*) -> $ret:ty = $raw:path,
        $into_name:ident $(,)?
    ) => {
        $(#[$doc])*
        #[inline(always)]
        pub fn $name($($arg: $aty),*) -> crate::error::Result<$ret> {
            let mut buf = <$ret>::default();
            $into_name(&mut buf, $($arg),*)?;
            Ok(buf)
        }

        #[doc = concat!(
            "Out-param twin of [`", stringify!($name), "`] — see the ",
            "`api` module doc comment's \"`_into` twins\" section for when ",
            "to reach for this over the by-value form."
        )]
        #[inline(always)]
        pub fn $into_name(out: &mut $ret, $($arg: $aty),*) -> crate::error::Result<()> {
            let _ = $raw(out.as_mut(), $($arg),*)?;
            Ok(())
        }
    };
}
pub(crate) use fixed_buf_fn;

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
