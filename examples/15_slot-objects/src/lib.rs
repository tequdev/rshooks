//! Exercises typed slot-object reads, navigation, casts, and cleanup.
//!
//! Each invocation runs the check group selected by its `CHK` parameter and
//! returns the passing checks as bits in the accept code.

#![no_std]

use rshooks::prelude::*;
use rshooks::slot_path;
use rshooks::*;

hook_errors! {
    /// `slot-objects` rollback codes.
    pub enum SlotObjectsError {
        /// The originating transaction has no `sfAccount`.
        NoSender = 1,
        /// Building the account keylet failed.
        KeyletFailed = 2,
        /// Loading the sender's account root into a slot failed.
        AccountRootFailed = 3,
    }
}

/// Iterations used to exercise slot recycling beyond the slot limit.
const LOOP_ITERATIONS: u32 = 260;

/// Bit 0: the account-root walk read all three fields.
const BIT_ACCOUNT_WALK: i64 = 1;
/// Bit 1: `as_xfl(balance)` round-tripped to raw drops.
const BIT_DROPS_ROUNDTRIP: i64 = 2;
/// Bit 2: a child slot stayed readable after its parent was cleared.
const BIT_PARENT_CLEAR: i64 = 4;
/// Bit 3: every `take_value()` in the success loop released its slot.
const BIT_TAKE_LOOP: i64 = 8;
/// Bit 4: failed `slot_path!` walks recycled intermediate slots.
const BIT_MIDHOP_LOOP: i64 = 16;
/// Bit 5: successful deep walks recycled slots.
const BIT_DEEP_LOOP: i64 = 32;
/// Bit 6: failed `take_value()` reads recycled slots.
const BIT_TAKE_FAILURE: i64 = 64;
/// Bit 7: failed `try_cast`s recycled slots.
const BIT_CAST_CLEANUP: i64 = 128;
/// Bit 8: a root slot's high object code is accepted by `try_cast::<STObject>`.
const BIT_ROOT_CAST: i64 = 256;
/// Bit 9: a `u64` field reads identically through `value()` and raw bytes.
const BIT_U64_WIRE: i64 = 512;
/// Bit 10: an IOU trust-line balance round-trips through `as_xfl()`.
const BIT_IOU_XFL: i64 = 1024;

/// Checks account-root fields.
#[inline(never)]
fn check_account_walk(account: &SlotObject<STObject>, sender: &AccountId) -> bool {
    let Ok(seq): Result<u32> = account.get(sfSequence).and_then(|s| s.value()) else {
        return false;
    };
    let Ok(acc) = account.get(sfAccount).and_then(|s| s.value()) else {
        return false;
    };
    buf_eq_20(&acc, sender) && seq > 0
}

/// Reads `sfBalance` two ways, both checked in this one
/// `#[inline(never)]` frame since the hook's guard nesting budget can't
/// spare an extra frame per check. Returns the bits earned.
///
/// 1. `as_xfl()` on a native amount yields **XAH units**, so
///    `to_int(6, false)` must recover the drop count the raw wire bytes
///    carry.
/// 2. The same eight bytes read as a `u64` must agree between `value()` —
///    which decodes the wire bytes big-endian — and a raw read. The native
///    amount encoding sets bit 63 ("not an IOU"), which is exactly the case
///    the host rejects in as-int64 mode and the reason `u64` does not use
///    it.
#[inline(never)]
fn check_balance_reads(account: &SlotObject<STObject>) -> i64 {
    let Ok(xfl) = account.get(sfBalance).and_then(|s| s.as_xfl()) else {
        return 0;
    };
    let Ok(buf) = account.get(sfBalance).and_then(|s| s.raw_exact::<8>()) else {
        return 0;
    };
    let raw = u64::from_be_bytes(buf);
    let Ok(scaled) = xfl.to_int(6, false) else {
        return 0;
    };

    let mut bits: i64 = 0;
    // The wire encoding sets the top "not an IOU" bit; the low 62 bits are
    // the drop count.
    if (scaled as u64) == (raw & 0x3FFF_FFFF_FFFF_FFFF) {
        bits |= BIT_DROPS_ROUNDTRIP;
    }
    // `value()` on a `u64` handle must reproduce those same bytes; bit 62
    // (asserted below) rules out an all-zero read passing by accident.
    let typed = account
        .get(sfBalance)
        .map(|s| s.assume_type::<u64>())
        .and_then(|s| s.value());
    if typed == Ok(raw) && (raw >> 62) & 1 == 1 {
        bits |= BIT_U64_WIRE;
    }
    bits
}

