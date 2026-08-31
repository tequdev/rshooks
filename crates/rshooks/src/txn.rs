//! Generic emitted-transaction encoding primitives, and the `txn_template!`
//! declarative macro for building typed, byte-exact emitted-transaction
//! templates.
//!
//! # Why there is no library-owned `PaymentTemplate` here
//!
//! `rshooks` does not hard-code a `PaymentTemplate` (or any other
//! hand-written transaction-shaped type) — mirroring xahaud's own C "Tx
//! Builder" split (template bytes pasted into the *hook's own source*, with
//! only generic helpers like `SET_UINT32`/`SET_NATIVE_AMOUNT`/`COPY_20`
//! shared):
//!
//! 1. [`codec`] — generic, panic-free encoding primitives: STObject
//!    field-header derivation from an `sfXxx` constant, native-amount encoding
//!    (62-bit check + the `0x40` native bit), and both `const fn`
//!    (compile-time, used by the macro) and runtime (`Result`-returning,
//!    usable standalone) byte writers.
//! 2. [`txn_template!`] — a declarative macro playing the role of the C code
//!    generator's output: the hook author declares an ordered field list,
//!    the macro computes cumulative offsets and the total length at compile
//!    time, bakes the field headers and defaults into a `const fn new()`
//!    (so the whole template lands in a wasm data segment via
//!    [`crate::static_cell::HookStatic`]), and generates typed setters.
//!
//! The macro recognizes the emit-plumbing fields (`FirstLedgerSequence`/
//! `LastLedgerSequence`, `Account`, `Fee`) by their `sfXxx` code value
//! among the uniform field kinds; `EmitDetails` is instead a structural
//! marker tracked separately (see [`txn_template!`]'s grammar section).
//! The macro generates `prepare_for_emit()` itself, replicating xahaud's C
//! `PREPARE_TXN()`/`PREPARE_PAYMENT_SIMPLE` semantics — see
//! `examples/10_emit-txn` for the worked example.
//!
//! [`crate::views`] ships library-owned per-transaction and per-ledger-entry
//! shapes despite the above, because they are *generated* from xahaud's
//! vendored format macros (`scripts/sync-vendor.sh` + `cargo xtask
//! gen-core`, checked by `cargo xtask gen-core --check` in CI) rather than
//! hand-maintained. Hand-written shape-specific code otherwise remains
//! banned; [`codec`]/[`txn_template!`]'s split of responsibilities is
//! unaffected; and [`crate::views`] is read-only — `sfEmitDetails` and the
//! other host-only fields are still written by
//! `prepare_for_emit`/[`crate::sto_writer`], which upstream's format macros
//! say nothing about.

/// Generic, panic-free STObject field-header and value-encoding primitives.
///
/// Every function here is layout-relevant but transaction-shape-agnostic:
/// none of them know about `Payment`, `EmitDetails`, or any other concrete
/// transaction type. [`txn_template!`](crate::txn_template) is built
/// entirely out of these.
pub mod codec {
    use crate::error::{HookError, Result};
    use crate::types::{ACC_ID_LEN, AccountId, SField};

    /// Native (XRP/XAH) amounts reserve their top 2 bits for control flags;
    /// a drops value must fit in the remaining 62 bits.
    pub const MAX_NATIVE_DROPS: u64 = 1u64 << 62;

    /// Derives the STObject field-id prefix bytes for a field.
    ///
    /// Takes the typed [`SField`] constant itself (`sfAccount`, `sfFee`,
    /// ...), not a raw code — `SField`'s constructor is crate-private, so a
    /// raw-`u32` overload would be the only way to build a header for a code
    /// no generated constant names.
    ///
    /// A field's code encodes `type = code >> 16` and
    /// `field = code & 0xFFFF`. Returns the prefix bytes (only the first `N`
    /// of the 3 are meaningful) and `N`, the number of meaningful bytes, per
    /// rippled's canonical field-id encoding:
    ///
    /// - `type < 16 && field < 16`: **1 byte**, `(type << 4) | field`.
    /// - `type < 16 && field >= 16`: **2 bytes**, `[type << 4, field]`.
    /// - `type >= 16 && field < 16`: **2 bytes**, `[field << 4, type]`.
    /// - `type >= 16 && field >= 16`: **3 bytes**, `[0, type, field]`.
    ///
    /// See `KNOWN_HEADERS` in the unit tests below for a table of real
    /// `sfcodes` verified against known-good wire bytes.
    ///
    /// The two-byte forms only have room for a single-byte `type`/`field`
    /// number; this holds for every real `sfcode` (`assert!`s below turn a
    /// hypothetical `>= 256` code into a compile error rather than silent
    /// truncation).
    #[must_use]
    pub const fn field_header<T>(f: SField<T>) -> ([u8; 3], usize) {
        let sfcode = f.code();
        let ty = sfcode.wrapping_shr(16);
        let field = sfcode & 0xFFFF;
        if ty < 16 && field < 16 {
            let byte0 = (ty.wrapping_shl(4) | field) as u8;
            ([byte0, 0, 0], 1)
        } else if ty < 16 {
            assert!(field < 256, "field_header: field code must fit in a byte");
            let byte0 = ty.wrapping_shl(4) as u8;
            let byte1 = field as u8;
            ([byte0, byte1, 0], 2)
        } else if field < 16 {
            assert!(ty < 256, "field_header: type code must fit in a byte");
            let byte0 = field.wrapping_shl(4) as u8;
            let byte1 = ty as u8;
            ([byte0, byte1, 0], 2)
        } else {
            assert!(
                ty < 256 && field < 256,
                "field_header: type and field codes must fit in a byte"
            );
            ([0, ty as u8, field as u8], 3)
        }
    }

    /// Header + value size of an STI_UINT16 `TransactionType` field (the
    /// only field `txn_template!`'s `transaction_type = ttXXX` key emits).
    #[must_use]
    pub const fn transaction_type_field_size<T>(f: SField<T>) -> usize {
        field_header(f).1.wrapping_add(2)
    }

    /// Header + value size of an STI_UINT32 field (`txn_template!`'s
    /// `u32_field` kind).
    #[must_use]
    pub const fn u32_field_size<T>(f: SField<T>) -> usize {
        field_header(f).1.wrapping_add(4)
    }

    /// Header + value size of an STI_AMOUNT field encoded as a native
    /// (XRP/XAH) amount (`txn_template!`'s `native_amount` kind).
    #[must_use]
    pub const fn native_amount_field_size<T>(f: SField<T>) -> usize {
        field_header(f).1.wrapping_add(8)
    }

    /// Header + value size, in bytes, of an STI_ACCOUNT field: header + a
    /// 1-byte VL length prefix (always `20`, which fits the single-byte VL
    /// length form) + the 20-byte payload (`txn_template!`'s `account_id`
    /// kind).
    #[must_use]
    pub const fn account_id_field_size<T>(f: SField<T>) -> usize {
        field_header(f).1.wrapping_add(1).wrapping_add(ACC_ID_LEN)
    }

    /// Header + value size, in bytes, of an STI_VL field encoded as an
    /// **empty** blob: header + a 1-byte zero-length VL marker, no payload
    /// (`txn_template!`'s `empty_vl` kind — e.g. `SigningPubKey` on an
    /// emitted transaction).
    #[must_use]
    pub const fn empty_vl_field_size<T>(f: SField<T>) -> usize {
        field_header(f).1.wrapping_add(1)
    }

    /// Writes an STObject field-header (see [`field_header`]) into `bytes`
    /// at `offset`, at compile time.
    ///
    /// # Panics (compile-time only)
    ///
    /// Panics if the header would not fit within `bytes` — this function is
    /// only ever called from a `const` context (`txn_template!`'s generated
    /// `new()`), where a panic is a compile error, never a runtime one.
    #[allow(clippy::indexing_slicing)] // in-bounds per the assert above; const-only, see the Panics note
    pub const fn write_field_header<const N: usize, T>(
        bytes: &mut [u8; N],
        offset: usize,
        f: SField<T>,
    ) {
        let (hdr, hdr_len) = field_header(f);
        let mut i = 0;
        while i < hdr_len {
            let dst = offset.wrapping_add(i);
            assert!(dst < N, "txn_template!: field header write out of bounds");
            bytes[dst] = hdr[i];
            i = i.wrapping_add(1);
        }
    }

    /// Writes `src` into `bytes` starting at `offset`, at compile time. Used
    /// by `txn_template!`'s generated `new()` to bake in default field
    /// values.
    ///
    /// # Panics (compile-time only)
    ///
    /// See [`write_field_header`] — same const-context-only guarantee.
    #[allow(clippy::indexing_slicing)] // same justification as `write_field_header`
    pub const fn write_const_bytes<const N: usize>(bytes: &mut [u8; N], offset: usize, src: &[u8]) {
        let mut i = 0;
        while i < src.len() {
            let dst = offset.wrapping_add(i);
            assert!(dst < N, "txn_template!: field value write out of bounds");
            bytes[dst] = src[i];
            i = i.wrapping_add(1);
        }
    }

