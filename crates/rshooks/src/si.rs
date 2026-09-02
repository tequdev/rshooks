//! The Hook State Interface: [`SiFieldType`], the version-0 fixed-width
//! field codec `#[state_interface(..)]`-generated code is built on.
//!
//! See `docs/STATE_INTERFACE_DESIGN.md` for the full design. The interface
//! draft defines a `HookParameterName`/`HookParameterValue` convention that
//! exposes a Hook's state layout as a machine-readable, typed key/value
//! schema:
//!
//! ```text
//! HookParameterName  = 0x5F 0x53 0x49          ; "_SI" reserved prefix
//!                    | version (1 byte, 0x00)
//!                    | State ID (1 byte, 0x00..=0xFF)
//!                    | key field count (1 byte)
//!                    | key field descriptors...
//!
//! HookParameterValue = value field count (1 byte)
//!                    | value field descriptors...
//!
//! field descriptor    = type byte (1) | name length (1, 0x01..=0x10) | name
//! ```
//!
//! Declaration bytes are built entirely at macro time (see
//! `crates/rshooks-macros/src/hooks_struct.rs`) — a hook binary never
//! materializes a `HookParameterName`/`HookParameterValue` string at
//! runtime, so this module carries no name-building code of its own (unlike
//! [`crate::sig`], whose invocation values *are* read at runtime).
//!
//! # Why big-endian
//!
//! A state interface value crosses the same protocol boundary a raw
//! `otxn_field`/`otxn_param` read does — see DESIGN.md §5.6 ("Endianness
//! conventions"): Xahau Binary integers are big-endian. This crate's own
//! [`crate::convert::FromBytes`]/[`crate::convert::ToBytes`] little-endian
//! convention only applies to state/param values this crate's own typed
//! layer wrote for its own private use — a different domain. The state
//! interface's whole point is to make a Hook's state layout readable by an
//! external, schema-aware client, so [`SiFieldType`]'s integer impls encode
//! big-endian, for the same reason [`crate::sig::SigParamType`] does.
//!
//! # Why the full 32-byte key, not rshooks' ordinary short-key convention
//!
//! Elsewhere in this crate a hook-state key is sent to the host at its own
//! real, unpadded length (see [`crate::state`]'s module doc, "Key length
//! and padding") and the host left-pads it. The state interface instead
//! fixes the *physical* 32-byte key layout as part of its wire contract
//! (`StateID || Encode(K0) || .. || zero padding`, §1.6 of the design doc)
//! — a schema-aware external client parses the key by that fixed layout,
//! not by whatever length rshooks happened to send. So `#[state_interface]`
//! -generated code builds the full 32-byte key locally (right-zero-padded)
//! and sends all 32 bytes, rather than relying on the host's own left-pad.
//!
//! # Declarations are advisory metadata
//!
//! The protocol does not enforce that a hook actually writes what it
//! advertises: `#[state_interface]` fields still go through the ordinary
//! [`crate::state`]/[`crate::decl::State`] accessors, which the host
//! accepts unconditionally. The wire format above only shapes what a
//! schema-aware client can expect a *conforming* hook to have written; nothing
//! stops a hook from advertising a schema and then never writing to it, or
//! writing something else — same as any other machine-readable interface
//! layered on top of an otherwise-untyped protocol.

/// Whether `name` matches the interface draft's charset:
/// `[A-Za-z][A-Za-z0-9]*`, 1..=16 bytes — the macro-time twin of this
/// function is `is_valid_interface_name` in
/// `crates/rshooks-macros/src/hooks_shared.rs` (field names are validated
/// at `#[state_interface(..)]` parse time, never at runtime). The charset
/// rule itself is shared with the Hook Parameter Signature Interface — the
/// one shared copy lives in `crate::interface_name`.
pub use crate::interface_name::is_valid_name;

/// Length in bytes of the key's leading State ID byte (§1.6 of the design
/// doc) — key field encoding starts at this offset.
pub const STATE_ID_PREFIX_LEN: usize = 1;

/// Maximum total encoded width, in bytes, of a state interface key's field
/// payload (§1.6): the 32-byte key minus the 1-byte State ID prefix.
pub const MAX_KEY_PAYLOAD: usize = 31;

