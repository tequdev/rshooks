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
//! These modules are generated from xahaud's protocol format declarations;
//! `cargo xtask gen-core --check` verifies that checked-in views are current.
//!
//! Fields shared by every format live once, on common-field traits
//! ([`tx::TransactionCommonFields`], [`tx::TransactionCommonSlotFields`],
//! [`ledger::LedgerEntryCommonFields`]) that every view implements. The
//! prelude re-exports them, so `use rshooks::prelude::*` is enough to call
//! the common accessors.
//!
//! # Which views exist
//!
//! Upstream's format tables include formats unavailable on Xahau mainnet.
//! `crates/rshooks-core/format_availability.json` classifies each format and
//! the generator applies these tiers:
//!
//! - **active** — activated on Xahau mainnet.
//! - **pending** — supported by xahaud but not yet activated.
//! - **dormant** — not expected on Xahau mainnet; custom networks may enable it.
//!
//! The default includes active and pending formats. `active-amendments`
//! narrows the surface to what is live; `all-amendments` exposes dormant
//! formats for custom networks. If both are enabled, the wider surface wins.
//!
//! The `sfield` constants a view reads follow the same tiers, so a pending
//! view and its pending-only fields compile together or not at all. The raw
//! layers stay untouched and exhaustive regardless: `rshooks::raw::sfcodes`,
//! [`crate::tx_type::TxType`], [`crate::ledger_entry_type::LedgerEntryType`].
//!
//! [`source`] is hand-written and holds the whole of the views' logic; the
//! generated files contain declarations and nothing else.
//!
//! Views suit named fields of a known object type. Use [`crate::api::otxn`]
//! or [`crate::slot_obj`] directly for dynamic objects and array traversal.
//!
//! # Cost
//!
//! Source types are monomorphized and accessors are `#[inline(always)]`;
//! originating-transaction views add no bookkeeping.
//!
//! Slot-backed reads additionally clear each child slot, allowing repeated
//! accessor calls without exhausting the 255-slot budget.
//!
//! # What is not here (yet)
//!
//! - **No array iteration sugar.** An `STArray` field gives you raw bytes
//!   (`…_into`) or a `SlotObject<STArray>` handle (`…_slot`); iterate it
//!   with the slot API and wrap each element in a [`inner`] view.
//! - **No builders.** These are read views. Emitting a transaction is
//!   [`crate::txn`]'s and [`crate::sto_writer`]'s job.

pub mod source;

// Module docs live in the generated files so their links resolve there.
pub mod inner;
pub mod ledger;
pub mod tx;
