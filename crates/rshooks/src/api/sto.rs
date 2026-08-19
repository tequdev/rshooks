//! Serialized Transaction Object (STO) manipulation: extracting fields and
//! array elements, and inserting/removing fields.
//!
//! `sto_subfield`/`sto_subarray` return a packed `(offset, length)` value
//! (upper 32 bits = offset into the source buffer, lower 32 bits = payload
//! length — see `SUB_OFFSET`/`SUB_LENGTH` in `macro.h`,
//! `crates/rshooks-core/vendor/xahaud-hook/macro.h`) as a raw `Result<i64>`.
//! [`sto_subfield_range`]/[`sto_subarray_range`] unpack that into a plain
//! `(usize, usize)` offset/length pair, and [`sto_subfield_slice`]/
//! [`sto_subarray_slice`] go one step further and slice the source buffer
//! directly.

use crate::error::{HookError, Result, res};

/// Validate the integrity of the STO in `sto`.
#[inline(always)]
pub fn sto_validate(sto: &[u8]) -> Result<bool> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.sto_validate(sto)) {
        return res(v).map(|v| v != 0);
    }
    res(unsafe { rshooks_core::sto_validate(sto.as_ptr() as u32, sto.len() as u32) })
        .map(|v| v != 0)
}

/// Locate field `field_id` within the STO `sto`. Returns the raw packed
/// `(offset, length)` value; see the module doc comment.
#[inline(always)]
pub fn sto_subfield(sto: &[u8], field_id: impl Into<u32>) -> Result<i64> {
    let field_id = field_id.into();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.sto_subfield(sto, field_id)) {
        return res(v);
    }
    res(unsafe { rshooks_core::sto_subfield(sto.as_ptr() as u32, sto.len() as u32, field_id) })
}

/// Locate element `index` within the STO array `array`. Returns the raw
/// packed `(offset, length)` value; see the module doc comment.
#[inline(always)]
pub fn sto_subarray(array: &[u8], index: u32) -> Result<i64> {
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(v) = rshooks_core::backend::with_backend(|b| b.sto_subarray(array, index)) {
        return res(v);
    }
    res(unsafe { rshooks_core::sto_subarray(array.as_ptr() as u32, array.len() as u32, index) })
}

/// Unpack a raw packed `(offset, length)` value from `sto_subfield`/
/// `sto_subarray` into its two 32-bit components, mirroring the upstream
/// `SUB_OFFSET`/`SUB_LENGTH` macros exactly: offset = upper 32 bits, length
/// = lower 32 bits, each reinterpreted as a signed 32-bit integer
/// (`(int32_t)(x >> 32)` / `(int32_t)(x & 0xFFFFFFFFULL)`). Pure bit
/// manipulation, no host call — kept separate from [`sto_subfield_range`]/
/// [`sto_subarray_range`] so it has a standalone unit test.
#[inline(always)]
const fn unpack_sub(packed: i64) -> (i32, i32) {
    let offset = (packed >> 32) as i32;
    let length = (packed & 0xFFFF_FFFF) as i32;
    (offset, length)
}

/// Turn a raw packed `sto_subfield`/`sto_subarray` result into a
/// `(offset, length)` pair of plain `usize`s, rejecting a negative
/// component (which the packed encoding can in principle represent, though
/// no real STO ever produces one) as
/// [`HookError::ParseError`](crate::error::HookError::ParseError) — a
/// malformed field pointer rather than a valid offset/length.
#[inline(always)]
fn range_from_packed(packed: i64) -> Result<(usize, usize)> {
    let (offset, length) = unpack_sub(packed);
    if offset < 0 || length < 0 {
        return Err(HookError::ParseError);
    }
    Ok((offset as usize, length as usize))
}

/// Locate field `field_id` within the STO `sto`, returning its
/// `(offset, length)` within `sto` as plain `usize`s (unpacked from
/// [`sto_subfield`]'s raw packed value — see the module doc comment).
///
/// # Examples
///
/// ```
/// use rshooks::api::sto::sto_subfield_range;
/// use rshooks::error::HookError;
///
/// let sto = [0u8; 4];
/// assert_eq!(sto_subfield_range(&sto, 0u32), Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn sto_subfield_range(sto: &[u8], field_id: impl Into<u32>) -> Result<(usize, usize)> {
    let field_id = field_id.into();
    range_from_packed(sto_subfield(sto, field_id)?)
}

/// Locate element `index` within the STO array `array`, returning its
/// `(offset, length)` within `array` as plain `usize`s (unpacked from
/// [`sto_subarray`]'s raw packed value — see the module doc comment).
#[inline(always)]
pub fn sto_subarray_range(array: &[u8], index: u32) -> Result<(usize, usize)> {
    range_from_packed(sto_subarray(array, index)?)
}

/// Locate field `field_id` within the STO `sto` and return it as a slice of
/// `sto` directly (via [`sto_subfield_range`]). An out-of-range offset/length
/// pair (would only occur for a malformed STO) is reported as
/// [`HookError::OutOfBounds`](crate::error::HookError::OutOfBounds) rather
/// than panicking — `slice::get` is used throughout, never indexing.
///
/// # Examples
///
/// ```
/// use rshooks::api::sto::sto_subfield_slice;
/// use rshooks::error::HookError;
///
/// let sto = [0u8; 4];
/// assert_eq!(sto_subfield_slice(&sto, 0u32), Err(HookError::NotImplemented));
/// ```
#[inline(always)]
pub fn sto_subfield_slice(sto: &[u8], field_id: impl Into<u32>) -> Result<&[u8]> {
    let field_id = field_id.into();
    let (offset, length) = sto_subfield_range(sto, field_id)?;
    let end = offset.checked_add(length).ok_or(HookError::OutOfBounds)?;
    sto.get(offset..end).ok_or(HookError::OutOfBounds)
}

