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
        /// The `MIN` Hook parameter is present but malformed, or the host
        /// call to read it failed for a reason other than absence.
        CouldNotReadMinDrops = 3,
    }
}

/// The `MIN` Hook parameter's decoded shape: a single drops value, wrapped
/// in a `ParamValue` so it decodes through `FixedRead` (implemented for
/// fixed-size arrays, `rshooks::types` newtypes, `XFL`, and
/// `#[derive(ParamValue)]`/`#[derive(HookData)]` structs — not a bare
/// `u64`, which only implements `FromBytes` and decodes its bytes as
/// little-endian).
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
/// [`MinDrops::default`] only when `MIN` is absent.
/// [`HookParam::get_or_default`] already substitutes the default solely
/// for that case; a present-but-malformed `MIN` (or any other host error)
/// surfaces as `Err` here and must not be masked back to the default.
fn min_drops() -> u64 {
    match HookParams.hook_param.min.get_or_default() {
        Ok(min) => min.drops,
        Err(_) => rollback!(
            b"hook-params: could not read MIN parameter",
            HookParamsError::CouldNotReadMinDrops
        ),
    }
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