/// Derives a child, clears its parent, and only then reads the child — the
/// assumption `slot_path!` is built on.
#[inline(never)]
fn check_parent_clear(keylet: &Keylet) -> bool {
    let Ok(parent) = SlotObject::from_keylet(keylet) else {
        return false;
    };
    let Ok(child) = parent.get(sfSequence) else {
        return false;
    };
    let _ = parent.clear();
    child.value().map(|v: u32| v > 0).unwrap_or(false)
}

/// 260 `slot_path!` walks over the sender's SignerList whose **third** hop
/// fails, after the first two have succeeded.
///
/// Hops 1 and 2 — `sfSignerEntries`, then element 0 — allocate two *owned*
/// intermediates before hop 3 asks a SignerEntry for a `sfBalance` it does
/// not have. The ladder clears each intermediate unconditionally, before
/// inspecting the result, so 260 failures must leak nothing; with a leak
/// the 255-slot budget runs out partway through and the tail fails for a
/// different reason.
///
/// The root is loaded **once** and borrowed by every walk — `slot_path!`
/// never clears its root.
#[inline(never)]
fn check_midhop_loop(signers: &Keylet) -> bool {
    let Ok(root) = SlotObject::from_keylet(signers) else {
        return false;
    };
    let mut failures: u32 = 0;
    let mut i: u32 = 0;
    while i < LOOP_ITERATIONS {
        guard!(LOOP_ITERATIONS);
        i = i.wrapping_add(1);
        if slot_path!(root[sfSignerEntries][0u32][sfBalance]).is_err() {
            failures = failures.wrapping_add(1);
        }
    }
    let _ = root.clear();
    failures == LOOP_ITERATIONS
}

/// 260 *successful* three-hop walks, each reading its leaf with
/// `take_value()`.
///
/// This one loop proves both success-path contracts at once: each
/// iteration derives three slots — two intermediates the ladder clears,
/// plus a leaf `take_value()` releases — so 260 iterations move 780 slots
/// through a 255-slot budget. A failure to recycle in *either* mechanism
/// exhausts it well before the end.
#[inline(never)]
fn check_deep_loop(signers: &Keylet) -> bool {
    let Ok(root) = SlotObject::from_keylet(signers) else {
        return false;
    };
    let mut ok: u32 = 0;
    let mut i: u32 = 0;
    while i < LOOP_ITERATIONS {
        guard!(LOOP_ITERATIONS);
        i = i.wrapping_add(1);
        if let Ok(leaf) = slot_path!(root[sfSignerEntries][0u32][sfAccount]) {
            if leaf.take_value().map(|_: AccountId| true).unwrap_or(false) {
                ok = ok.wrapping_add(1);
            }
        }
    }
    let _ = root.clear();
    ok == LOOP_ITERATIONS
}

/// 260 `take_value()` reads that *fail*, proving `take_*` clears on the
/// failure path as well as the success one.
///
/// `sfSequence` on an account root is a `u32`; reading it as a `Hash` asks
/// for 32 bytes from a 4-byte slot, so every read errors. Only the clear
/// keeps this inside the budget.
#[inline(never)]
fn check_take_failure_loop(keylet: &Keylet) -> bool {
    let Ok(root) = SlotObject::from_keylet(keylet) else {
        return false;
    };
    let mut failures: u32 = 0;
    let mut i: u32 = 0;
    while i < LOOP_ITERATIONS {
        guard!(LOOP_ITERATIONS);
        i = i.wrapping_add(1);
        let derived = root.get(sfSequence).map(|s| s.assume_type::<Hash>());
        if derived.and_then(|s| s.take_value()).is_err() {
            failures = failures.wrapping_add(1);
        }
    }
    let _ = root.clear();
    failures == LOOP_ITERATIONS
}

