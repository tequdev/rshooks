//! Field-code parameters take `impl Into<u32>`, which has to accept all three
//! things a caller can hold: a typed `SField`, a raw `sfcodes` constant, and
//! a `u32` computed or stored earlier.
//!
//! Also pins the two const bridges — `SField::code()` and the raw table — and
//! that the numbered slot functions plus the raw `sfcodes` glob are still
//! reachable by explicit path after leaving the prelude.

use rshooks::prelude::*;

// Explicit paths: both of these left the prelude, neither left the crate.
use rshooks::api::otxn::otxn_slot;
use rshooks::api::slot::{meta_slot, slot_clear, slot_set, slot_subarray, slot_subfield};

// `SField::code()` in a const context — `Into` is not const, this is.
const SEQUENCE: u32 = sfSequence.code();

fn main() {
    assert_eq!(SEQUENCE, rshooks::raw::sfcodes::sfSequence);

    let mut buf = [0u8; 20];

    // A typed constant.
    let _ = otxn_field(&mut buf, sfAccount);
    // A raw constant.
    let _ = otxn_field(&mut buf, rshooks::raw::sfcodes::sfAccount);
    // A `u32` held from earlier — the stored-code compatibility case.
    let stored: u32 = SEQUENCE;
    let _ = otxn_field(&mut buf, stored);
    // A suffixed literal.
    let _ = otxn_field(&mut buf, 0u32);

    // The rest of the widened inventory.
    let _ = otxn_field_u64(sfSequence);
    let _ = otxn_field_exact::<[u8; 20]>(sfAccount);
    let sto = [0u8; 8];
    let _ = sto_subfield(&sto, sfAccount);
    let _ = sto_subfield_range(&sto, sfAccount);
    let _ = sto_subfield_slice(&sto, sfAccount);
    let mut out = [0u8; 64];
    let _ = sto_emplace(&mut out, &sto, &sto, sfAccount);
    let _ = sto_erase(&mut out, &sto, sfAccount);
    let _ = float_sto(&mut out, None, None, XFL::one(), sfAmount);
    let _ = XFL::one().sto(&mut out, None, None, sfAmount);
    let _ = slot_subfield(1, sfAccount, 0);

    // Every *non-slot* `api::otxn` export stays in the prelude: only
    // `otxn_slot` left with the numbered slot family. Pinned by name,
    // because the prelude now lists them individually.
    let _ = otxn_burden();
    let _ = otxn_generation();
    let _ = otxn_type();
    let _ = otxn_id(0);
    let mut idbuf = [0u8; 32];
    let _ = otxn_id_into(&mut idbuf, 0);

    // Numbered slot functions, by explicit path.
    let _ = slot_set(&[0u8; 34], 1);
    let _ = slot_clear(1);
    let _ = slot_subarray(1, 0, 0);
    let _ = otxn_slot(0);
    let _ = meta_slot(0);
}
