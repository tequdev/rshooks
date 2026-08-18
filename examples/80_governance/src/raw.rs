//! Raw XFL calls that preserve the reward calculation's error semantics.

use rshooks::raw as rshooks_core;

/// `float_set(exponent, mantissa)`, raw.
pub fn float_set(exponent: i32, mantissa: i64) -> i64 {
    unsafe { rshooks_core::float_set(exponent, mantissa) }
}

/// `float_divide(a, b)`, raw.
pub fn float_divide(a: i64, b: i64) -> i64 {
    unsafe { rshooks_core::float_divide(a, b) }
}

/// `float_multiply(a, b)`, raw.
pub fn float_multiply(a: i64, b: i64) -> i64 {
    unsafe { rshooks_core::float_multiply(a, b) }
}

/// `float_int(x, decimal_places, abs)`, raw.
pub fn float_int(x: i64, decimal_places: u32, abs: u32) -> i64 {
    unsafe { rshooks_core::float_int(x, decimal_places, abs) }
}

/// `float_sign(x)`, raw.
pub fn float_sign(x: i64) -> i64 {
    unsafe { rshooks_core::float_sign(x) }
}

/// `float_one()`, raw.
pub fn float_one() -> i64 {
    unsafe { rshooks_core::float_one() }
}

/// `float_compare(a, b, mode)`, raw.
pub fn float_compare(a: i64, b: i64, mode: u32) -> i64 {
    unsafe { rshooks_core::float_compare(a, b, mode) }
}