/// 260 failed `try_cast`s: any failure consumes the handle and best-effort
/// clears the slot, so repeating past the budget must keep failing the same
/// way rather than running out of slots.
#[inline(never)]
fn check_cast_cleanup_loop(keylet: &Keylet) -> bool {
    let Ok(root) = SlotObject::from_keylet(keylet) else {
        return false;
    };
    let mut failures: u32 = 0;
    let mut i: u32 = 0;
    while i < LOOP_ITERATIONS {
        guard!(LOOP_ITERATIONS);
        i = i.wrapping_add(1);
        // `sfSequence` is a UInt32 (type ID 2); casting it to STArray (15)
        // cannot hold.
        if root
            .get(sfSequence)
            .and_then(|s| s.try_cast::<STArray>())
            .is_err()
        {
            failures = failures.wrapping_add(1);
        }
    }
    let _ = root.clear();
    failures == LOOP_ITERATIONS
}

/// The IOU half of the amount path: reads the sender's trust-line balance,
/// which is a **non-native** amount, and checks both that `is_native()` says
/// so and that `as_xfl()` recovers the value the e2e paid in.
///
/// The issuer arrives as an `ISS` parameter on the originating transaction —
/// the e2e generates its wallets at runtime, so the address cannot be baked
/// in. The currency is `USD` in XRPL's standard 20-byte encoding: twelve
/// zero bytes, the three ASCII characters, then five more zeros.
///
/// The comparison uses `to_int(0, true)` — **absolute** value. A
/// `RippleState` object's balance is signed relative to whichever of the two
/// accounts sorts low, which is an ordering this hook has no business
/// depending on; the magnitude is the fact being checked.
#[inline(never)]
fn check_iou_amount(holder: &AccountId) -> bool {
    /// `USD` in the standard 20-byte currency encoding.
    const USD: CurrencyCode = CurrencyCode::from_iso(b"USD");

    let Ok(Some(issuer)) = SlotObjects.otxn_param.iss.get() else {
        return false;
    };
    let Ok(keylet) = keylet_line(holder, &issuer, &USD) else {
        return false;
    };
    let Ok(line) = SlotObject::from_keylet(&keylet) else {
        return false;
    };
    let Ok(balance) = line.get(sfBalance) else {
        return false;
    };
    // `is_native` borrows; `as_xfl` consumes.
    let Ok(native) = balance.is_native() else {
        return false;
    };
    let Ok(xfl) = balance.as_xfl() else {
        return false;
    };
    let Ok(units) = xfl.to_int(0, true) else {
        return false;
    };
    let _ = line.clear();
    !native && units == IOU_AMOUNT
}

/// How much of the IOU the e2e pays in before invoking, and therefore what
/// [`check_iou_amount`] expects the trust-line balance to round-trip to.
const IOU_AMOUNT: i64 = 100;

/// A root slot reports a high-level object code (serialized type ID
/// 10001–10004), not the ordinary 14 an object *field* reports — so
/// `try_cast::<STObject>()` has to accept it, and the ordinary type IDs
/// must still be rejected.
#[inline(never)]
fn check_root_cast() -> bool {
    let Ok(root) = SlotObject::from_otxn() else {
        return false;
    };
    // The reported code's high half is what the predicate looks at.
    let Ok(code) = root.field_code() else {
        return false;
    };
    let id = code.code() >> 16;
    let is_root_code = (10001..=10004).contains(&id);
    let Ok(cast) = root.try_cast::<STObject>() else {
        return false;
    };
    let rejected = cast.try_cast::<STArray>().is_err();
    is_root_code && rejected
}