/// A Rust type usable as one state interface key or value field — pairs an
/// XAS-010d type code and fixed encoded width with a big-endian (integers) /
/// verbatim (byte arrays) codec. See the module doc's "Why big-endian"
/// section.
///
/// Implemented only for the version-0 fixed-width types
/// (`docs/STATE_INTERFACE_DESIGN.md` §1.5) — variable-width `STI_*` types
/// (`STI_AMOUNT`, `STI_VL`, `STI_ISSUE`, ...) have no impl and so cannot be
/// used as a `#[state_interface(..)]` key/value field type at all: using one
/// is an ordinary rustc trait-bound error.
pub trait SiFieldType: Sized {
    /// The XAS-010d type code advertised in the declaration
    /// (`docs/STATE_INTERFACE_DESIGN.md` §1.5's table): an `STI_*` code or
    /// `0x80` `XFL`.
    const TYPE_BYTE: u8;

    /// This type's fixed encoded width, in bytes.
    const WIDTH: usize;

    /// Encodes `self` into `out[..Self::WIDTH]`. `out` shorter than
    /// `Self::WIDTH` writes nothing (mirrors [`crate::convert::ToBytes::write`]'s
    /// short-buffer contract) — generated code always hands this exactly
    /// `Self::WIDTH` bytes, sliced from a fixed, macro-computed offset.
    fn write_si(&self, out: &mut [u8]);

    /// Decodes `Self` from `bytes`. `bytes` shorter than `Self::WIDTH` is
    /// treated as all-zero padding rather than panicking — generated code
    /// always hands this exactly `Self::WIDTH` bytes, so this only matters
    /// for a direct, off-the-macro-path caller.
    fn read_si(bytes: &[u8]) -> Self;
}

/// Generates a big-endian [`SiFieldType`] impl for a narrow unsigned
/// integer, via `$ty::to_be_bytes`/`$ty::from_be_bytes`.
macro_rules! be_int_si {
    ($ty:ty, $len:literal, $type_byte:literal) => {
        impl SiFieldType for $ty {
            const TYPE_BYTE: u8 = $type_byte;
            const WIDTH: usize = $len;

            #[inline(always)]
            fn write_si(&self, out: &mut [u8]) {
                if let Some(dst) = out.get_mut(..$len) {
                    dst.copy_from_slice(&self.to_be_bytes());
                }
            }

            #[inline(always)]
            fn read_si(bytes: &[u8]) -> Self {
                let mut buf = [0u8; $len];
                if let Some(src) = bytes.get(..$len) {
                    buf.copy_from_slice(src);
                }
                <$ty>::from_be_bytes(buf)
            }
        }
    };
}

impl SiFieldType for u8 {
    /// `STI_UINT8`.
    const TYPE_BYTE: u8 = 0x10;
    const WIDTH: usize = 1;

    #[inline(always)]
    fn write_si(&self, out: &mut [u8]) {
        if let Some(dst) = out.get_mut(0) {
            *dst = *self;
        }
    }

    #[inline(always)]
    fn read_si(bytes: &[u8]) -> Self {
        bytes.first().copied().unwrap_or(0)
    }
}

be_int_si!(u16, 2, 0x01); // STI_UINT16
be_int_si!(u32, 4, 0x02); // STI_UINT32
be_int_si!(u64, 8, 0x03); // STI_UINT64

/// Generates a verbatim-bytes [`SiFieldType`] impl for `[u8; $len]` itself —
/// the "byte-array types are copied verbatim" half of
/// `docs/STATE_INTERFACE_DESIGN.md` §1.5.
macro_rules! array_si {
    ($len:literal, $type_byte:literal) => {
        impl SiFieldType for [u8; $len] {
            const TYPE_BYTE: u8 = $type_byte;
            const WIDTH: usize = $len;

            #[inline(always)]
            fn write_si(&self, out: &mut [u8]) {
                if let Some(dst) = out.get_mut(..$len) {
                    dst.copy_from_slice(self);
                }
            }

            #[inline(always)]
            fn read_si(bytes: &[u8]) -> Self {
                let mut buf = [0u8; $len];
                if let Some(src) = bytes.get(..$len) {
                    buf.copy_from_slice(src);
                }
                buf
            }
        }
    };
}

array_si!(16, 0x04); // STI_UINT128
array_si!(32, 0x05); // STI_UINT256

/// Generates a verbatim-bytes [`SiFieldType`] impl for a `fixed_bytes_type!`
/// newtype (`crate::types`) wrapping `[u8; $len]`.
macro_rules! newtype_si {
    ($ty:ty, $len:literal, $type_byte:literal) => {
        impl SiFieldType for $ty {
            const TYPE_BYTE: u8 = $type_byte;
            const WIDTH: usize = $len;

            #[inline(always)]
            fn write_si(&self, out: &mut [u8]) {
                if let Some(dst) = out.get_mut(..$len) {
                    dst.copy_from_slice(&self.0);
                }
            }

            #[inline(always)]
            fn read_si(bytes: &[u8]) -> Self {
                let mut buf = [0u8; $len];
                if let Some(src) = bytes.get(..$len) {
                    buf.copy_from_slice(src);
                }
                Self(buf)
            }
        }
    };
}