    /// Encodes `drops` as an 8-byte native amount at compile time: top byte
    /// `0x40 | ((drops >> 56) & 0x3F)`, remaining 7 bytes big-endian. Used by
    /// `txn_template!` to bake in a `native_amount` field's default.
    ///
    /// # Panics (compile-time only)
    ///
    /// Panics if `drops >=`[`MAX_NATIVE_DROPS`] — only ever called from a
    /// `const` context, where this is a compile error (a `native_amount`
    /// default that doesn't fit is a template-authoring bug, not something
    /// to discover at runtime).
    #[must_use]
    pub const fn encode_native_amount_const(drops: u64) -> [u8; 8] {
        assert!(
            drops < MAX_NATIVE_DROPS,
            "txn_template!: native_amount default does not fit in 62 bits"
        );
        let mut value = drops.to_be_bytes();
        value[0] |= 0x40;
        value
    }

    /// Runtime, `Result`-returning counterpart to
    /// [`encode_native_amount_const`]: encodes `drops` as an 8-byte native
    /// amount into `out[..8]`.
    ///
    /// # Errors
    ///
    /// Returns [`HookError::InvalidArgument`] if `drops >=`
    /// [`MAX_NATIVE_DROPS`], or if `out` is shorter than 8 bytes.
    #[inline(always)]
    pub fn encode_native_amount(out: &mut [u8], drops: u64) -> Result<()> {
        if drops >= MAX_NATIVE_DROPS {
            return Err(HookError::InvalidArgument);
        }
        let dst = out.get_mut(0..8).ok_or(HookError::InvalidArgument)?;
        let mut value = drops.to_be_bytes();
        value[0] |= 0x40;
        dst.copy_from_slice(&value);
        Ok(())
    }

    /// Writes `value` as 4 big-endian bytes into `bytes[offset..offset+4]`.
    /// Stands in for the C Tx Builder's `SET_UINT32`, for hooks that patch
    /// transaction bytes directly (`txn_template!`'s generated setters don't
    /// use this — they write with compile-time-proven-in-bounds offsets).
    ///
    /// # Errors
    ///
    /// Returns [`HookError::InvalidArgument`] if `offset + 4 > bytes.len()`.
    #[inline(always)]
    pub fn write_u32_be(bytes: &mut [u8], offset: usize, value: u32) -> Result<()> {
        let end = offset.checked_add(4).ok_or(HookError::InvalidArgument)?;
        let dst = bytes
            .get_mut(offset..end)
            .ok_or(HookError::InvalidArgument)?;
        dst.copy_from_slice(&value.to_be_bytes());
        Ok(())
    }

    /// Writes `value`'s 20 bytes into `bytes[offset..offset+20]`. A generic
    /// runtime primitive standing in for the C Tx Builder's `COPY_20`; see
    /// [`write_u32_be`]'s doc comment for why `txn_template!`'s own
    /// generated `account_id` setters don't call this.
    ///
    /// # Errors
    ///
    /// Returns [`HookError::InvalidArgument`] if `offset + 20 >
    /// bytes.len()`.
    #[inline(always)]
    pub fn write_account_id(bytes: &mut [u8], offset: usize, value: &AccountId) -> Result<()> {
        let end = offset
            .checked_add(ACC_ID_LEN)
            .ok_or(HookError::InvalidArgument)?;
        let dst = bytes
            .get_mut(offset..end)
            .ok_or(HookError::InvalidArgument)?;
        dst.copy_from_slice(value.as_ref());
        Ok(())
    }

    // --- Field table: value-based detection of the emit-plumbing fields ---
    //
    // `txn_template!` recognizes the fields `prepare_for_emit` needs
    // (`sfSequence`, `sfFirstLedgerSequence`, `sfLastLedgerSequence`,
    // `sfFee`, `sfSigningPubKey`, `sfAccount`) by their `sfXxx` code
    // *value*, not by any special declaration syntax — every field is
    // declared with the same uniform kinds (`u32_field`, `native_amount`,
    // `account_id`, `empty_vl`). The macro accumulates one
    // `(sfcode, kind tag, payload offset)` row per declared field (see
    // `FieldEntry`) into a per-template const table, and the helpers below
    // do the compile-time lookups that back the macro's presence/
    // kind-agreement checks and `prepare_for_emit`'s offset resolution.

    /// Kind tag for a `u32_field` table row.
    pub const KIND_U32_FIELD: u8 = 0;
    /// Kind tag for a `native_amount` table row.
    pub const KIND_NATIVE_AMOUNT: u8 = 1;
    /// Kind tag for an `account_id` table row.
    pub const KIND_ACCOUNT_ID: u8 = 2;
    /// Kind tag for an `empty_vl` table row.
    pub const KIND_EMPTY_VL: u8 = 3;

    /// One row of a `txn_template!` field table: `(sfcode, kind tag,
    /// payload offset)`. `payload offset` is the offset of the field's
    /// *value* (after the header, and after the VL length byte for
    /// `account_id`) — the same offset each kind's generated setter writes
    /// to.
    pub type FieldEntry = (u32, u8, usize);

    /// Finds `sfcode` in `table`, at compile time, returning its
    /// `(kind tag, payload offset)` if present. `table` is a template's
    /// generated `FIELDS` const; comparison is by the `sfcode`'s runtime
    /// *value*, so it is robust to how the constant was spelled at the
    /// declaration site (qualified path, alias, ...).
    #[must_use]
    #[allow(clippy::indexing_slicing)] // in-bounds per the `i < table.len()` guard; const-only, see the module doc
    pub const fn find_field(table: &[FieldEntry], sfcode: u32) -> Option<(u8, usize)> {
        let mut i = 0;
        while i < table.len() {
            let (code, kind, off) = table[i];
            if code == sfcode {
                return Some((kind, off));
            }
            i = i.wrapping_add(1);
        }
        None
    }

    /// Whether `sfcode` appears anywhere in `table`, regardless of kind.
    /// Backs `txn_template!`'s required-field *presence* checks.
    #[must_use]
    pub const fn field_present(table: &[FieldEntry], sfcode: u32) -> bool {
        find_field(table, sfcode).is_some()
    }

    /// Whether `sfcode`, if present in `table`, was declared with `kind`.
    /// Returns `true` when `sfcode` is absent — absence is the separate
    /// [`field_present`] check's job, so a missing field surfaces exactly
    /// one named error (from that check), not a redundant second one from
    /// this one. Backs `txn_template!`'s required-field *kind-agreement*
    /// checks (e.g. `sfFee` must be `native_amount`).
    #[must_use]
    pub const fn field_kind_ok(table: &[FieldEntry], sfcode: u32, kind: u8) -> bool {
        match find_field(table, sfcode) {
            Some((k, _)) => k == kind,
            None => true,
        }
    }

    /// Resolves `sfcode`'s payload offset in `table`, or `default` if
    /// absent. Used only to keep `prepare_for_emit`'s body well-typed when
    /// a required field is missing from `table` — that code path is dead
    /// in practice, since a missing field already fails the crate's build
    /// via [`field_present`]'s generated assertion before `prepare_for_emit`
    /// could ever run.
    #[must_use]
    pub const fn field_offset_or(table: &[FieldEntry], sfcode: u32, default: usize) -> usize {
        match find_field(table, sfcode) {
            Some((_, off)) => off,
            None => default,
        }
    }

    #[cfg(test)]
    mod tests {
        #![allow(clippy::expect_used, clippy::indexing_slicing)] // tests are exempt from panic-freedom lints, docs/DESIGN.md §8

        use super::*;

        /// `(sfcode, expected header bytes)`: real `sfcodes` from
        /// `rshooks_core::sfcodes` with known-good wire bytes, plus
        /// `sfCloseResolution`/`sfTickSize` (`type >= 16`) to exercise the
        /// two- and three-byte forms.
        ///
        /// Raw codes, wrapped in an erased `SField` where passed: the table
        /// stays spelled as `(type, field)` arithmetic, an independent
        /// transcription of the wire format rather than a re-read of the
        /// generated constants.
        #[rustfmt::skip]
        const KNOWN_HEADERS: &[(u32, &[u8])] = &[
            ((1 << 16) + 2, &[0x12]),           // TransactionType (1,2)
            ((2 << 16) + 2, &[0x22]),           // Flags (2,2)
            ((2 << 16) + 3, &[0x23]),           // SourceTag (2,3)
            ((2 << 16) + 4, &[0x24]),           // Sequence (2,4)
            ((2 << 16) + 14, &[0x2E]),          // DestinationTag (2,14)
            ((2 << 16) + 26, &[0x20, 0x1A]),    // FirstLedgerSequence (2,26)
            ((2 << 16) + 27, &[0x20, 0x1B]),    // LastLedgerSequence (2,27)
            ((6 << 16) + 1, &[0x61]),           // Amount (6,1)
            ((6 << 16) + 8, &[0x68]),           // Fee (6,8)
            ((7 << 16) + 3, &[0x73]),           // SigningPubKey (7,3)
            ((8 << 16) + 1, &[0x81]),           // Account (8,1)
            ((8 << 16) + 3, &[0x83]),           // Destination (8,3)
            ((16 << 16) + 1, &[0x10, 0x10]),    // sfCloseResolution (16,1): type>=16, field<16
            ((16 << 16) + 16, &[0x00, 0x10, 0x10]), // sfTickSize (16,16): type>=16, field>=16
        ];