/// Which group of checks one invocation runs, read from the `CHK` parameter
/// on the originating transaction.
///
/// Each recycling loop runs more than 255 iterations — the per-execution
/// slot budget — so a single execution running all five would need
/// ~130,000 instructions against the Hook API's 65,535 limit. The e2e
/// drives one group per `Invoke` and ORs the accept codes together. Group 0
/// is everything cheap enough to share an execution.
const CHK_CHEAP: u8 = 0;
/// Group 1: the successful `slot_path!` walk, whose leaf `take_value()`
/// makes it the `take_*` success proof too.
const CHK_DEEP: u8 = 1;
/// Group 2: the `take_*` failure-path loop.
const CHK_TAKE_FAILURE: u8 = 2;
/// Group 3: the failed-`try_cast` cleanup loop.
const CHK_CAST: u8 = 3;
/// Group 4: the `slot_path!` mid-hop-failure loop.
const CHK_MIDHOP: u8 = 4;
/// Group 5: the IOU trust-line read (no loop; it needs the `ISS` parameter
/// the e2e only attaches to this invocation).
const CHK_IOU: u8 = 5;

/// Returns the requested check group, masking *any* read failure — absence
/// or an undecodable value — to [`CHK_CHEAP`]. `.get_or_default()` alone
/// only substitutes the default for absence (a malformed `CHK` would stay
/// `Err`), so the outer `.unwrap_or(..)` re-applies the same masking to a
/// decode failure too.
fn selected_group() -> u8 {
    SlotObjects
        .otxn_param
        .chk
        .get_or_default()
        .map(|v| v[0])
        .unwrap_or(CHK_CHEAP)
}

#[hooks]
pub struct SlotObjects {
    /// Selects which check group this invocation runs (absent = group 0).
    #[otxn_param(name = b"CHK", default = [CHK_CHEAP])]
    chk: OtxnParam<[u8; 1]>,

    /// The trust-line issuer for the [`CHK_IOU`] group, attached only to
    /// that invocation.
    #[otxn_param(name = b"ISS")]
    iss: OtxnParam<AccountId>,
}

#[hooks]
impl SlotObjects {
    /// Hook entry point. See the module doc comment for what each bit means.
    ///
    /// Runs the check group named by the `CHK` parameter (absent = group 0)
    /// and accepts with the bits it earned.
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let Ok(sender) = otxn_field_typed(sfAccount) else {
            rollback!(b"slot-objects: no sfAccount", SlotObjectsError::NoSender)
        };
        let Ok(keylet) = keylet_account(&sender) else {
            rollback!(
                b"slot-objects: keylet_account failed",
                SlotObjectsError::KeyletFailed
            )
        };
        let group = selected_group();

        let mut result: i64 = 0;
        match group {
            CHK_DEEP => {
                // Earns both success-path bits: see `check_deep_loop`.
                let signers = keylet_signers(&sender).unwrap_or(Keylet::zeroed());
                if check_deep_loop(&signers) {
                    result |= BIT_TAKE_LOOP | BIT_DEEP_LOOP;
                }
            }
            CHK_TAKE_FAILURE => {
                if check_take_failure_loop(&keylet) {
                    result |= BIT_TAKE_FAILURE;
                }
            }
            CHK_CAST => {
                if check_cast_cleanup_loop(&keylet) {
                    result |= BIT_CAST_CLEANUP;
                }
            }
            CHK_MIDHOP => {
                let signers = keylet_signers(&sender).unwrap_or(Keylet::zeroed());
                if check_midhop_loop(&signers) {
                    result |= BIT_MIDHOP_LOOP;
                }
            }
            CHK_IOU => {
                if check_iou_amount(&sender) {
                    result |= BIT_IOU_XFL;
                }
            }
            // CHK_CHEAP and anything unrecognized: the checks that need no loop.
            _ => {
                let account = match SlotObject::from_keylet(&keylet) {
                    Ok(a) => a,
                    Err(_) => rollback!(
                        b"slot-objects: could not slot the account root",
                        SlotObjectsError::AccountRootFailed
                    ),
                };
                if check_account_walk(&account, &sender) {
                    result |= BIT_ACCOUNT_WALK;
                }
                result |= check_balance_reads(&account);
                if check_parent_clear(&keylet) {
                    result |= BIT_PARENT_CLEAR;
                }
                if check_root_cast() {
                    result |= BIT_ROOT_CAST;
                }
                let _ = account.clear();
            }
        }

        accept!(b"slot-objects: typed slot layer checks", result)
    }
}