newtype_si!(crate::types::Hash, 32, 0x05); // STI_UINT256
newtype_si!(crate::types::AccountId, 20, 0x08); // STI_ACCOUNT
array_si!(20, 0x11); // STI_UINT160
newtype_si!(crate::types::CurrencyCode, 20, 0x1A); // STI_CURRENCY

/// XAS-010d `XFL` — big-endian raw `int64` bit pattern, no validity check.
///
/// Deliberately not built on [`crate::convert::ToBytes`]/
/// [`crate::convert::FromBytes`]: those encode `XFL` little-endian, for this
/// crate's own hook-private state convention, the wrong byte order for this
/// protocol-facing boundary (see the module doc's "Why big-endian" section).
impl SiFieldType for crate::xfl::XFL {
    const TYPE_BYTE: u8 = 0x80;
    const WIDTH: usize = 8;

    #[inline(always)]
    fn write_si(&self, out: &mut [u8]) {
        if let Some(dst) = out.get_mut(..8) {
            dst.copy_from_slice(&self.raw_bits().to_be_bytes());
        }
    }

    #[inline(always)]
    fn read_si(bytes: &[u8]) -> Self {
        let mut buf = [0u8; 8];
        if let Some(src) = bytes.get(..8) {
            buf.copy_from_slice(src);
        }
        crate::xfl::XFL::from_raw_bits(i64::from_be_bytes(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AccountId, CurrencyCode, Hash};

    #[test]
    fn is_valid_name_accepts_the_worked_example_names() {
        assert!(is_valid_name(b"account"));
        assert!(is_valid_name(b"token"));
        assert!(is_valid_name(b"amount"));
        assert!(is_valid_name(b"updated"));
    }

    #[test]
    fn is_valid_name_rejects_empty_too_long_and_bad_charset() {
        assert!(!is_valid_name(b""));
        assert!(!is_valid_name(b"abcdefghijklmnopq")); // 17 bytes
        assert!(is_valid_name(b"abcdefghijklmnop")); // 16 bytes, ok
        assert!(!is_valid_name(b"my_field")); // underscore
        assert!(!is_valid_name(b"1field")); // leading digit
        assert!(is_valid_name(b"field1")); // trailing digit ok
    }

    #[test]
    fn u8_round_trips_and_reports_type_byte_and_width() {
        assert_eq!(<u8 as SiFieldType>::TYPE_BYTE, 0x10);
        assert_eq!(<u8 as SiFieldType>::WIDTH, 1);
        let mut buf = [0u8; 1];
        0x42u8.write_si(&mut buf);
        assert_eq!(buf, [0x42]);
        assert_eq!(u8::read_si(&buf), 0x42);
    }

    #[test]
    fn u16_encodes_big_endian() {
        assert_eq!(<u16 as SiFieldType>::TYPE_BYTE, 0x01);
        assert_eq!(<u16 as SiFieldType>::WIDTH, 2);
        let mut buf = [0u8; 2];
        0x0102u16.write_si(&mut buf);
        assert_eq!(buf, [0x01, 0x02]);
        assert_eq!(u16::read_si(&buf), 0x0102);
    }

    #[test]
    fn u32_encodes_big_endian_worked_example_token_42() {
        assert_eq!(<u32 as SiFieldType>::TYPE_BYTE, 0x02);
        assert_eq!(<u32 as SiFieldType>::WIDTH, 4);
        let mut buf = [0u8; 4];
        42u32.write_si(&mut buf);
        assert_eq!(buf, [0x00, 0x00, 0x00, 0x2A]);
        assert_eq!(u32::read_si(&buf), 42);
    }

    #[test]
    fn u64_encodes_big_endian_worked_example_amount_1000() {
        assert_eq!(<u64 as SiFieldType>::TYPE_BYTE, 0x03);
        assert_eq!(<u64 as SiFieldType>::WIDTH, 8);
        let mut buf = [0u8; 8];
        1000u64.write_si(&mut buf);
        assert_eq!(buf, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xE8]);
        assert_eq!(u64::read_si(&buf), 1000);
    }

    #[test]
    fn fixed_16_and_32_round_trip_verbatim() {
        let a = [7u8; 16];
        let mut buf = [0u8; 16];
        a.write_si(&mut buf);
        assert_eq!(buf, a);
        assert_eq!(<[u8; 16]>::read_si(&buf), a);

        let b = [9u8; 32];
        let mut buf = [0u8; 32];
        b.write_si(&mut buf);
        assert_eq!(buf, b);
        assert_eq!(<[u8; 32]>::read_si(&buf), b);
    }

    #[test]
    fn hash_round_trips_verbatim() {
        let h = Hash([1u8; 32]);
        let mut buf = [0u8; 32];
        h.write_si(&mut buf);
        assert_eq!(buf, [1u8; 32]);
        assert_eq!(Hash::read_si(&buf), h);
    }

    #[test]
    fn account_id_round_trips_verbatim_worked_example() {
        let account: [u8; 20] = [
            0x4B, 0x4E, 0x9C, 0x06, 0xF2, 0x42, 0x96, 0x07, 0x4F, 0x7B, 0xC4, 0x8F, 0x92, 0xA9,
            0x79, 0x16, 0xC6, 0xDC, 0x5E, 0xA9,
        ];
        let id = AccountId(account);
        let mut buf = [0u8; 20];
        id.write_si(&mut buf);
        assert_eq!(buf, account);
        assert_eq!(AccountId::read_si(&buf), id);
    }

    #[test]
    fn fixed_20_round_trips_verbatim() {
        let a = [3u8; 20];
        let mut buf = [0u8; 20];
        a.write_si(&mut buf);
        assert_eq!(buf, a);
        assert_eq!(<[u8; 20]>::read_si(&buf), a);
    }

    #[test]
    fn currency_code_round_trips_verbatim() {
        let c = CurrencyCode([4u8; 20]);
        let mut buf = [0u8; 20];
        c.write_si(&mut buf);
        assert_eq!(buf, [4u8; 20]);
        assert_eq!(CurrencyCode::read_si(&buf), c);
    }

    #[test]
    fn write_si_into_short_buffer_writes_nothing() {
        let mut buf = [0xFFu8; 1];
        0x0102u16.write_si(&mut buf);
        assert_eq!(buf, [0xFF]);
    }

    #[test]
    fn read_si_from_short_buffer_zero_pads_instead_of_panicking() {
        // `bytes` shorter than `WIDTH` never satisfies `bytes.get(..WIDTH)`,
        // so the scratch buffer is left all-zero rather than partially
        // filled — this is the "treated as all-zero padding" case the
        // trait's doc comment describes, not a partial big-endian read.
        assert_eq!(u16::read_si(&[0x01]), 0);
        assert_eq!(u16::read_si(&[]), 0);
    }

    #[test]
    fn xfl_reports_type_byte_and_width() {
        assert_eq!(<crate::xfl::XFL as SiFieldType>::TYPE_BYTE, 0x80);
        assert_eq!(<crate::xfl::XFL as SiFieldType>::WIDTH, 8);
    }

    #[test]
    fn xfl_one_encodes_big_endian_raw_bits() {
        let one = crate::xfl::XFL::from_raw_bits(0x54838D7EA4C68000u64 as i64);
        assert_eq!(one.raw_bits(), 0x54838D7EA4C68000u64 as i64);
        let mut buf = [0u8; 8];
        one.write_si(&mut buf);
        assert_eq!(buf, [0x54, 0x83, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00]);
        assert_eq!(
            crate::xfl::XFL::read_si(&buf).raw_bits(),
            0x54838D7EA4C68000u64 as i64
        );
    }

    #[test]
    fn xfl_zero_round_trips() {
        let zero = crate::xfl::XFL::from_raw_bits(0);
        let mut buf = [0xFFu8; 8];
        zero.write_si(&mut buf);
        assert_eq!(buf, [0u8; 8]);
        assert_eq!(crate::xfl::XFL::read_si(&buf).raw_bits(), 0);
    }

    #[test]
    fn xfl_write_si_into_short_buffer_writes_nothing() {
        let mut buf = [0xFFu8; 4];
        crate::xfl::XFL::from_raw_bits(0x54838D7EA4C68000u64 as i64).write_si(&mut buf);
        assert_eq!(buf, [0xFFu8; 4]);
    }

    #[test]
    fn xfl_read_si_from_short_buffer_zero_pads() {
        assert_eq!(crate::xfl::XFL::read_si(&[0x54, 0x83]).raw_bits(), 0);
        assert_eq!(crate::xfl::XFL::read_si(&[]).raw_bits(), 0);
    }
}