        #[test]
        fn field_header_matches_known_patterns() {
            for &(sfcode, expected) in KNOWN_HEADERS {
                let (hdr, hdr_len) = field_header(SField::<crate::types::Opaque>::new(sfcode));
                assert_eq!(
                    &hdr[..hdr_len],
                    expected,
                    "sfcode 0x{sfcode:08X} (type {}, field {})",
                    sfcode >> 16,
                    sfcode & 0xFFFF,
                );
            }
        }

        #[test]
        fn native_amount_one_drop() {
            assert_eq!(encode_native_amount_const(1), [0x40, 0, 0, 0, 0, 0, 0, 1]);
            let mut out = [0u8; 8];
            encode_native_amount(&mut out, 1).expect("1 drop is in range");
            assert_eq!(out, [0x40, 0, 0, 0, 0, 0, 0, 1]);
        }

        #[test]
        fn native_amount_zero_drops() {
            assert_eq!(encode_native_amount_const(0), [0x40, 0, 0, 0, 0, 0, 0, 0]);
        }

        #[test]
        fn native_amount_rejects_out_of_range() {
            let mut out = [0u8; 8];
            assert_eq!(
                encode_native_amount(&mut out, 1u64 << 62),
                Err(HookError::InvalidArgument)
            );
        }

        #[test]
        #[should_panic(expected = "does not fit in 62 bits")]
        fn native_amount_const_rejects_out_of_range() {
            let _ = encode_native_amount_const(1u64 << 62);
        }

        #[test]
        fn write_u32_be_rejects_out_of_bounds() {
            let mut buf = [0u8; 2];
            assert_eq!(
                write_u32_be(&mut buf, 0, 1),
                Err(HookError::InvalidArgument)
            );
        }

        #[test]
        fn write_u32_be_writes_big_endian() {
            let mut buf = [0u8; 4];
            write_u32_be(&mut buf, 0, 0x0102_0304).expect("in bounds");
            assert_eq!(buf, [0x01, 0x02, 0x03, 0x04]);
        }

        #[test]
        fn write_account_id_writes_at_offset() {
            let mut buf = [0u8; 22];
            let id = AccountId([0xAB; ACC_ID_LEN]);
            write_account_id(&mut buf, 2, &id).expect("in bounds");
            assert_eq!(&buf[..2], &[0, 0]);
            assert_eq!(&buf[2..], &id[..]);
        }

        const SAMPLE_TABLE: &[FieldEntry] =
            &[(100, KIND_U32_FIELD, 4), (200, KIND_NATIVE_AMOUNT, 12)];

        #[test]
        fn find_field_locates_present_entry() {
            assert_eq!(
                find_field(SAMPLE_TABLE, 200),
                Some((KIND_NATIVE_AMOUNT, 12))
            );
        }

        #[test]
        fn find_field_returns_none_for_absent_entry() {
            assert_eq!(find_field(SAMPLE_TABLE, 999), None);
        }

        #[test]
        fn field_present_matches_find_field() {
            assert!(field_present(SAMPLE_TABLE, 100));
            assert!(!field_present(SAMPLE_TABLE, 999));
        }

        #[test]
        fn field_kind_ok_checks_kind_when_present() {
            assert!(field_kind_ok(SAMPLE_TABLE, 100, KIND_U32_FIELD));
            assert!(!field_kind_ok(SAMPLE_TABLE, 100, KIND_NATIVE_AMOUNT));
        }

        #[test]
        fn field_kind_ok_is_vacuously_true_when_absent() {
            // Absence is `field_present`'s job to report; this avoids a
            // redundant second error for the same missing field.
            assert!(field_kind_ok(SAMPLE_TABLE, 999, KIND_U32_FIELD));
        }

        #[test]
        fn field_offset_or_resolves_or_falls_back() {
            assert_eq!(field_offset_or(SAMPLE_TABLE, 100, 0), 4);
            assert_eq!(field_offset_or(SAMPLE_TABLE, 999, 42), 42);
        }
    }
}

/// Implemented automatically for every `txn_template!`-generated type.
/// Names that type's generated `bytes()` accessor generically, purely so
/// [`Prepared`] can call it without knowing the concrete template type.
/// Hook authors never call this trait's method directly — use the
/// generated inherent `bytes()` instead.
pub trait TemplateBytes {
    /// Returns the full, fixed `Self::LEN`-capacity backing buffer (the
    /// same value the generated inherent `bytes()` method returns).
    fn template_bytes(&self) -> &[u8];
}

/// A `txn_template!` template that has finished `prepare_for_emit` (see
/// [`txn_template!`]'s grammar docs) — the compile-time proof that the
/// emit-plumbing fields (`FirstLedgerSequence`/`LastLedgerSequence`/
/// `Account`/`EmitDetails`/`Fee`) were actually filled before anyone reads
/// out an emit-ready blob. The unprepared template type has no
/// `as_bytes`/`emit` method of its own — only `Prepared` does — so code
/// that tries to emit without preparing first fails to compile rather than
/// silently emitting a stale or zeroed buffer.
///
/// # Why a borrow, not an owned typestate
///
/// `txn_template!`-generated structs typically live behind
/// [`crate::static_cell::HookStatic`], whose
/// [`take`](crate::static_cell::HookStatic::take) only ever hands out
/// `&'static mut T`, never an owned `T`. Borrowing `&mut T` here matches
/// that: while a `Prepared<'_, T>` is alive, the borrow checker forbids
/// using the original `&mut T` directly, so code can't keep mutating
/// through a stale handle while believing it still reflects an unprepared
/// buffer. Setters remain reachable through
/// [`core::ops::Deref`]/[`core::ops::DerefMut`], so re-adjusting a field and
/// calling `prepare_for_emit` again is fine.
///
/// Obtained only from a generated `prepare_for_emit()`; [`Self::new`] is
/// `#[doc(hidden)]` and crate-external code has no other way to produce a
/// `Prepared`.
#[must_use = "a Prepared handle that is never read via as_bytes()/emit() means prepare_for_emit's work was wasted"]
pub struct Prepared<'a, T: TemplateBytes> {
    inner: &'a mut T,
    len: usize,
}

impl<'a, T: TemplateBytes> Prepared<'a, T> {
    /// Wraps an already-`prepare_for_emit`-completed `inner` together with
    /// the real serialized length `prepare_for_emit` computed. Only
    /// `txn_template!`'s generated `prepare_for_emit` calls this.
    #[doc(hidden)]
    pub fn new(inner: &'a mut T, len: usize) -> Self {
        Self { inner, len }
    }

    /// The real, emit-ready transaction bytes: a prefix of the template's
    /// full reserved buffer, sized to what `etxn_details` actually returned
    /// at `prepare_for_emit` time — never the full reserved capacity. Pass
    /// this directly to [`crate::api::etxn::emit`]/
    /// [`crate::api::etxn::emit_buf`] (or just call [`Self::emit`]).
    #[inline(always)]
    #[must_use]
    #[allow(clippy::indexing_slicing)] // `len` came from prepare_for_emit's own bounds-checked computation
    pub fn as_bytes(&self) -> &[u8] {
        &self.inner.template_bytes()[..self.len]
    }

    /// Emits [`Self::as_bytes`] as a new transaction, returning its hash.
    /// Convenience wrapper over [`crate::api::etxn::emit_buf`] — see its
    /// docs for the `etxn_reserve` precondition.
    ///
    /// # Errors
    ///
    /// Propagates [`crate::api::etxn::emit_buf`]'s errors.
    #[inline(always)]
    pub fn emit(&self) -> crate::error::Result<crate::types::Hash> {
        crate::api::etxn::emit_buf(self.as_bytes())
    }
}

impl<'a, T: TemplateBytes> core::ops::Deref for Prepared<'a, T> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &T {
        self.inner
    }
}

impl<'a, T: TemplateBytes> core::ops::DerefMut for Prepared<'a, T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        self.inner
    }
}

impl<'a, T: TemplateBytes> core::fmt::Debug for Prepared<'a, T> {
    // Doesn't require `T: Debug` (the generated template types don't derive
    // it) — just enough to make `Result<Prepared<'_, T>, _>::expect_err`
    // callable in tests, per `Result::expect_err`'s `T: Debug` bound.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Prepared").field("len", &self.len).finish()
    }
}

