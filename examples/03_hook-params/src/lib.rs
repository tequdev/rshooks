#![no_std]

use rshooks::prelude::*;
use rshooks::*;

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

/// The `MIN` Hook parameter's decoded shape: a single drops value. A
/// one-field `ParamValue` wrapper (rather than reading a bare `[u8; 8]`)
/// lets the parameter's meaning travel with its type, and decodes through
/// this crate's little-endian `FromBytes`, matching `examples/12_typed-data`'s
/// `CFG` convention (`FixedRead` is implemented for `[u8; N]`,
/// `rshooks::types` newtypes, `XFL`, and `#[derive(ParamValue)]`/
/// `#[derive(HookData)]` structs — not for a bare `u64`).
#[derive(ParamValue)]
struct MinDrops {
    drops: u64,
}

/// The single source of the compiled-in fallback: both the declared
/// `default = ..` and the malformed-value mask below go through it.
impl Default for MinDrops {
    fn default() -> Self {
        Self {
            drops: DEFAULT_MIN_DROPS,
        }
    }
}

/// Returns the configured `MIN` value, falling back to
/// [`MinDrops::default`] when `MIN` is absent *or* present-but-malformed:
/// `.unwrap_or_default()` masks any `Err` from
/// [`HookParam::get_or_default`], not just the "absent" case.
fn min_drops() -> u64 {
    HookParams
        .hook_param
        .min
        .get_or_default()
        .unwrap_or_default()
        .drops
}

#[hooks]
pub struct HookParams {
    /// The minimum amount in drops, configured via the `MIN` Hook parameter.
    #[hook_param(name = b"MIN", default = MinDrops::default())]
    min: HookParam<MinDrops>,
}

#[hooks]
impl HookParams {
    /// Rejects the originating transaction if its native `Amount` falls
    /// below the configured (or default) minimum.
    #[hook(0, on = [Payment])]
    fn main(&self) -> HookResult {
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

        Ok(Accept::from_code(0))
    }
}
