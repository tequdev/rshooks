#![no_std]

use rshooks::prelude::*;
use rshooks::*;

/// The big-endian minimum-amount parameter name.
const MIN_PARAM: &[u8] = b"MIN";

/// The default minimum amount in drops.
const DEFAULT_MIN_DROPS: u64 = 1_000_000;

/// Flags excluded from a native amount's drops value.
const NATIVE_AMOUNT_FLAG_BITS: u64 = 0xC000_0000_0000_0000;

hook_errors! {
    /// Errors returned by the parameterized amount filter.
    pub enum HookParamsError {
        /// The transaction amount is not a native amount.
        UnsupportedAmount = 1,
        /// The native amount is below the configured minimum.
        BelowMinimum = 2,
    }
}

/// Reads the configured minimum amount, or the default.
fn min_drops() -> u64 {
    hook_param_exact(MIN_PARAM)
        .map(u64::from_be_bytes)
        .unwrap_or(DEFAULT_MIN_DROPS)
}

#[hooks]
pub struct HookParams;

#[hooks]
impl HookParams {
    /// Rejects the originating transaction if its native `Amount` falls
    /// below the configured (or default) minimum.
    #[hook(0, on = [Payment])]
    fn main(&self) -> i64 {
        let drops = match otxn_field_typed(sfAmount) {
            Ok(AmountBytes::Native(n)) => u64::from_be_bytes(n.0) & !NATIVE_AMOUNT_FLAG_BITS,
            Ok(AmountBytes::Iou(_)) | Err(_) => rollback!(
                b"hook-params: unsupported (non-native) Amount",
                HookParamsError::UnsupportedAmount
            ),
        };

        if drops < min_drops() {
            rollback!(
                b"hook-params: amount below configured minimum",
                HookParamsError::BelowMinimum
            );
        }

        accept!()
    }
}