/// Declares a typed, byte-exact emitted-transaction template.
///
/// Modeled on xahaud's C "Tx Builder" split (see the module doc comment):
/// the hook author declares an ordered field list, and the macro computes
/// cumulative offsets and the total length at compile time, bakes the field
/// headers and defaults into a `const fn new()` (so the result composes with
/// `HookStatic<T>` and lands in a wasm data segment — see
/// [`crate::static_cell::HookStatic`]), and generates typed setters.
///
/// # Grammar
///
/// ```text
/// txn_template! {
///     $(#[$meta])* $vis struct Name {
///         transaction_type = ttXXX,                 // required, first
///         field_name: u32_field(sfXxx) = default,    // any count/order after this
///         field_name: native_amount(sfXxx) = default,
///         field_name: account_id(sfXxx),
///         field_name: empty_vl(sfXxx),
///         field_name: emit_details,                  // must be LAST
///     }
/// }
/// ```
///
/// Every field uses one of these same four uniform kinds (plus
/// `emit_details`) — there is no separate "role" syntax. The macro
/// recognizes the handful of fields an emitted transaction always needs
/// (see "Required fields" below) **by their `sfXxx` code value**, not by
/// which keyword declared them:
///
/// - `u32_field(sfXxx) = default` — an STI_UINT32 field with a `u32`
///   default.
/// - `native_amount(sfXxx) = default` — an STI_AMOUNT field, always encoded
///   as a native (XRP/XAH) amount; `default` is a `u64` drops value (`0`
///   encodes as the `0x40`-prefixed zero amount).
/// - `account_id(sfXxx)` — an STI_ACCOUNT field; always defaults to the
///   all-zero `AccountId`.
/// - `empty_vl(sfXxx)` — an STI_VL field encoded as an **empty** blob (a
///   1-byte zero-length VL marker, no payload) — this is what
///   `SigningPubKey` looks like on an emitted transaction. No setter is
///   generated (there is nothing to set).
/// - `emit_details` — reserves
///   [`EMIT_DETAILS_MAX_LEN`](crate::types::EMIT_DETAILS_MAX_LEN) zeroed
///   bytes with **no header** (the host's `etxn_details` writes its own,
///   complete field). Has no `sfcode` (it is a structural marker, not an
///   STObject field), so it is tracked separately from the value-based
///   detection below. Must be the last declared field; declaring anything
///   after it is a macro-parse compile error.
///
/// ## Required fields, and `prepare_for_emit()`
///
/// An emitted transaction is invalid at the protocol level without
/// `Sequence`, `FirstLedgerSequence`, `LastLedgerSequence`, `Fee`,
/// `SigningPubKey`, and `Account` — plus an `EmitDetails` field. **Every
/// `txn_template!` declaration must include all of these**, in any
/// relative order (subject to the canonical `sfcode` ordering below and
/// `emit_details` being last), each declared with the kind that matches
/// what `prepare_for_emit` needs to do with it:
///
/// | required field        | `sfcode`               | kind             |
/// |------------------------|-------------------------|------------------|
/// | Sequence               | `sfSequence`             | `u32_field`      |
/// | FirstLedgerSequence     | `sfFirstLedgerSequence`  | `u32_field`      |
/// | LastLedgerSequence      | `sfLastLedgerSequence`   | `u32_field`      |
/// | Fee                    | `sfFee`                  | `native_amount`  |
/// | SigningPubKey          | `sfSigningPubKey`        | `empty_vl`       |
/// | Account                | `sfAccount`              | `account_id`     |
/// | *(structural, no sfcode)* | —                     | `emit_details`   |
///
/// The macro accumulates a `(sfcode, kind, payload offset)` row per
/// declared field into a compile-time table (see
/// [`crate::txn::codec::FieldEntry`]) and, in its single generated
/// expansion arm, emits named `const _: () = assert!(...)` checks (all
/// failures are `E0080`, one independent item per check so multiple
/// problems are all reported, not just the first):
///
/// - **presence**: each of the six `sfXxx` codes above must appear in the
///   table, and an `emit_details` field must have been declared (tracked
///   separately, since it has no `sfcode`).
/// - **kind agreement**: whichever of the six codes *is* present must have
///   been declared with the kind in the table above — `sfFee` declared as
///   `u32_field` instead of `native_amount`, for instance, is a compile
///   error, because `prepare_for_emit` would otherwise corrupt the
///   template writing an 8-byte native amount over a 4-byte `u32_field`'s
///   worth of space (or vice versa).
///
/// Because detection is by the `sfcode`'s runtime *value*, it doesn't
/// matter how that constant is spelled at the declaration site — a
/// qualified path or a re-exported alias works identically to the
/// unqualified `sfXxx` name.
///
/// Because the seven required fields are mandatory, `prepare_for_emit(&mut
/// self) -> Result<Prepared<'_, Self>>` (see
/// [`Prepared`](crate::txn::Prepared)) is generated **unconditionally** by
/// every `txn_template!` invocation that compiles. It:
///
/// 1. Reads `ledger_seq()`, writes `FirstLedgerSequence = ledger_seq + 1`
///    and `LastLedgerSequence = FirstLedgerSequence + 4`.
/// 2. Writes `Account` from `hook_account()`.
/// 3. Calls `etxn_details()` into the reserved `EmitDetails` region and
///    takes its *returned* length (not the region's max capacity — the
///    actual serialized `EmitDetails` is 116 or 138 bytes depending on
///    whether this hook's module exports `cbak`).
/// 4. Computes the real blob length as `emit_details offset + returned
///    length`, slices `bytes()` to exactly that length, and calls
///    `etxn_fee_base()` over *that* slice (not the full reserved region) to
///    get the fee, then writes `Fee`.
/// 5. Wraps `self` together with that real blob length in a
///    [`Prepared`](crate::txn::Prepared) handle and returns it — see
///    [`Prepared::as_bytes`](crate::txn::Prepared::as_bytes)/
///    [`Prepared::emit`](crate::txn::Prepared::emit). There is no way to
///    obtain an emit-sized slice, or call `emit`, without going through
///    `prepare_for_emit` first: the unprepared type has no `as_bytes`/
///    `emit` method of its own, and `Prepared` is only ever constructed
///    here — this is the compile-time fix for the overwrite footgun
///    described next (see `docs/DESIGN.md` §5.5 for the full rationale).
///
/// **`prepare_for_emit` overwrites whatever `FirstLedgerSequence`,
/// `LastLedgerSequence`, `Fee`, and `Account` were set to** — their setters
/// exist (see below) but any value written through them before calling
/// `prepare_for_emit` is discarded. `Sequence` and `SigningPubKey` are never
/// touched at runtime at all — their baked defaults (`0`, and the empty VL
/// marker) are already correct. The returned `Prepared<'_, Self>` derefs to
/// `Self`, so setters remain callable afterward too (e.g. to adjust a field
/// and call `prepare_for_emit` again) — only re-running `prepare_for_emit`
/// itself refreshes the five emit-plumbing fields again.
///
/// # Setter names
///
/// A field declared `flags: u32_field(sfFlags) = 0` gets a method
/// `fn set_flags(&mut self, value: u32)`, synthesized via
/// `$crate::__paste!`'s `[<set_ $field>]` splice — **uniformly, for every
/// `u32_field`/`native_amount`/`account_id` field**, including the required
/// ones above (`set_sequence`, the two ledger-sequence setters, `set_fee`,
/// `set_account` all exist; see the overwrite note above for why setting
/// them is rarely useful once `prepare_for_emit` is in the picture).
/// `native_amount` setters take a `u64` named `drops` (native amounts are
/// always in drops) and return `Result<()>` (out of range is fallible);
/// `account_id` setters take `&AccountId` and are infallible. `empty_vl`
/// fields get no setter (there is nothing to set).
///
/// `$crate::__paste!` (from `rshooks-macros`) is `rshooks`'s own
/// stable-Rust replacement for nightly's `${concat(...)}` metavariable
/// expression — see its doc comment for the `[< .. >]` splice syntax it
/// recognizes. It is invoked from `txn_template!`'s own expansion, so
/// nothing crate-root-level is required of whichever crate calls
/// `txn_template!` (unlike the nightly feature this replaced).
///
/// # Generated items
///
/// `Self::LEN`, `Self::new()`, a `derive(Clone)` (a trivial byte-buffer
/// copy — the generated type is always a fixed-size byte array underneath;
/// required unconditionally by
/// [`HookStatic<T: Clone>`](crate::static_cell::HookStatic)), one
/// `set_<field>` per `u32_field`/`native_amount`/`account_id` field
/// (`empty_vl` gets none), `emit_details_region()`, `bytes()`, a `Default`
/// impl equivalent to `new()`, an `impl` [`TemplateBytes`](crate::txn::TemplateBytes)
/// forwarding to `bytes()` (so [`Prepared`](crate::txn::Prepared) can name
/// the type generically), and `prepare_for_emit()` (see above) — the last
/// three are unconditional because the required fields, including
/// `emit_details`, are mandatory.
///
/// # Compile-time canonical-order check
///
/// Declared fields' `sfXxx` codes must be strictly increasing (canonical
/// `(type, field)` order, since `sfcode = (type << 16) | field` and `field`
/// is always 16 bits) — a compile error otherwise. This also catches a
/// duplicated field (two entries with the same `sfcode` violate *strictly*
/// increasing order). `emit_details` has no `sfcode` and is exempt.
///
/// # Examples
///
/// ```
/// use rshooks::prelude::*;
/// use rshooks::txn_template;
///
/// txn_template! {
///     /// A minimal made-up template exercising every field kind, including
///     /// all six required fields (canonical order:
///     /// `sfSequence < sfFirstLedgerSequence < sfLastLedgerSequence <
///     /// sfAmount < sfFee < sfSigningPubKey < sfAccount < sfDestination`).
///     pub struct Example {
///         transaction_type = ttPAYMENT,
///         flags: u32_field(sfFlags) = tfCANONICAL,
///         sequence: u32_field(sfSequence) = 0,
///         first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
///         last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
///         amount: native_amount(sfAmount) = 0,
///         fee: native_amount(sfFee) = 0,
///         signing_pub_key: empty_vl(sfSigningPubKey),
///         account: account_id(sfAccount),
///         destination: account_id(sfDestination),
///         emit_details: emit_details,
///     }
/// }
///
/// let mut tpl = Example::new();
/// tpl.set_flags(0);
/// tpl.set_amount(1).expect("1 drop is in range");
/// tpl.set_destination(&AccountId([0xAB; ACC_ID_LEN]));
/// let _ = tpl.emit_details_region();
/// assert_eq!(tpl.bytes().len(), Example::LEN);
/// // All six required fields plus `emit_details` are declared, so
/// // `prepare_for_emit` always exists (host stubs deterministically fail
/// // here — this only proves the method exists and is callable; see the
/// // rshooks test suite for the byte-compat and required-field
/// // regression proofs).
/// assert!(tpl.prepare_for_emit().is_err());
/// ```
///
/// Fields out of canonical order fail to compile (E0080: the const
/// ordering assertion fires — the error code is pinned so this test cannot
/// silently pass for an unrelated reason, e.g. a missing feature gate). All
/// required fields are declared correctly here so the *only* failure is
/// the order violation (`destination` placed before `flags`):
/// ```compile_fail,E0080
/// use rshooks::prelude::*;
/// use rshooks::txn_template;
///
/// txn_template! {
///     struct BadOrder {
///         transaction_type = ttPAYMENT,
///         destination: account_id(sfDestination),
///         flags: u32_field(sfFlags) = 0,
///         sequence: u32_field(sfSequence) = 0,
///         first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
///         last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
///         amount: native_amount(sfAmount) = 0,
///         fee: native_amount(sfFee) = 0,
///         signing_pub_key: empty_vl(sfSigningPubKey),
///         account: account_id(sfAccount),
///         emit_details: emit_details,
///     }
/// }
/// ```
///
/// A field after `emit_details` fails to compile (macro-grammar rejection,
/// before the required-field checks even run):
/// ```compile_fail
/// use rshooks::prelude::*;
/// use rshooks::txn_template;
///
/// txn_template! {
///     struct BadEmitDetails {
///         transaction_type = ttPAYMENT,
///         emit_details: emit_details,
///         flags: u32_field(sfFlags) = 0,
///     }
/// }
/// ```
///
/// A template missing a required field fails to compile: `E0080` from that
/// field's dedicated *presence* `assert!`, whose message names exactly
/// which `sfcode` is absent. Here every required field but `sfSequence` is
/// declared, so the error text contains `` missing required \`sfSequence\`
/// field `` and nothing else is wrong (canonical order and every other
/// required field are fine):
/// ```compile_fail,E0080
/// use rshooks::prelude::*;
/// use rshooks::txn_template;
///
/// txn_template! {
///     struct MissingField {
///         transaction_type = ttPAYMENT,
///         // sfSequence never declared
///         first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
///         last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
///         fee: native_amount(sfFee) = 0,
///         signing_pub_key: empty_vl(sfSigningPubKey),
///         account: account_id(sfAccount),
///         emit_details: emit_details,
///     }
/// }
/// ```
///
/// A required field declared with the wrong kind fails to compile: `E0080`
/// from that field's dedicated *kind-agreement* `assert!`. Here every
/// required field is present, but `sfFee` is declared as `u32_field`
/// instead of `native_amount`:
/// ```compile_fail,E0080
/// use rshooks::prelude::*;
/// use rshooks::txn_template;
///
/// txn_template! {
///     struct WrongKind {
///         transaction_type = ttPAYMENT,
///         sequence: u32_field(sfSequence) = 0,
///         first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
///         last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
///         fee: u32_field(sfFee) = 0, // WRONG: sfFee must be native_amount
///         signing_pub_key: empty_vl(sfSigningPubKey),
///         account: account_id(sfAccount),
///         emit_details: emit_details,
///     }
/// }
/// ```
///
/// Trying to obtain emit-ready bytes (or to `emit`) before calling
/// `prepare_for_emit` fails to compile: the unprepared template type has no
/// `as_bytes`/`emit` method at all — only the
/// [`Prepared`](crate::txn::Prepared) handle `prepare_for_emit` returns
/// does. This is the actual footgun fix (`docs/DESIGN.md` §5.5): the
/// compiler, not a runtime check, refuses code that would read out an
/// emit-sized blob whose `FirstLedgerSequence`/`LastLedgerSequence`/
/// `Account`/`EmitDetails`/`Fee` were never actually filled in.
/// ```compile_fail
/// use rshooks::prelude::*;
/// use rshooks::txn_template;
///
/// txn_template! {
///     struct EmitWithoutPrepare {
///         transaction_type = ttPAYMENT,
///         sequence: u32_field(sfSequence) = 0,
///         first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
///         last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
///         fee: native_amount(sfFee) = 0,
///         signing_pub_key: empty_vl(sfSigningPubKey),
///         account: account_id(sfAccount),
///         emit_details: emit_details,
///     }
/// }
///
/// let mut tpl = EmitWithoutPrepare::new();
/// let _ = tpl.as_bytes(); // ERROR: no method named `as_bytes` on `EmitWithoutPrepare`
/// ```
#[macro_export]
macro_rules! txn_template {
    (
        $(#[$meta:meta])*
        $vis:vis struct $Name:ident {
            transaction_type = $tt:ident
            $(, $($fields:tt)*)?
        }
    ) => {
        $crate::__txn_template_step! {
            @step
            name = $Name,
            meta = [$(#[$meta])*],
            vis = $vis,
            order = [$crate::sfield::sfTransactionType.code()],
            setters = [],
            emit_region = [],
            buf = [__bytes],
            init = [
                $crate::txn::codec::write_field_header(&mut __bytes, 0usize, $crate::sfield::sfTransactionType);
                $crate::txn::codec::write_const_bytes(
                    &mut __bytes,
                    $crate::txn::codec::field_header($crate::sfield::sfTransactionType).1,
                    &(($crate::raw::tts::$tt) as u16).to_be_bytes(),
                );
            ],
            prev = [ $crate::txn::codec::transaction_type_field_size($crate::sfield::sfTransactionType) ],
            table = [],
            emit_details = [false, 0usize],
            fields = [ $($($fields)*)? ]
        }
    };
}

/// Internal recursive tt-muncher backing [`txn_template!`](crate::txn_template).
///
/// `#[doc(hidden)]` but necessarily `#[macro_export]`ed (a macro invoked as
/// `$crate::name!` from another macro's expansion must be exported). Each
/// `@step` peels one field off `fields = [...]`, appending to the
/// accumulators (`setters`, `init`, `order`, `emit_region`) and advancing
/// `prev` — the cumulative byte offset, threaded as a token stream so every
/// offset stays a compile-time expression built from
/// [`crate::txn::codec`]'s `const fn`s, never separately recomputed.
///
/// Two extra accumulators back value-based required-field detection:
///
/// - `table` accumulates one `(($sfcode).code(), kind tag, payload offset)` tuple
///   literal per plain-kind field (`u32_field`/`native_amount`/
///   `account_id`/`empty_vl`), becoming the generated `$Name::FIELDS` const
///   array. `emit_details` has no `sfcode`, so it contributes no row.
/// - `emit_details` holds `[presence flag, offset]` — `[false, 0usize]`
///   until an `emit_details` field is declared, `[true, (offset_expr)]`
///   after (structurally guaranteed to happen at most once: the
///   `emit_details` field must be last, so a second one would leave
///   unconsumed tokens and fail to parse before ever reaching this
///   accumulator).
///
/// There is a single, unconditional base case (`fields = []`): every field
/// kind is uniform (no per-field "role" arms), so `prepare_for_emit()` and
/// `$Name::FIELDS` are always generated. A duplicated field is caught by the
/// canonical-order assert, since two equal `sfcode`s violate
/// strictly-increasing order. Whether the crate actually compiles comes down to
/// thirteen independent `const _: () = assert!(...)` items generated in
/// that same base case: six required-field *presence* checks and six
/// *kind-agreement* checks (via [`crate::txn::codec::field_present`] /
/// [`crate::txn::codec::field_kind_ok`] over `$Name::FIELDS`, at const-eval
/// time), plus one presence check for `emit_details` (sourced from its own
/// accumulator, since it isn't in the table). Each is a separate `const`
/// item — a single `const`'s initializer panics at its first failing
/// statement, so grouping them would only ever surface one error; separate
/// items let rustc evaluate and report all of them independently.
#[doc(hidden)]
#[macro_export]
macro_rules! __txn_template_step {
    // u32_field
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        fields = [ $field:ident : u32_field($sfcode:expr) = $default:expr $(, $($rest:tt)*)? ]
    ) => {
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* , ($sfcode).code()],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "` (default `", stringify!($default), "`). Overwritten by `prepare_for_emit` if this is one of the required emit-plumbing fields.")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction: `Self::LEN` sums these same field sizes
                $vis fn [<set_ $field>](&mut self, value: u32) {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    self.bytes[OFF..OFF.wrapping_add(4)].copy_from_slice(&value.to_be_bytes());
                }
            ],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
                $crate::txn::codec::write_const_bytes(
                    &mut $($buf)*,
                    ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1),
                    &(($default) as u32).to_be_bytes(),
                );
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::u32_field_size($sfcode)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_U32_FIELD, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1)),
            ],
            emit_details = [$($emit_details)*],
            fields = [ $($($rest)*)? ]
        }
    };

    // native_amount
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        fields = [ $field:ident : native_amount($sfcode:expr) = $default:expr $(, $($rest:tt)*)? ]
    ) => {
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* , ($sfcode).code()],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "` to `drops` native XAH/XRP (default `", stringify!($default), "` drops). Overwritten by `prepare_for_emit` if this is the required `sfFee` field.")]
                ///
                /// # Errors
                ///
                /// Returns [`crate::error::HookError::InvalidArgument`] if
                /// `drops` does not fit in 62 bits (native amounts reserve
                /// their top 2 bits for control flags).
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds as above; only `drops` itself is runtime-fallible
                $vis fn [<set_ $field>](&mut self, drops: u64) -> $crate::error::Result<()> {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    $crate::txn::codec::encode_native_amount(&mut self.bytes[OFF..OFF.wrapping_add(8)], drops)
                }
            ],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
                $crate::txn::codec::write_const_bytes(
                    &mut $($buf)*,
                    ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1),
                    &$crate::txn::codec::encode_native_amount_const($default),
                );
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::native_amount_field_size($sfcode)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_NATIVE_AMOUNT, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1)),
            ],
            emit_details = [$($emit_details)*],
            fields = [ $($($rest)*)? ]
        }
    };

    // account_id
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        fields = [ $field:ident : account_id($sfcode:expr) $(, $($rest:tt)*)? ]
    ) => {
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* , ($sfcode).code()],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "` (defaults to the all-zero `AccountId`). Overwritten by `prepare_for_emit` if this is the required `sfAccount` field.")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction, as above
                $vis fn [<set_ $field>](&mut self, value: &$crate::types::AccountId) {
                    const OFF: usize = ($($prev)*)
                        .wrapping_add($crate::txn::codec::field_header($sfcode).1)
                        .wrapping_add(1);
                    self.bytes[OFF..OFF.wrapping_add($crate::types::ACC_ID_LEN)].copy_from_slice(value.as_ref());
                }
            ],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
                $crate::txn::codec::write_const_bytes(
                    &mut $($buf)*,
                    ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1),
                    &[$crate::types::ACC_ID_LEN as u8],
                );
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::account_id_field_size($sfcode)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_ACCOUNT_ID, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1).wrapping_add(1)),
            ],
            emit_details = [$($emit_details)*],
            fields = [ $($($rest)*)? ]
        }
    };

    // empty_vl (no setter)
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        fields = [ $field:ident : empty_vl($sfcode:expr) $(, $($rest:tt)*)? ]
    ) => {
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* , ($sfcode).code()],
            setters = [$($setters)*],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
                $crate::txn::codec::write_const_bytes(
                    &mut $($buf)*,
                    ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1),
                    &[0u8],
                );
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::empty_vl_field_size($sfcode)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_EMPTY_VL, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1)),
            ],
            emit_details = [$($emit_details)*],
            fields = [ $($($rest)*)? ]
        }
    };

    // emit_details must be last: this arm only accepts an optional trailing
    // comma after it, so anything declared afterward is unconsumed tokens —
    // a macro-parse compile error. No `sfcode`, so it doesn't join `table`;
    // its own offset is recorded directly into the `emit_details`
    // accumulator for `prepare_for_emit` and the presence check to use.
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        fields = [ $field:ident : emit_details $(,)? ]
    ) => {
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)*],
            setters = [$($setters)*],
            emit_region = [
                #[doc = concat!("Returns the mutable `", stringify!($field), "` region: pass it directly to [`crate::api::etxn::etxn_details`].")]
                #[inline(always)]
                #[must_use]
                #[allow(clippy::indexing_slicing)] // in-bounds: exactly the trailing region `txn_template!` reserved
                $vis fn emit_details_region(&mut self) -> &mut [u8] {
                    const OFF: usize = $($prev)*;
                    &mut self.bytes[OFF..OFF.wrapping_add($crate::types::EMIT_DETAILS_MAX_LEN)]
                }
            ],
            buf = [$($buf)*],
            init = [$($init)*],
            prev = [ ($($prev)*).wrapping_add($crate::types::EMIT_DETAILS_MAX_LEN) ],
            table = [$($table)*],
            emit_details = [true, (($($prev)*))],
            fields = []
        }
    };

    // Base case (single, unconditional): no fields left. `table` and
    // `emit_details` bind unconditionally — `emit_details` is ALWAYS
    // exactly `[bool, expr]` (a real offset if declared, `0usize` if not),
    // so its `:expr` fragment always matches. `$Name::FIELDS` and
    // `prepare_for_emit()` are generated unconditionally; whether the
    // crate actually compiles is entirely down to the thirteen
    // presence/kind-agreement assert items below.
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$ed_p:tt, $ed_off:expr],
        fields = []
    ) => {
        $(#[$meta])*
        #[derive(Clone)]
        $vis struct $Name {
            bytes: [u8; $Name::LEN],
        }

        // `[<set_ $field>]` setter names (spliced into `$($setters)*` above,
        // per-field, by the `u32_field`/`native_amount`/`account_id` arms)
        // are only resolved to real identifiers here, by wrapping the whole
        // impl block in `$crate::__paste!` — `rshooks`'s own stable
        // replacement for nightly's `${concat(...)}` (see the `Setter
        // names` section above and `rshooks_macros::paste`'s doc comment).
        $crate::__paste! {
        impl $Name {
            /// Total serialized length: the fixed-position field prefix,
            /// plus the reserved `EmitDetails` region.
            pub const LEN: usize = $($prev)*;

            /// Builds the template with every field header and default
            /// baked in at compile time. `const fn`, so it composes with
            /// `HookStatic<Self>` and lands in a wasm data segment rather
            /// than being materialized at runtime.
            #[must_use]
            pub const fn new() -> Self {
                let mut $($buf)* = [0u8; Self::LEN];
                $($init)*
                Self { bytes: $($buf)* }
            }

            $($setters)*

            $($emit_region)*

            /// Returns the entire backing array (full [`Self::LEN`]
            /// capacity).
            #[inline(always)]
            #[must_use]
            $vis fn bytes(&self) -> &[u8] {
                &self.bytes
            }

            /// This template's compile-time field table: one
            /// `(sfcode, kind tag, payload offset)` row per declared
            /// `u32_field`/`native_amount`/`account_id`/`empty_vl` field
            /// (`emit_details` excluded — it has no `sfcode`). Backs the
            /// required-field presence/kind-agreement checks below and
            /// `prepare_for_emit`'s offset resolution; see
            /// [`crate::txn::codec::FieldEntry`].
            const FIELDS: &'static [$crate::txn::codec::FieldEntry] = &[$($table)*];

            /// Fills the emit-plumbing fields (`FirstLedgerSequence`,
            /// `LastLedgerSequence`, `Account`, `EmitDetails`, `Fee` —
            /// `Sequence` is left at its baked `0` default) and returns a
            /// [`Prepared`](crate::txn::Prepared) handle wrapping `self`
            /// together with the transaction's real serialized length
            /// (sized to `EmitDetails`'s actual returned length, not
            /// [`Self::LEN`]'s reserved capacity). See
            /// [`Prepared::as_bytes`](crate::txn::Prepared::as_bytes)/
            /// [`Prepared::emit`](crate::txn::Prepared::emit) — this is the
            /// only way to obtain emit-ready bytes or call `emit`. Mirrors
            /// xahaud's C `PREPARE_TXN()`: `EmitDetails` is filled before
            /// `Fee`, since `etxn_fee_base` needs the `EmitDetails`-sized
            /// blob to size the fee. **Overwrites** whatever
            /// `FirstLedgerSequence`, `LastLedgerSequence`, `Fee`, and
            /// `Account` were previously set to via their generated
            /// setters. Precondition: the caller must have already reserved
            /// an emission slot via [`crate::api::etxn::etxn_reserve`] —
            /// this method does not call it.
            ///
            /// # Errors
            ///
            /// Propagates any error from the underlying `hook_account`,
            /// `etxn_details`, or `etxn_fee_base` host calls
            /// (`ledger_seq` cannot fail), or returns
            /// [`crate::error::HookError::InvalidArgument`] if the computed
            /// blob length exceeds [`Self::LEN`].
            #[inline(always)]
            #[allow(clippy::indexing_slicing)] // in-bounds by construction, as the setters above
            $vis fn prepare_for_emit(&mut self) -> $crate::error::Result<$crate::txn::Prepared<'_, Self>> {
                let __fls = $crate::api::ledger::ledger_seq().wrapping_add(1);
                {
                    const OFF: usize = $crate::txn::codec::field_offset_or(
                        $Name::FIELDS,
                        $crate::sfield::sfFirstLedgerSequence.code(),
                        0usize,
                    );
                    self.bytes[OFF..OFF.wrapping_add(4)].copy_from_slice(&__fls.to_be_bytes());
                }
                {
                    const OFF: usize = $crate::txn::codec::field_offset_or(
                        $Name::FIELDS,
                        $crate::sfield::sfLastLedgerSequence.code(),
                        0usize,
                    );
                    self.bytes[OFF..OFF.wrapping_add(4)].copy_from_slice(&__fls.wrapping_add(4).to_be_bytes());
                }
                {
                    const OFF: usize = $crate::txn::codec::field_offset_or(
                        $Name::FIELDS,
                        $crate::sfield::sfAccount.code(),
                        0usize,
                    );
                    $crate::api::hook_ctx::hook_account(
                        &mut self.bytes[OFF..OFF.wrapping_add($crate::types::ACC_ID_LEN)],
                    )?;
                }
                const ED_OFF: usize = $ed_off;
                let __edlen = $crate::api::etxn::etxn_details(
                    &mut self.bytes[ED_OFF..ED_OFF.wrapping_add($crate::types::EMIT_DETAILS_MAX_LEN)],
                )?;
                let __total_len = ED_OFF.wrapping_add(__edlen);
                let __fee = {
                    let __blob = self
                        .bytes
                        .get(..__total_len)
                        .ok_or($crate::error::HookError::InvalidArgument)?;
                    $crate::api::etxn::etxn_fee_base(__blob)?
                };
                {
                    const OFF: usize = $crate::txn::codec::field_offset_or(
                        $Name::FIELDS,
                        $crate::sfield::sfFee.code(),
                        0usize,
                    );
                    $crate::txn::codec::encode_native_amount(&mut self.bytes[OFF..OFF.wrapping_add(8)], __fee)?;
                }
                Ok($crate::txn::Prepared::new(self, __total_len))
            }
        }
        } // $crate::__paste!

        impl ::core::default::Default for $Name {
            /// Equivalent to [`Self::new`].
            #[inline(always)]
            fn default() -> Self {
                Self::new()
            }
        }

        impl $crate::txn::TemplateBytes for $Name {
            #[inline(always)]
            fn template_bytes(&self) -> &[u8] {
                self.bytes()
            }
        }

        #[allow(non_snake_case)]
        const _: () = {
            const ORDER: &[u32] = &[$($order)*];
            let mut i = 1;
            while i < ORDER.len() {
                assert!(
                    ORDER[i - 1] < ORDER[i],
                    "txn_template!: fields must be declared in canonical (type, field) order (sfXxx codes must be strictly increasing) — this also catches a duplicated field, since two equal sfcodes violate strict ordering"
                );
                i = i.wrapping_add(1);
            }
        };

        // Thirteen required-field checks (E0080), each its own `const`
        // item so every problem is reported, not just the first one found:
        // six presence checks, six kind-agreement checks (both via
        // value-based lookup in `$Name::FIELDS`, robust to how the sfcode
        // constant was spelled at the declaration site), and one presence
        // check for `emit_details` (which has no sfcode, so it is tracked
        // via its own accumulator instead of the table).
        const _: () = assert!(
            $crate::txn::codec::field_present($Name::FIELDS, $crate::sfield::sfSequence.code()),
            "txn_template!: missing required `sfSequence` field — declare a field as `<name>: u32_field(sfSequence) = 0,`"
        );
        const _: () = assert!(
            $crate::txn::codec::field_present($Name::FIELDS, $crate::sfield::sfFirstLedgerSequence.code()),
            "txn_template!: missing required `sfFirstLedgerSequence` field — declare a field as `<name>: u32_field(sfFirstLedgerSequence) = 0,`"
        );
        const _: () = assert!(
            $crate::txn::codec::field_present($Name::FIELDS, $crate::sfield::sfLastLedgerSequence.code()),
            "txn_template!: missing required `sfLastLedgerSequence` field — declare a field as `<name>: u32_field(sfLastLedgerSequence) = 0,`"
        );
        const _: () = assert!(
            $crate::txn::codec::field_present($Name::FIELDS, $crate::sfield::sfFee.code()),
            "txn_template!: missing required `sfFee` field — declare a field as `<name>: native_amount(sfFee) = 0,`"
        );
        const _: () = assert!(
            $crate::txn::codec::field_present($Name::FIELDS, $crate::sfield::sfSigningPubKey.code()),
            "txn_template!: missing required `sfSigningPubKey` field — declare a field as `<name>: empty_vl(sfSigningPubKey),`"
        );
        const _: () = assert!(
            $crate::txn::codec::field_present($Name::FIELDS, $crate::sfield::sfAccount.code()),
            "txn_template!: missing required `sfAccount` field — declare a field as `<name>: account_id(sfAccount),`"
        );
        #[allow(clippy::assertions_on_constants)] // `$ed_p` is a literal `true`/`false` baked in at macro-expansion time, not a runtime condition
        const _: () = assert!(
            $ed_p,
            "txn_template!: missing required `emit_details` field — declare a field as `<name>: emit_details,` (must be the last declared field)"
        );
        const _: () = assert!(
            $crate::txn::codec::field_kind_ok($Name::FIELDS, $crate::sfield::sfSequence.code(), $crate::txn::codec::KIND_U32_FIELD),
            "txn_template!: `sfSequence` must be declared as `u32_field(sfSequence)` — prepare_for_emit would corrupt the template with any other kind"
        );
        const _: () = assert!(
            $crate::txn::codec::field_kind_ok($Name::FIELDS, $crate::sfield::sfFirstLedgerSequence.code(), $crate::txn::codec::KIND_U32_FIELD),
            "txn_template!: `sfFirstLedgerSequence` must be declared as `u32_field(sfFirstLedgerSequence)` — prepare_for_emit would corrupt the template with any other kind"
        );
        const _: () = assert!(
            $crate::txn::codec::field_kind_ok($Name::FIELDS, $crate::sfield::sfLastLedgerSequence.code(), $crate::txn::codec::KIND_U32_FIELD),
            "txn_template!: `sfLastLedgerSequence` must be declared as `u32_field(sfLastLedgerSequence)` — prepare_for_emit would corrupt the template with any other kind"
        );
        const _: () = assert!(
            $crate::txn::codec::field_kind_ok($Name::FIELDS, $crate::sfield::sfFee.code(), $crate::txn::codec::KIND_NATIVE_AMOUNT),
            "txn_template!: `sfFee` must be declared as `native_amount(sfFee)` — prepare_for_emit would corrupt the template with any other kind (it writes an 8-byte native amount)"
        );
        const _: () = assert!(
            $crate::txn::codec::field_kind_ok($Name::FIELDS, $crate::sfield::sfSigningPubKey.code(), $crate::txn::codec::KIND_EMPTY_VL),
            "txn_template!: `sfSigningPubKey` must be declared as `empty_vl(sfSigningPubKey)` — any other kind would encode the wrong wire representation"
        );
        const _: () = assert!(
            $crate::txn::codec::field_kind_ok($Name::FIELDS, $crate::sfield::sfAccount.code(), $crate::txn::codec::KIND_ACCOUNT_ID),
            "txn_template!: `sfAccount` must be declared as `account_id(sfAccount)` — prepare_for_emit would corrupt the template with any other kind (it writes a 20-byte AccountId)"
        );
    };
}

