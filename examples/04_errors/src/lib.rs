#![no_std]

use rshooks::prelude::*;
use rshooks::*;

/// The source tag rejected by this policy.
const BLOCKED_SOURCE_TAG: u32 = 13;

/// The maximum native amount in drops.
const MAX_DROPS: u64 = 100_000_000;

/// Flags excluded from a native amount's drops value.
const NATIVE_AMOUNT_FLAG_BITS: u64 = 0xC000_0000_0000_0000;

hook_errors! {
    /// Rejection reasons returned by this hook.
    pub enum RejectReason {
        /// The originating account could not be read.
        BadAccountField = -101,
        /// The source tag is blocked.
        BlockedSourceTag = -102,
        /// The amount is not native.
        NotNativeAmount = -103,
        /// The amount exceeds the policy limit.
        AmountTooLarge = -104,
    }
}

impl RejectReason {
    /// Returns this reason's rollback message.
    fn message(self) -> &'static [u8] {
        match self {
            RejectReason::BadAccountField => b"errors: could not read otxn Account",
            RejectReason::BlockedSourceTag => b"errors: blocked SourceTag",
            RejectReason::NotNativeAmount => b"errors: unsupported (non-native) Amount",
            RejectReason::AmountTooLarge => b"errors: amount exceeds policy limit",
        }
    }

    /// Rolls the hook back with this reason.
    fn rollback(self) -> ! {
        rollback!(self.message(), self)
    }
}

#[hooks]
pub struct Errors;

#[hooks]
impl Errors {
    /// Runs a short chain of policy checks, rolling back on the first
    /// failure with its own [`RejectReason`] code.
    #[hook(0, on = [Payment])]
    fn main() -> i64 {
        if otxn_field_typed(sfAccount).is_err() {
            RejectReason::BadAccountField.rollback();
        }

        match otxn_field_u64(sfSourceTag) {
            Ok(tag) if tag == u64::from(BLOCKED_SOURCE_TAG) => {
                RejectReason::BlockedSourceTag.rollback()
            }
            _ => {}
        }

        let drops = match otxn_field_typed(sfAmount) {
            Ok(AmountBytes::Native(n)) => u64::from_be_bytes(n.0) & !NATIVE_AMOUNT_FLAG_BITS,
            Ok(AmountBytes::Iou(_)) | Err(_) => RejectReason::NotNativeAmount.rollback(),
        };

        if drops > MAX_DROPS {
            RejectReason::AmountTooLarge.rollback();
        }

        accept!()
    }
}