/// Locate element `index` within the STO array `array` and return it as a
/// slice of `array` directly (via [`sto_subarray_range`]). See
/// [`sto_subfield_slice`] for the bounds-checking / error convention.
#[inline(always)]
pub fn sto_subarray_slice(array: &[u8], index: u32) -> Result<&[u8]> {
    let (offset, length) = sto_subarray_range(array, index)?;
    let end = offset.checked_add(length).ok_or(HookError::OutOfBounds)?;
    array.get(offset..end).ok_or(HookError::OutOfBounds)
}

/// Insert or replace field `field_id` (encoded as `field`) into the STO
/// `source`, writing the result to `out`. Returns the number of bytes
/// written.
#[inline(always)]
pub fn sto_emplace<B: AsMut<[u8]> + ?Sized>(
    out: &mut B,
    source: &[u8],
    field: &[u8],
    field_id: impl Into<u32>,
) -> Result<usize> {
    let field_id = field_id.into();
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.sto_emplace(source, field, field_id))
    {
        return crate::testenv_bridge::write_bytes(out, r);
    }
    res(unsafe {
        rshooks_core::sto_emplace(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            source.as_ptr() as u32,
            source.len() as u32,
            field.as_ptr() as u32,
            field.len() as u32,
            field_id,
        )
    })
    .map(|v| v as usize)
}

/// Remove field `field_id` from the STO `source`, writing the result to
/// `out`. Returns the number of bytes written.
#[inline(always)]
pub fn sto_erase<B: AsMut<[u8]> + ?Sized>(
    out: &mut B,
    source: &[u8],
    field_id: impl Into<u32>,
) -> Result<usize> {
    let field_id = field_id.into();
    let out = out.as_mut();
    #[cfg(all(feature = "testenv", not(target_arch = "wasm32")))]
    if let Some(r) = rshooks_core::backend::with_backend(|b| b.sto_erase(source, field_id)) {
        return crate::testenv_bridge::write_bytes(out, r);
    }
    res(unsafe {
        rshooks_core::sto_erase(
            out.as_mut_ptr() as u32,
            out.len() as u32,
            source.as_ptr() as u32,
            source.len() as u32,
            field_id,
        )
    })
    .map(|v| v as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::HookError;

    #[test]
    fn smoke_not_implemented_on_host() {
        let sto = [0u8; 4];
        let mut out = [0u8; 8];
        assert_eq!(sto_validate(&sto), Err(HookError::NotImplemented));
        assert_eq!(sto_subfield(&sto, 0u32), Err(HookError::NotImplemented));
        assert_eq!(sto_subarray(&sto, 0), Err(HookError::NotImplemented));
        assert_eq!(
            sto_emplace(&mut out, &sto, &sto, 0u32),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            sto_erase(&mut out, &sto, 0u32),
            Err(HookError::NotImplemented)
        );
        assert_eq!(
            sto_subfield_range(&sto, 0u32),
            Err(HookError::NotImplemented)
        );
        assert_eq!(sto_subarray_range(&sto, 0), Err(HookError::NotImplemented));
        assert_eq!(
            sto_subfield_slice(&sto, 0u32),
            Err(HookError::NotImplemented)
        );
        assert_eq!(sto_subarray_slice(&sto, 0), Err(HookError::NotImplemented));
    }

    #[test]
    fn unpack_sub_matches_c_macros() {
        // SUB_OFFSET(x) = (int32_t)(x >> 32); SUB_LENGTH(x) = (int32_t)(x &
        // 0xFFFFFFFF) — verified against `macro.h` directly.
        assert_eq!(unpack_sub(0), (0, 0));
        // offset = 5, length = 12.
        let packed = (5i64 << 32) | 12i64;
        assert_eq!(unpack_sub(packed), (5, 12));
        // A large offset/length still round-trips through the same shift.
        let packed_big = (0x1234i64 << 32) | 0x5678i64;
        assert_eq!(unpack_sub(packed_big), (0x1234, 0x5678));
    }

    #[test]
    fn range_from_packed_rejects_negative_components() {
        // A negative unpacked length (top bit of the low 32 bits set) is
        // rejected as `ParseError`, not silently misinterpreted.
        let negative_length = 0x8000_0000i64; // offset 0, length -2147483648
        assert_eq!(
            range_from_packed(negative_length),
            Err(HookError::ParseError)
        );
        // A normal, well-formed packed value round-trips.
        let packed = (3i64 << 32) | 4i64;
        assert_eq!(range_from_packed(packed), Ok((3usize, 4usize)));
    }

    #[test]
    fn slice_helpers_use_get_not_indexing() {
        // Pure-logic check on the bounds arithmetic used by
        // `sto_subfield_slice`/`sto_subarray_slice`, independent of the host
        // call: an in-bounds range slices correctly, an out-of-bounds range
        // (offset/length beyond the buffer) is `OutOfBounds`, not a panic.
        // `clippy::indexing_slicing` is denied crate-wide, so this uses
        // `.get()` throughout rather than `data[1..3]`.
        let data = [1u8, 2, 3, 4, 5];
        let expected: &[u8] = &[2, 3];
        assert_eq!(data.get(1..3).ok_or(HookError::OutOfBounds), Ok(expected));
        assert_eq!(
            data.get(4..10).ok_or(HookError::OutOfBounds),
            Err(HookError::OutOfBounds)
        );
    }
}