#[cfg(test)]
mod tests {
    // Tests are exempt from the panic-freedom lints (see docs/DESIGN.md
    // §8); expect/indexing on known-good values is idiomatic here.
    #![allow(clippy::expect_used, clippy::indexing_slicing)]

    use crate::types::{ACC_ID_LEN, AccountId, EMIT_DETAILS_MAX_LEN};
    use rshooks_core::consts::tfCANONICAL;
    // The typed constants: `txn_template!` calls `.code()` on whatever it is
    // given, so its field list takes `SField`s, not raw `u32`s.
    use crate::sfield::{
        sfAccount, sfAmount, sfDestination, sfDestinationTag, sfFee, sfFirstLedgerSequence,
        sfFlags, sfLastLedgerSequence, sfSequence, sfSigningPubKey, sfSourceTag,
    };

    crate::txn_template! {
        /// Payment template used to verify serialized field order; see
        /// `EXPECTED_FIXED_PREFIX` below for the byte-compat proof.
        struct TestPayment {
            transaction_type = ttPAYMENT,
            flags: u32_field(sfFlags) = tfCANONICAL,
            source_tag: u32_field(sfSourceTag) = 0,
            sequence: u32_field(sfSequence) = 0,
            destination_tag: u32_field(sfDestinationTag) = 0,
            first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
            last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
            amount: native_amount(sfAmount) = 0,
            fee: native_amount(sfFee) = 0,
            signing_pub_key: empty_vl(sfSigningPubKey),
            account: account_id(sfAccount),
            destination: account_id(sfDestination),
            emit_details: emit_details,
        }
    }

