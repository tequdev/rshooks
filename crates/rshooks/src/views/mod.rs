//! Typed read views over every transaction, ledger entry and inner object
//! xahaud declares a format for.
//!
//! ```no_run
//! use rshooks::prelude::*;
//! use rshooks::views::tx::Payment;
//!
//! # fn f() -> Result<()> {
//! // Fails with `HookError::DoesNotMatch` if the originating transaction
//! // is not a Payment, so the reads below cannot be about something else.
//! let p = Payment::otxn()?;
//! let dest: AccountId = p.destination()?;      // soeREQUIRED  -> Result<T>
//! let tag: Option<u32> = p.destination_tag()?; // soeOPTIONAL  -> Result<Option<T>>
//! # let _ = (dest, tag);
//! # Ok(())
//! # }
//! ```
//!
//! # Where the shapes come from
//!
//! [`tx`], [`ledger`] and [`inner`] are **generated** by `cargo xtask
//! gen-core` from `crates/rshooks-core/protocol_formats.json`, which is
//! itself parsed from xahaud's own `transactions.macro`,
//! `ledger_entries.macro` and `InnerObjectFormats.cpp`. Every struct, every
//! accessor, every required/optional distinction is upstream's declaration,
//! not this crate's opinion; an amendment that adds a field is picked up by
//! `scripts/sync-vendor.sh` + `cargo xtask gen-core`, and `gen-core --check`
//! fails CI if the checked-in output has drifted.
//!
//! That is what makes these shapes acceptable where hand-written ones are
//! not — see `crate::txn`'s "Why there is no library-owned
//! `PaymentTemplate` here", which this module amends rather than overturns.
//!
//! # Which views exist
//!
//! Upstream's format tables are inherited wholesale from rippled, so they
//! declare a great deal Xahau cannot run. Generating all of it would offer
//! you an `AMMBid` or an `XChainCommit` view that no real transaction can
//! ever match, so `crates/rshooks-core/format_availability.json` classifies
//! every format and the generator follows it:
//!
//! - **active** — activated on Xahau mainnet. Generated normally. This is
//!   what you get by default.
//! - **pending** — supported by xahaud but not yet activated as of the
//!   vendored snapshot. Generated behind the `pending-amendments` cargo
//!   feature, so you can write and test against the shape in advance
//!   without it cluttering everyone else's autocomplete. Enable it with
//!   `rshooks = { …, features = ["pending-amendments"] }`.
//! - **dormant** — gated by an amendment xahaud marks `Supported::no`, so
//!   it cannot activate on Xahau without a node upgrade. **Not generated at
//!   all**, and no feature brings it back.
//!
//! The `sfield` constants a view reads follow the same tiers, so a pending
//! view and its pending-only fields compile together or not at all. The raw
//! layers are untouched: `rshooks::raw::sfcodes` stays a complete mirror,
//! and [`crate::tx_type::TxType`]/[`crate::ledger_entry_type::LedgerEntryType`]
//! stay exhaustive, because decoding a wire value is a different job from
//! offering an API.
//!
//! [`source`] is hand-written: it holds the whole of the views' logic, so
//! the generated files contain declarations and nothing else.
//!
//! # When to use a view, and when not to
//!
//! A view is worth it when you are reading **named fields of a known
//! type**: it names the fields for you, gets their value types right, and
//! checks on construction that the object really is what you think.
//!
//! Reach past it for anything else. [`crate::api::otxn`] and
//! [`crate::slot_obj`] are unchanged and public: a hook that reads one
//! field, walks an array, or works with an object whose type it does not
//! know in advance is better served by those directly.
//!
//! # Cost
//!
//! A view is not a layer over the Hook API, it is a spelling of it. The
//! source types are monomorphized and every accessor is
//! `#[inline(always)]`, so a `Payment<OtxnSource>` accessor compiles to the
//! same single `otxn_field` call a hand-written hook would make, and the
//! view value itself is zero-sized.
//!
//! The one place a view spends something a hand-written hook need not is
//! slot-backed reads: each one clears the child slot it opened, which is a
//! `slot_clear` host call the C idiom skips. That is the price of an
//! accessor that can be called any number of times without exhausting the
//! 255-slot budget — [`source`]'s module docs make the argument in full.
//!
//! # What is not here (yet)
//!
//! - **No array iteration sugar.** An `STArray` field gives you raw bytes
//!   (`…_into`) or a `SlotObject<STArray>` handle (`…_slot`); iterate it
//!   with the slot API and wrap each element in a [`inner`] view.
//! - **No builders.** These are read views. Emitting a transaction is
//!   [`crate::txn`]'s and [`crate::sto_writer`]'s job.

pub mod source;

// The three generated modules carry their own `//!` docs. Deliberately no
// `///` doc here as well: rustdoc merges an outer doc comment with the
// module's own but then resolves the whole merged text's intra-doc links in
// *this* module's scope, where `Payment` and `RippleState` do not exist.
pub mod inner;
pub mod ledger;
pub mod tx;