    /// The exact 99-byte fixed prefix a hand-written `PaymentTemplate`
    /// produces for this field set. Byte equality against this
    /// hand-transcribed array is the regression proof that `txn_template!`
    /// reproduces it exactly.
    #[rustfmt::skip]
    const EXPECTED_FIXED_PREFIX: [u8; 99] = [
        0x12, 0x00, 0x00,                                                        // TransactionType (1,2)
        0x22, 0x80, 0x00, 0x00, 0x00,                                            // Flags (2,2): tfCANONICAL
        0x23, 0x00, 0x00, 0x00, 0x00,                                            // SourceTag (2,3)
        0x24, 0x00, 0x00, 0x00, 0x00,                                            // Sequence (2,4): required field
        0x2E, 0x00, 0x00, 0x00, 0x00,                                            // DestinationTag (2,14)
        0x20, 0x1A, 0x00, 0x00, 0x00, 0x00,                                      // FirstLedgerSequence (2,26): required field
        0x20, 0x1B, 0x00, 0x00, 0x00, 0x00,                                      // LastLedgerSequence (2,27): required field
        0x61, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,                    // Amount (6,1): native 0 drops
        0x68, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,                    // Fee (6,8): required field, native 0 drops
        0x73, 0x00,                                                              // SigningPubKey (7,3): required field, empty VL
        0x81, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,   // Account (8,1): required field, VL(20)
        0x83, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,   // Destination (8,3): VL(20)
    ];

    #[test]
    fn matches_expected_fixed_prefix_byte_for_byte() {
        let tpl = TestPayment::new();
        assert_eq!(&tpl.bytes()[..99], &EXPECTED_FIXED_PREFIX[..]);
    }

    #[test]
    fn len_is_fixed_prefix_plus_emit_details_max() {
        assert_eq!(TestPayment::LEN, 99 + EMIT_DETAILS_MAX_LEN);
    }

    #[test]
    fn const_template_zeroes_emit_details_region() {
        let tpl = TestPayment::new();
        assert_eq!(&tpl.bytes()[99..], &[0u8; EMIT_DETAILS_MAX_LEN][..]);
    }

    #[test]
    fn amount_setter_writes_at_the_same_offset_as_before() {
        let mut tpl = TestPayment::new();
        tpl.set_amount(1).expect("1 drop is in range");
        assert_eq!(&tpl.bytes()[36..44], &[0x40, 0, 0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn remaining_u32_and_native_amount_setters_write_at_the_same_offsets_as_before() {
        // Setters exist even for the required fields; `prepare_for_emit`
        // overwrites them at emit time, but they still work standalone.
        let mut tpl = TestPayment::new();
        tpl.set_flags(0x1234_5678);
        tpl.set_source_tag(1);
        tpl.set_sequence(2);
        tpl.set_destination_tag(3);
        tpl.set_first_ledger_sequence(4);
        tpl.set_last_ledger_sequence(5);
        tpl.set_fee(6).expect("6 drops is in range");

        assert_eq!(&tpl.bytes()[4..8], &0x1234_5678u32.to_be_bytes());
        assert_eq!(&tpl.bytes()[9..13], &1u32.to_be_bytes());
        assert_eq!(&tpl.bytes()[14..18], &2u32.to_be_bytes());
        assert_eq!(&tpl.bytes()[19..23], &3u32.to_be_bytes());
        assert_eq!(&tpl.bytes()[25..29], &4u32.to_be_bytes());
        assert_eq!(&tpl.bytes()[31..35], &5u32.to_be_bytes());
        assert_eq!(&tpl.bytes()[45..53], &[0x40, 0, 0, 0, 0, 0, 0, 6]);
    }

    #[test]
    fn amount_setter_rejects_out_of_range() {
        let mut tpl = TestPayment::new();
        assert_eq!(
            tpl.set_amount(1u64 << 62),
            Err(crate::error::HookError::InvalidArgument)
        );
    }

    #[test]
    fn destination_setter_writes_at_the_same_offset_as_before() {
        let reference = TestPayment::new();
        let mut tpl = TestPayment::new();
        let dest = AccountId([0xAB; ACC_ID_LEN]);
        tpl.set_destination(&dest);

        assert_eq!(&tpl.bytes()[79..99], dest.as_ref());
        // Nothing outside the Destination value range changed.
        assert_eq!(&tpl.bytes()[..79], &reference.bytes()[..79]);
        assert_eq!(&tpl.bytes()[99..], &reference.bytes()[99..]);
    }

    #[test]
    fn account_setter_writes_at_the_same_offset_as_before() {
        let mut tpl = TestPayment::new();
        let acct = AccountId([0xCD; ACC_ID_LEN]);
        tpl.set_account(&acct);
        assert_eq!(&tpl.bytes()[57..77], acct.as_ref());
    }

    #[test]
    fn emit_details_region_is_the_trailing_region() {
        let mut tpl = TestPayment::new();
        assert_eq!(tpl.emit_details_region().len(), EMIT_DETAILS_MAX_LEN);
    }

    #[test]
    fn default_matches_new() {
        assert_eq!(TestPayment::default().bytes(), TestPayment::new().bytes());
    }

    #[test]
    fn prepare_for_emit_propagates_host_stub_errors() {
        // On the host target every Hook API call is a deterministic
        // `NOT_IMPLEMENTED` stub (see rshooks-core), so `prepare_for_emit`
        // must fail on its very first host call (`ledger_seq`) and
        // propagate that error rather than panicking or silently
        // succeeding.
        let mut tpl = TestPayment::new();
        // `Prepared` (the `Ok` variant) doesn't implement `PartialEq` — only
        // the error path is ever compared here — so pull the `Err` out with
        // `expect_err` first rather than `assert_eq!`ing the whole `Result`.
        assert_eq!(
            tpl.prepare_for_emit()
                .expect_err("prepare_for_emit must fail on the host stub"),
            crate::error::HookError::NotImplemented
        );
    }

    // Declares `sfAccount` under a different name for `QualifiedPathAccount`
    // below, to prove required-field detection is genuinely value-based —
    // it must not matter how the constant was spelled at the declaration
    // site. Still an `SField`, since the macro calls `.code()` on it.
    use crate::sfield::sfAccount as raw_sf_account;

    crate::txn_template! {
        /// See the `raw_sf_account` import above: declares `sfAccount` via
        /// a differently-spelled path to prove detection is by value.
        struct QualifiedPathAccount {
            transaction_type = ttPAYMENT,
            sequence: u32_field(sfSequence) = 0,
            first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
            last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
            fee: native_amount(sfFee) = 0,
            signing_pub_key: empty_vl(sfSigningPubKey),
            account: account_id(raw_sf_account),
            emit_details: emit_details,
        }
    }

    #[test]
    fn required_field_detection_is_robust_to_qualified_paths() {
        // Compiling with `account_id(raw_sf_account)` above is most of the
        // proof. Exercise every generated method too (dead-code hygiene and
        // a smoke test of the full generated surface), ending with
        // `prepare_for_emit`, which still fails on the host stub but from
        // the correct first call.
        let mut tpl = QualifiedPathAccount::new();
        tpl.set_sequence(0);
        tpl.set_first_ledger_sequence(0);
        tpl.set_last_ledger_sequence(0);
        tpl.set_fee(0).expect("0 drops is in range");
        tpl.set_account(&AccountId::default());
        let _ = tpl.emit_details_region();
        assert_eq!(tpl.bytes().len(), QualifiedPathAccount::LEN);
        assert_eq!(
            tpl.prepare_for_emit()
                .expect_err("prepare_for_emit must fail on the host stub"),
            crate::error::HookError::NotImplemented
        );
    }
}
