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
    use crate::types::{ACC_ID_LEN, AccountId, CurrencyCode, SField};
    use crate::xfl::XFL;

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

    /// Serialized type IDs (`sfcode >> 16`), hand-transcribed from
    /// rippled/xahaud's `SField.h` `STI_*` enumeration (protocol constants —
    /// no vendored source names them individually). [`txn_template!`]'s
    /// [`sti_of`] compares a declared field's actual serialized type against
    /// these, once per kind, to reject e.g. `u32_field(sfFee)` (an
    /// `STI_AMOUNT` field declared as if it were `STI_UINT32`) at compile
    /// time.
    pub mod sti {
        /// `UInt16` (e.g. `TransactionType`, `LedgerEntryType`).
        pub const STI_UINT16: u32 = 1;
        /// `UInt32` (e.g. `Flags`, `Sequence`).
        pub const STI_UINT32: u32 = 2;
        /// `UInt64` (e.g. `IndexNext`, `BaseFee`).
        pub const STI_UINT64: u32 = 3;
        /// `Hash128` (e.g. `EmailHash`).
        pub const STI_UINT128: u32 = 4;
        /// `Hash256` (e.g. `InvoiceID`).
        pub const STI_UINT256: u32 = 5;
        /// `Amount` (native or issued).
        pub const STI_AMOUNT: u32 = 6;
        /// `Blob`/variable-length (e.g. `SigningPubKey`).
        pub const STI_VL: u32 = 7;
        /// `AccountID`.
        pub const STI_ACCOUNT: u32 = 8;
        /// `STObject` (nested field list, closed by `0xE1`).
        pub const STI_OBJECT: u32 = 14;
        /// `STArray` (nested element list, closed by `0xF1`).
        pub const STI_ARRAY: u32 = 15;
        /// `UInt8` (e.g. `TransactionResult`).
        pub const STI_UINT8: u32 = 16;
        /// `Hash160` (e.g. `TakerPaysCurrency`).
        pub const STI_UINT160: u32 = 17;
        /// `PathSet`.
        pub const STI_PATHSET: u32 = 18;
        /// `Vector256` (e.g. `URITokenIDs`).
        pub const STI_VECTOR256: u32 = 19;
        /// `Hash192` (e.g. `MPTokenIssuanceID`).
        pub const STI_UINT192: u32 = 21;
        /// `Issue` (currency, or currency + issuer).
        pub const STI_ISSUE: u32 = 24;
        /// `Currency` (a bare 20-byte currency code, no issuer).
        pub const STI_CURRENCY: u32 = 26;
    }

    /// The serialized type ID (`code >> 16`) of `f` — the same value
    /// [`sti`]'s constants enumerate. Backs [`txn_template!`]'s per-field
    /// STI-agreement compile-time checks.
    #[must_use]
    pub const fn sti_of<T>(f: SField<T>) -> u32 {
        f.code().wrapping_shr(16)
    }

    /// Header + value size of an STI_UINT8 field (`txn_template!`'s
    /// `u8_field` kind).
    #[must_use]
    pub const fn u8_field_size<T>(f: SField<T>) -> usize {
        field_header(f).1.wrapping_add(1)
    }

    /// Header + value size of an STI_UINT16 field (`txn_template!`'s
    /// `u16_field` kind).
    #[must_use]
    pub const fn u16_field_size<T>(f: SField<T>) -> usize {
        field_header(f).1.wrapping_add(2)
    }

    /// Header + value size of an STI_UINT64 field (`txn_template!`'s
    /// `u64_field` kind).
    #[must_use]
    pub const fn u64_field_size<T>(f: SField<T>) -> usize {
        field_header(f).1.wrapping_add(8)
    }

    /// Header + `n`-byte value size of a fixed-width field (`txn_template!`'s
    /// `hash128`/`hash160`/`hash256`/`currency`/`native_issue`/`issue`
    /// kinds, each passing its own fixed `n`).
    #[must_use]
    pub const fn fixed_field_size<T>(f: SField<T>, n: usize) -> usize {
        field_header(f).1.wrapping_add(n)
    }

    /// Header + 48-byte value size of an STI_AMOUNT field encoded as an
    /// issued (IOU) amount (`txn_template!`'s `amount` kind).
    #[must_use]
    pub const fn iou_amount_field_size<T>(f: SField<T>) -> usize {
        field_header(f).1.wrapping_add(crate::types::IOU_AMOUNT_LEN)
    }

    /// Header-only size of a container field (`txn_template!`'s
    /// `object`/`array` kinds) — the inner fields and the closing end
    /// marker are sized separately, one declared field at a time.
    #[must_use]
    pub const fn container_header_size<T>(f: SField<T>) -> usize {
        field_header(f).1
    }

    /// Closing byte of a serialized `STObject` (`txn_template!`'s `object`
    /// kind).
    pub const OBJECT_END_MARKER: u8 = 0xE1;
    /// Closing byte of a serialized `STArray` (`txn_template!`'s `array`
    /// kind).
    pub const ARRAY_END_MARKER: u8 = 0xF1;

    /// Encodes `xfl`'s 8-byte issued-amount *value* region at compile time:
    /// the identical bit positions as `XFL`'s own layout, with bit 63 set
    /// (`STAmount`'s "not native" flag) — see [`txn_template!`]'s `amount`
    /// kind docs for the full bit-identity rationale. Canonical XFL zero
    /// (`0`) becomes `STAmount`'s canonical issued zero
    /// (`0x8000_0000_0000_0000`); every other canonical XFL's exponent and
    /// mantissa already occupy exactly `STAmount`'s fields, so the same `|`
    /// covers both.
    #[must_use]
    pub const fn encode_iou_amount_value_const(xfl: XFL) -> [u8; 8] {
        ((xfl.raw_bits() as u64) | 0x8000_0000_0000_0000).to_be_bytes()
    }

    /// Runtime counterpart to [`encode_iou_amount_value_const`]: writes
    /// `xfl`'s 8-byte issued-amount value into `out[..8]`.
    ///
    /// # Errors
    ///
    /// Returns [`HookError::InvalidArgument`] if `out` is shorter than 8
    /// bytes — there is no value-range failure (unlike
    /// [`encode_native_amount`]): every canonical `XFL`'s bit pattern is
    /// already a valid `STAmount` issued value.
    #[inline(always)]
    pub fn encode_iou_amount_value(out: &mut [u8], xfl: XFL) -> Result<()> {
        let dst = out.get_mut(0..8).ok_or(HookError::InvalidArgument)?;
        dst.copy_from_slice(&encode_iou_amount_value_const(xfl));
        Ok(())
    }

    /// Encodes a full 48-byte issued `Amount` at compile time: 8-byte value
    /// ([`encode_iou_amount_value_const`]), 20-byte currency, 20-byte
    /// issuer — [`crate::types::IouAmount`]'s layout. Used by
    /// `txn_template!`'s `amount` kind to bake in either its zero default
    /// or a declared `(xfl, currency, issuer)` triple.
    #[must_use]
    pub const fn encode_iou_amount_const(
        xfl: XFL,
        currency: &CurrencyCode,
        issuer: &AccountId,
    ) -> [u8; 48] {
        let mut out = [0u8; 48];
        write_const_bytes(&mut out, 0, &encode_iou_amount_value_const(xfl));
        write_const_bytes(&mut out, 8, &currency.0);
        write_const_bytes(&mut out, 28, &issuer.0);
        out
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

    /// Writes `count` back-to-back copies of `src` into `bytes` starting at
    /// `offset`, at compile time — [`write_const_bytes`] applied `count`
    /// times at successive `src.len()`-sized strides. Used by
    /// `txn_template!`'s homogeneous `array(sfX) [ Elem: object(sfY) { .. }
    /// ; N ]` kind to bake in `N` copies of the element's default
    /// (`Elem::TEMPLATE`) into the reserved array region.
    ///
    /// # Panics (compile-time only)
    ///
    /// See [`write_const_bytes`] — same const-context-only guarantee, per
    /// copy.
    pub const fn write_repeated<const N: usize>(
        bytes: &mut [u8; N],
        offset: usize,
        src: &[u8],
        count: usize,
    ) {
        let elem_len = src.len();
        let mut i = 0;
        while i < count {
            write_const_bytes(bytes, offset.wrapping_add(i.wrapping_mul(elem_len)), src);
            i = i.wrapping_add(1);
        }
    }

    /// The largest length rippled's three-byte VL length prefix can
    /// represent. [`vl_length_prefix`] panics (at compile time) past this;
    /// [`crate::sto_writer::StoWriter::vl`] (the runtime counterpart)
    /// checks it explicitly and returns
    /// [`HookError::InvalidArgument`](crate::error::HookError::InvalidArgument)
    /// instead, since a runtime panic would abort the whole hook.
    pub const MAX_VL_LEN: usize = 918_744;

    /// Computes rippled's variable-length (`VL`) size prefix for a blob of
    /// `len` bytes: `len <= 192` is a single byte (`len` itself);
    /// `193..=12480` is two bytes; `12481..=`[`MAX_VL_LEN`] is three.
    /// Returns the prefix bytes (only the first `N` of the 3 are
    /// meaningful) and `N`, mirroring [`field_header`]'s `([u8; 3],
    /// usize)` shape. Used by `txn_template!`'s `fixed_vl` kind — `len` is
    /// always the field's declared, compile-time-fixed length there — and
    /// by [`crate::sto_writer::StoWriter::vl`], the runtime counterpart.
    ///
    /// # Panics (compile-time only)
    ///
    /// Panics if `len` exceeds [`MAX_VL_LEN`] — only ever called from a
    /// `const` context; [`crate::sto_writer::StoWriter::vl`] checks this
    /// bound itself before calling in, so it never hits the panic at
    /// runtime.
    #[must_use]
    pub const fn vl_length_prefix(len: usize) -> ([u8; 3], usize) {
        if len <= 192 {
            ([len as u8, 0, 0], 1)
        } else if len <= 12480 {
            let adj = len.wrapping_sub(193);
            let byte0 = 193u8.wrapping_add((adj >> 8) as u8);
            let byte1 = (adj & 0xFF) as u8;
            ([byte0, byte1, 0], 2)
        } else if len <= MAX_VL_LEN {
            let adj = len.wrapping_sub(12481);
            let byte0 = 241u8.wrapping_add((adj >> 16) as u8);
            let byte1 = ((adj >> 8) & 0xFF) as u8;
            let byte2 = (adj & 0xFF) as u8;
            ([byte0, byte1, byte2], 3)
        } else {
            panic!(
                "txn_template!: fixed_vl length exceeds the maximum representable VL length (918744)"
            );
        }
    }

    /// Header + VL-prefix + value size of a `fixed_vl(sfX, n)` field:
    /// [`field_header`] plus [`vl_length_prefix`]'s prefix length plus `n`
    /// itself.
    #[must_use]
    pub const fn fixed_vl_field_size<T>(f: SField<T>, n: usize) -> usize {
        field_header(f)
            .1
            .wrapping_add(vl_length_prefix(n).1)
            .wrapping_add(n)
    }

    /// Writes a [`vl_length_prefix`] for `len` into `bytes` at `offset`, at
    /// compile time. Used by `txn_template!`'s generated `new()` to bake in
    /// a `fixed_vl` field's length prefix, the same way
    /// [`write_field_header`] bakes in a field header.
    ///
    /// # Panics (compile-time only)
    ///
    /// See [`write_field_header`] — same const-context-only guarantee.
    #[allow(clippy::indexing_slicing)] // in-bounds per the assert below; const-only, see the Panics note
    pub const fn write_vl_length_prefix<const N: usize>(
        bytes: &mut [u8; N],
        offset: usize,
        len: usize,
    ) {
        let (prefix, prefix_len) = vl_length_prefix(len);
        let mut i = 0;
        while i < prefix_len {
            let dst = offset.wrapping_add(i);
            assert!(
                dst < N,
                "txn_template!: fixed_vl length-prefix write out of bounds"
            );
            bytes[dst] = prefix[i];
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
    /// Kind tag for a `u8_field` table row.
    pub const KIND_U8_FIELD: u8 = 4;
    /// Kind tag for a `u16_field` table row.
    pub const KIND_U16_FIELD: u8 = 5;
    /// Kind tag for a `u64_field` table row.
    pub const KIND_U64_FIELD: u8 = 6;
    /// Kind tag for a `hash128` table row.
    pub const KIND_HASH128: u8 = 7;
    /// Kind tag for a `hash160` table row.
    pub const KIND_HASH160: u8 = 8;
    /// Kind tag for a `hash256` table row.
    pub const KIND_HASH256: u8 = 9;
    /// Kind tag for a `currency` table row.
    pub const KIND_CURRENCY: u8 = 10;
    /// Kind tag for an `amount` table row.
    pub const KIND_IOU_AMOUNT: u8 = 11;
    /// Kind tag for a `native_issue` table row.
    pub const KIND_NATIVE_ISSUE: u8 = 12;
    /// Kind tag for an `issue` table row.
    pub const KIND_ISSUE: u8 = 13;
    /// Kind tag for an `object` table row.
    pub const KIND_OBJECT: u8 = 14;
    /// Kind tag for an `array` table row.
    pub const KIND_ARRAY: u8 = 15;
    /// Kind tag for a `fixed_vl` table row.
    pub const KIND_FIXED_VL: u8 = 16;

    /// One row of a `txn_template!` field table: `(sfcode, kind tag,
    /// payload offset, depth)`. `payload offset` is the offset of the
    /// field's *value* (after the header, and after the VL length byte for
    /// `account_id`) — the same offset each kind's generated setter writes
    /// to. `depth` is `0` for a field declared directly on the template,
    /// and one more than its enclosing container's depth for a field
    /// declared inside a nested `object`/`array` — see [`find_field`].
    pub type FieldEntry = (u32, u8, usize, usize);

    /// Finds `sfcode` in `table`, at compile time, returning its
    /// `(kind tag, payload offset)` if present **at depth 0** — a row from
    /// inside a nested `object`/`array` never matches, even if its `sfcode`
    /// happens to equal one being looked up (e.g. an `sfAccount` nested in
    /// a `Signer` object must not satisfy the top-level `sfAccount`
    /// presence check, or be patched by `prepare_for_emit`). `table` is a
    /// template's generated `FIELDS` const; comparison is by the `sfcode`'s
    /// runtime *value*, so it is robust to how the constant was spelled at
    /// the declaration site (qualified path, alias, ...).
    #[must_use]
    #[allow(clippy::indexing_slicing)] // in-bounds per the `i < table.len()` guard; const-only, see the module doc
    pub const fn find_field(table: &[FieldEntry], sfcode: u32) -> Option<(u8, usize)> {
        let mut i = 0;
        while i < table.len() {
            let (code, kind, off, depth) = table[i];
            if code == sfcode && depth == 0 {
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

        /// Pins every `sti::STI_*` constant against a real generated
        /// `sfXxx` constant of that serialized type -- one field per
        /// constant, including `STI_PATHSET`/`STI_VECTOR256`/`STI_UINT192`,
        /// which back no `txn_template!` kind but still name real protocol
        /// type ids that a future kind (`docs/TXN_TEMPLATE_FIELDS_DESIGN.md`
        /// §6) would need to match.
        #[test]
        fn sti_constants_match_real_sfield_codes() {
            use crate::sfield::{
                sfAccount, sfAmount, sfAmountEntry, sfAmounts, sfBaseAsset, sfClaimCurrency,
                sfEmailHash, sfIndexNext, sfIndexes, sfInvoiceID, sfPaths, sfSequence,
                sfSigningPubKey, sfTakerPaysCurrency, sfTransactionResult, sfTransactionType,
            };

            assert_eq!(sti_of(sfTransactionType), sti::STI_UINT16);
            assert_eq!(sti_of(sfSequence), sti::STI_UINT32);
            assert_eq!(sti_of(sfIndexNext), sti::STI_UINT64);
            assert_eq!(sti_of(sfEmailHash), sti::STI_UINT128);
            assert_eq!(sti_of(sfInvoiceID), sti::STI_UINT256);
            assert_eq!(sti_of(sfAmount), sti::STI_AMOUNT);
            assert_eq!(sti_of(sfSigningPubKey), sti::STI_VL);
            assert_eq!(sti_of(sfAccount), sti::STI_ACCOUNT);
            assert_eq!(sti_of(sfAmountEntry), sti::STI_OBJECT);
            assert_eq!(sti_of(sfAmounts), sti::STI_ARRAY);
            assert_eq!(sti_of(sfTransactionResult), sti::STI_UINT8);
            assert_eq!(sti_of(sfTakerPaysCurrency), sti::STI_UINT160);
            assert_eq!(sti_of(sfPaths), sti::STI_PATHSET);
            assert_eq!(sti_of(sfIndexes), sti::STI_VECTOR256);
            assert_eq!(sti_of(sfClaimCurrency), sti::STI_ISSUE);
            assert_eq!(sti_of(sfBaseAsset), sti::STI_CURRENCY);

            #[cfg(feature = "all-amendments")]
            assert_eq!(sti_of(crate::sfield::sfMPTokenIssuanceID), sti::STI_UINT192);
        }

        #[test]
        fn vl_length_prefix_one_byte_form() {
            assert_eq!(vl_length_prefix(0), ([0, 0, 0], 1));
            assert_eq!(vl_length_prefix(1), ([1, 0, 0], 1));
            assert_eq!(vl_length_prefix(192), ([192, 0, 0], 1));
        }

        #[test]
        fn vl_length_prefix_two_byte_boundary() {
            // 193 is the smallest length needing a two-byte prefix:
            // adj = 193 - 193 = 0, so [193 + 0, 0].
            assert_eq!(vl_length_prefix(193), ([193, 0, 0], 2));
        }

        #[test]
        fn vl_length_prefix_two_byte_form() {
            // 200: adj = 200 - 193 = 7, so [193 + (7 >> 8), 7 & 0xFF] = [193, 7].
            assert_eq!(vl_length_prefix(200), ([193, 7, 0], 2));
            // 12480 (the largest two-byte length): adj = 12480 - 193 = 12287
            // (0x2FFF), so [193 + (0x2FFF >> 8), 0x2FFF & 0xFF] = [193 + 0x2F, 0xFF]
            // = [240, 255].
            assert_eq!(vl_length_prefix(12480), ([240, 255, 0], 2));
        }

        #[test]
        fn vl_length_prefix_three_byte_boundary() {
            // 12481 is the smallest length needing a three-byte prefix:
            // adj = 12481 - 12481 = 0, so [241 + 0, 0, 0].
            assert_eq!(vl_length_prefix(12481), ([241, 0, 0], 3));
        }

        #[test]
        fn vl_length_prefix_three_byte_form_and_maximum() {
            // 918744 (the largest representable length): adj = 918744 -
            // 12481 = 906263 (0x0D_D417), so
            // [241 + (0x0D_D417 >> 16), (0x0D_D417 >> 8) & 0xFF, 0x0D_D417 & 0xFF]
            // = [241 + 0x0D, 0xD4, 0x17] = [254, 212, 23].
            assert_eq!(vl_length_prefix(918_744), ([254, 212, 23], 3));
        }

        #[test]
        #[should_panic(expected = "exceeds the maximum representable VL length")]
        fn vl_length_prefix_rejects_too_large() {
            let _ = vl_length_prefix(918_745);
        }

        #[test]
        fn fixed_vl_field_size_sums_header_prefix_and_payload() {
            // sfSigningPubKey (7,3): single-byte header; 4-byte payload
            // needs a single-byte VL prefix too.
            assert_eq!(
                fixed_vl_field_size(SField::<crate::types::Opaque>::new((7 << 16) + 3), 4),
                1 + 1 + 4
            );
        }

        #[test]
        fn write_vl_length_prefix_writes_at_offset() {
            let mut buf = [0u8; 4];
            write_vl_length_prefix(&mut buf, 1, 200);
            assert_eq!(buf, [0, 193, 7, 0]);
        }

        #[test]
        #[should_panic(expected = "out of bounds")]
        fn write_vl_length_prefix_rejects_out_of_bounds() {
            let mut buf = [0u8; 1];
            write_vl_length_prefix(&mut buf, 0, 200);
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
        fn encode_iou_amount_value_rejects_out_of_bounds() {
            let mut out = [0u8; 7];
            assert_eq!(
                encode_iou_amount_value(&mut out, XFL::from_raw_bits(0)),
                Err(HookError::InvalidArgument)
            );
        }

        #[test]
        fn encode_iou_amount_value_writes_the_expected_bytes() {
            let mut out = [0u8; 8];
            encode_iou_amount_value(&mut out, XFL::from_raw_bits(0)).expect("8-byte buffer fits");
            assert_eq!(out, [0x80, 0, 0, 0, 0, 0, 0, 0]);
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

        const SAMPLE_TABLE: &[FieldEntry] = &[
            (100, KIND_U32_FIELD, 4, 0),
            (200, KIND_NATIVE_AMOUNT, 12, 0),
        ];

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
        fn find_field_ignores_a_nested_row() {
            // A depth-1 row (as a scalar field declared inside an
            // `object`/`array` produces) must be invisible to `find_field`
            // even when its `sfcode` matches — this is what keeps a nested
            // `sfAccount` from satisfying `txn_template!`'s top-level
            // required-field presence check.
            const NESTED_TABLE: &[FieldEntry] = &[(100, KIND_U32_FIELD, 4, 1)];
            assert_eq!(find_field(NESTED_TABLE, 100), None);
            assert!(!field_present(NESTED_TABLE, 100));
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
///         field_name: <kind>,                        // any count/order after this
///         field_name: object(sfXxx) { <field>* },     // nested STObject
///         field_name: array(sfXxx) [ <element>* ],    // nested STArray
///         field_name: emit_details,                   // must be LAST, top level only
///     }
/// }
///
/// <element> := field_name: object(sfXxx) { <field>* }  // objects only, directly in an array
/// ```
///
/// Every scalar field uses one of the uniform kinds in the table below —
/// there is no separate "role" syntax. The macro recognizes the handful of
/// fields an emitted transaction always needs (see "Required fields"
/// below) **by their `sfXxx` code value**, not by which keyword declared
/// them, and only at the top level (a field with the same `sfcode` nested
/// inside an `object`/`array` does not count — see "Nested containers"
/// below).
///
/// | kind | STI | wire bytes | default | setter |
/// |---|---|---|---|---|
/// | `u8_field(sfX) = e` | UINT8 | 1 | required | `set_x(u8)` |
/// | `u16_field(sfX) = e` | UINT16 | 2 | required | `set_x(u16)` |
/// | `u32_field(sfX) = e` | UINT32 | 4 | required | `set_x(u32)` |
/// | `u64_field(sfX) = e` | UINT64 | 8 | required | `set_x(u64)` |
/// | `hash128(sfX)` | UINT128 | 16 | zeroed | `set_x(&[u8; 16])` |
/// | `hash160(sfX)` | UINT160 | 20 | zeroed | `set_x(&[u8; 20])` |
/// | `hash256(sfX)` | UINT256 | 32 | zeroed | `set_x(&Hash)` |
/// | `currency(sfX)` | CURRENCY | 20 | zeroed | `set_x(&CurrencyCode)` |
/// | `native_amount(sfX) = e` | AMOUNT | 8 | required drops | `set_x(u64) -> Result<()>` |
/// | `amount(sfX)` | AMOUNT | 48 | IOU zero, zero currency/issuer | `set_x(XFL, &CurrencyCode, &AccountId)`, `set_x_value(XFL)` |
/// | `amount(sfX) = (xfl, cur, iss)` | AMOUNT | 48 | the declared triple | same as above |
/// | `native_issue(sfX)` | ISSUE | 20 | zeroed | none |
/// | `issue(sfX)` | ISSUE | 40 | zeroed | `set_x(&CurrencyCode, &AccountId)` |
/// | `account_id(sfX)` | ACCOUNT | 1 + 20 | zeroed | `set_x(&AccountId)` |
/// | `empty_vl(sfX)` | VL | 1 | empty blob | none |
/// | `fixed_vl(sfX, N) = e` | VL | VL-prefix(N) + N | zeroed, or the declared `[u8; N]` | `set_x(&[u8; N])` |
/// | `object(sfX) { .. }` | OBJECT | inner + 1 (`0xE1`) | inner defaults | inner setters, prefixed |
/// | `array(sfX) [ .. ]` | ARRAY | elements + 1 (`0xF1`) | inner defaults | inner setters, prefixed |
///
/// Every kind checks, at compile time, that the declared `sfXxx` constant's
/// serialized type (`code >> 16`) matches — `u32_field(sfFee)` (an
/// STI_AMOUNT field) is rejected rather than silently encoding the wrong
/// wire representation. Integer kinds are big-endian.
///
/// `amount`'s 48-byte value region is `[8-byte value][20-byte
/// currency][20-byte issuer]`. The 8-byte value is a pure bit transform of
/// the XFL (no host call, at compile time or at runtime): canonical XFL
/// zero becomes `STAmount`'s canonical issued zero
/// (`0x8000_0000_0000_0000`), and every other canonical XFL's exponent and
/// mantissa already occupy the identical bit positions `STAmount` uses, so
/// setting bit 63 (`STAmount`'s "not native" flag) is the whole transform.
/// `set_x_value` writes only those 8 bytes, keeping the baked or
/// previously set currency/issuer — the intended hot path when a default
/// triple bakes in the currency/issuer once.
///
/// `fixed_vl(sfX, N)` is a fixed-length variable-length (`VL`) blob: `N`
/// (a `usize` const expression, at least 1) is part of the declaration, so
/// the wire's length prefix — [`crate::txn::codec::vl_length_prefix`]'s
/// one-, two-, or three-byte rippled encoding, chosen by `N`'s own
/// magnitude — is computed and baked in at compile time, the same way
/// every other kind's header is. Declaring `N = 0` is a compile error —
/// `empty_vl` is the one spelling for an empty blob, so
/// `sfSigningPubKey`'s required-kind check still only accepts `empty_vl`,
/// not `fixed_vl(sfSigningPubKey, 0)`. A declared default (`= [u8; N]`
/// expr) must be exactly that array type — a wrong-length default is a
/// compile-time type error, not a truncation or a panic. Only fixed-length
/// `VL` is covered; `Vector256`/`PathSet` and a genuinely variable-length
/// blob stay out of scope (`docs/TXN_TEMPLATE_FIELDS_DESIGN.md` §6).
///
/// `emit_details` reserves
/// [`EMIT_DETAILS_MAX_LEN`](crate::types::EMIT_DETAILS_MAX_LEN) zeroed
/// bytes with **no header** (the host's `etxn_details` writes its own,
/// complete field). Has no `sfcode` (it is a structural marker, not an
/// STObject field), so it is tracked separately from the value-based
/// detection below. Must be the last declared field, at the top level;
/// declaring anything after it, or inside a nested `object`/`array`, is a
/// macro-parse compile error.
///
/// ## Nested containers
///
/// `object(sfX) { <field>* }` nests a fixed inner field list — its field
/// count and shape are known at declaration time, so the whole template
/// stays `const fn`-computable exactly like the scalar kinds. An
/// `array(sfX) [ .. ]` field takes a named-element form or a homogeneous
/// indexed form:
///
/// - **Named elements**: `array(sfX) [ name: object(sfY) { <field>* },
///   name2: object(sfY) { <field>* }, .. ]` — each element declared
///   individually (so heterogeneous element shapes, one native and one
///   issued entry say, fall out naturally), reached through its own
///   `_`-joined setter path (below).
/// - **Homogeneous, indexed elements**: `array(sfX) [ Elem: object(sfY) {
///   <field>* } ; N ]` — exactly one element shape, declared once and
///   repeated `N` times (`N` a `usize` const expression, at least 1); see
///   "Homogeneous arrays" below.
///
/// Either way, an array's elements must each be an `object(sfY) { .. }` —
/// a scalar or a nested `array` directly inside an `array` is a compile
/// error, since a bare value or an unbounded nesting has no fixed element
/// shape.
///
/// Canonical `(type, field)` order is checked **per container**, not just
/// at the top level: each object's own direct fields (and the template's
/// own top-level fields) must have strictly increasing `sfXxx` codes. An
/// array's elements are not order-checked against each other (named
/// elements typically share one repeated `sfcode` anyway, e.g. every
/// `sfAmounts` element is an `sfAmountEntry`). Nesting depth is bounded at
/// compile time by [`crate::sto_writer::STO_WRITER_MAX_DEPTH`], the same
/// limit xahaud's deserializer enforces — a homogeneous array's element
/// counts as **two** levels (the array itself, then the element), the same
/// as a named array's object element.
///
/// Setter names for a *named* nested field are the full `_`-joined
/// declaration path: `amounts: array(sfAmounts) [ usd: object(sfAmountEntry)
/// { amount: amount(sfAmount) = .. } ]` generates `set_amounts_usd_amount`/
/// `set_amounts_usd_amount_value` — an array element's own name is only a
/// path segment, not a repetition index.
///
/// ## Homogeneous arrays
///
/// `amounts: array(sfAmounts) [ AmountEntry: object(sfAmountEntry) {
/// amount: amount(sfAmount) = .. } ; 3 ]` generates two things instead of a
/// setter:
///
/// - A standalone element-view type named after `Elem` (`AmountEntry`
///   here): `pub struct AmountEntry<'a> { .. }`, wrapping a `&'a mut [u8]`
///   slice of exactly `AmountEntry::LEN` bytes (that element's header,
///   inner fields, and closing `0xE1`), with an `AmountEntry::TEMPLATE:
///   [u8; AmountEntry::LEN]` baked default and the *same* inner setters
///   (`set_amount`/`set_amount_value`, ...) a `txn_template!` struct itself
///   would generate for the same field list, writing directly into the
///   view's slice. There is no owned `AmountEntry::new()` — a view only
///   ever comes from the parent's accessor below.
/// - On the parent, a **runtime-indexed accessor** (named by the field
///   path, with no `set_` prefix): `fn amounts(&mut self, index: usize) ->
///   Option<AmountEntry<'_>>`, `None` if `index >= N`, `Some` of a view
///   over that element's `N`-repeated slot otherwise (`self.bytes.get_mut(..)`,
///   no unsafe, no raw indexing).
///
/// A view over `&mut [u8]` rather than a direct `txn.amounts[n]` index
/// expression is deliberate: the generated types stay ordinary safe Rust
/// (no `#[repr(C)]`/transmute over the byte buffer), and the workspace's
/// `indexing_slicing` lint (`docs/DESIGN.md` §8) would make a raw `[n]`
/// panic-on-out-of-range unusable inside a hook anyway — the `Option`
/// return makes the out-of-range case an ordinary, checked branch instead.
/// A homogeneous array may itself contain further named or homogeneous
/// nested containers, at whatever depth the `STO_WRITER_MAX_DEPTH` bound
/// above allows.
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
/// A top-level field declared `flags: u32_field(sfFlags) = 0` gets a
/// method `fn set_flags(&mut self, value: u32)`, synthesized via
/// `$crate::__paste!`'s `[<set_ $field>]` splice — one per scalar kind that
/// has a setter (see the kind table above; `empty_vl` and `native_issue`
/// get none), including the required ones (`set_sequence`, the two
/// ledger-sequence setters, `set_fee`, `set_account` all exist; see the
/// overwrite note above for why setting them is rarely useful once
/// `prepare_for_emit` is in the picture). A field nested inside an
/// `object`/named `array` gets the `_`-joined path form instead (see
/// "Nested containers" above); a homogeneous array field instead gets a
/// runtime-indexed accessor with no `set_` prefix (see "Homogeneous
/// arrays" above), since it returns a view rather than writing a value
/// directly.
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
/// [`HookStatic<T: Clone>`](crate::static_cell::HookStatic)), one setter
/// per field that has one (see the kind table above), `emit_details_region()`,
/// `bytes()`, a `Default` impl equivalent to `new()`, an `impl`
/// [`TemplateBytes`](crate::txn::TemplateBytes) forwarding to `bytes()` (so
/// [`Prepared`](crate::txn::Prepared) can name the type generically), and
/// `prepare_for_emit()` (see above) — the last three are unconditional
/// because the required fields, including `emit_details`, are mandatory.
///
/// # Compile-time canonical-order check
///
/// Declared fields' `sfXxx` codes must be strictly increasing (canonical
/// `(type, field)` order, since `sfcode = (type << 16) | field` and `field`
/// is always 16 bits), checked independently per container (see "Nested
/// containers" above) — a compile error otherwise. This also catches a
/// duplicated field within one container (two entries with the same
/// `sfcode` violate *strictly* increasing order). `emit_details` has no
/// `sfcode` and is exempt.
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
            order = [$crate::sfield::sfTransactionType.code(),],
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
            prefix = [],
            ctx = obj,
            depth = [0usize],
            stack = [],
            mode = tpl,
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
/// [`crate::txn::codec`]'s `const fn`s, never separately recomputed. One
/// arm exists per scalar kind (plus `ctx = arr` variants where a kind is
/// only legal inside/outside an array); each emits its own STI-agreement
/// `const _: () = assert!(...)` before recursing.
///
/// Extra accumulators back nesting and value-based required-field
/// detection:
///
/// - `table` accumulates one `(($sfcode).code(), kind tag, payload offset,
///   depth)` tuple literal per scalar or container field, becoming the
///   generated `$Name::FIELDS` const array — see
///   [`crate::txn::codec::FieldEntry`]. `emit_details` has no `sfcode`, so
///   it contributes no row.
/// - `emit_details` holds `[presence flag, offset]` — `[false, 0usize]`
///   until an `emit_details` field is declared, `[true, (offset_expr)]`
///   after (structurally guaranteed to happen at most once: the
///   `emit_details` field must be last, so a second one would leave
///   unconsumed tokens and fail to parse before ever reaching this
///   accumulator).
/// - `prefix`/`ctx`/`depth`/`stack` back nested `object`/named-array
///   fields. `object(sfX) { .. }`/`array(sfX) [ .. ]` flatten their inner
///   field list into the *same* linear `fields` stream, followed by an
///   `@end_object`/`@end_array` continuation marker — pushing the current
///   `prefix`/`order`/`ctx` onto `stack`, resetting `order` to `[]` and
///   `ctx` to what the container accepts (`arr` only accepts `object`
///   elements), and incrementing `depth` (with its own compile-time `<
///   STO_WRITER_MAX_DEPTH` assert). The `@end_*` arms write the
///   container's closing byte, pop `stack` to restore the parent
///   `prefix`/`order`/`ctx`, decrement `depth`, and — for `@end_object`
///   only, since array elements are not order-checked — emit that
///   container's own strictly-increasing-order `const _` check over the
///   `order` list just closed. A field's `prefix` is spliced into its
///   setter name as literal `ident`/`_` token pairs (`$name _`), which
///   [`crate::__paste!`] concatenates alongside `set_`/the field name — see
///   [`txn_template!`](crate::txn_template)'s "Setter names" section.
/// - `mode` distinguishes a template's own recursion (`tpl`) from an
///   element-view type's (`elem`); every arm but the two base cases just
///   threads it through unchanged. A homogeneous `array(sfX) [ Elem:
///   object(sfY) { .. } ; N ]` field's arm spawns a **wholly separate**
///   `$crate::__txn_template_step!` invocation, seeded fresh (`name =
///   $Elem`, `order = []`, `prefix = []`, `ctx = obj`, a single `stack`
///   frame, `mode = elem`, `fields = [ ..inner.., @end_object ]`) so the
///   same `@end_object` arm above closes it and checks its order — the
///   `elem`-mode base case then emits only `$Elem`'s standalone view type
///   (`LEN`, `TEMPLATE`, the inner setters, `bytes()`; see
///   [`txn_template!`](crate::txn_template)'s "Homogeneous arrays"
///   section), never the plumbing/presence/`prepare_for_emit` items the
///   `tpl`-mode base case emits. The parent's *own* recursion continues
///   alongside, unaffected, referencing `$Elem` by name for its
///   `Option<$Elem<'_>>` accessor.
///
/// There is one base case per `mode` (`fields = []`, `mode = tpl` or `mode
/// = elem`): every field kind's table row is uniform, so a `tpl`-mode
/// `prepare_for_emit()`/`$Name::FIELDS` is always generated for a
/// template, and an `elem`-mode view type is always generated for an
/// element. A duplicated field within one container is caught by that
/// container's canonical-order assert, since two equal `sfcode`s violate
/// strictly-increasing order. Whether the crate actually compiles comes
/// down to independent `const _: () = assert!(...)` items: one STI check
/// per declared field, one order check per container, one depth check per
/// nested container (two for a homogeneous array's element, matching a
/// named array's object element), one element-count check per homogeneous
/// array, plus the fixed set of required-field checks generated in a
/// `tpl`-mode base case — a presence check and a kind-agreement check per
/// required field (via [`crate::txn::codec::field_present`] /
/// [`crate::txn::codec::field_kind_ok`] over `$Name::FIELDS`, at const-eval
/// time, which only ever match a depth-0 row), plus a presence check for
/// `emit_details` (sourced from its own accumulator, since it isn't in the
/// table). Each is a separate `const` item — a single `const`'s
/// initializer panics at its first failing statement, so grouping them
/// would only ever surface one error; separate items let rustc evaluate
/// and report every independent problem.
#[doc(hidden)]
#[macro_export]
macro_rules! __txn_template_step {
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = $ctx:tt, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ , $($rest:tt)* ]
    ) => {
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)*],
            setters = [$($setters)*],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [$($init)*],
            prev = [$($prev)*],
            table = [$($table)*],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = $ctx,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($rest)* ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : u8_field($sfcode:expr) = $default:expr $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_UINT8,
            concat!("txn_template!: `", stringify!($field), "` is declared as `u8_field` but its sfXxx code is not an STI_UINT8 field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "` (default `", stringify!($default), "`). Overwritten by `prepare_for_emit` if this is one of the required emit-plumbing fields.")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction: `Self::LEN` sums these same field sizes
                $vis fn [<set_ $($prefix)* $field>](&mut self, value: u8) {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    self.bytes[OFF] = value;
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
                    &[($default) as u8],
                );
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::u8_field_size($sfcode)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_U8_FIELD, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : u16_field($sfcode:expr) = $default:expr $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_UINT16,
            concat!("txn_template!: `", stringify!($field), "` is declared as `u16_field` but its sfXxx code is not an STI_UINT16 field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "` (default `", stringify!($default), "`). Overwritten by `prepare_for_emit` if this is one of the required emit-plumbing fields.")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction: `Self::LEN` sums these same field sizes
                $vis fn [<set_ $($prefix)* $field>](&mut self, value: u16) {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    self.bytes[OFF..OFF.wrapping_add(2)].copy_from_slice(&value.to_be_bytes());
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
                    &(($default) as u16).to_be_bytes(),
                );
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::u16_field_size($sfcode)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_U16_FIELD, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : u32_field($sfcode:expr) = $default:expr $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_UINT32,
            concat!("txn_template!: `", stringify!($field), "` is declared as `u32_field` but its sfXxx code is not an STI_UINT32 field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "` (default `", stringify!($default), "`). Overwritten by `prepare_for_emit` if this is one of the required emit-plumbing fields.")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction: `Self::LEN` sums these same field sizes
                $vis fn [<set_ $($prefix)* $field>](&mut self, value: u32) {
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
                (($sfcode).code(), $crate::txn::codec::KIND_U32_FIELD, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : u64_field($sfcode:expr) = $default:expr $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_UINT64,
            concat!("txn_template!: `", stringify!($field), "` is declared as `u64_field` but its sfXxx code is not an STI_UINT64 field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "` (default `", stringify!($default), "`). Overwritten by `prepare_for_emit` if this is one of the required emit-plumbing fields.")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction: `Self::LEN` sums these same field sizes
                $vis fn [<set_ $($prefix)* $field>](&mut self, value: u64) {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    self.bytes[OFF..OFF.wrapping_add(8)].copy_from_slice(&value.to_be_bytes());
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
                    &(($default) as u64).to_be_bytes(),
                );
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::u64_field_size($sfcode)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_U64_FIELD, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : native_amount($sfcode:expr) = $default:expr $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_AMOUNT,
            concat!("txn_template!: `", stringify!($field), "` is declared as `native_amount` but its sfXxx code is not an STI_AMOUNT field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
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
                $vis fn [<set_ $($prefix)* $field>](&mut self, drops: u64) -> $crate::error::Result<()> {
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
                (($sfcode).code(), $crate::txn::codec::KIND_NATIVE_AMOUNT, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : account_id($sfcode:expr) $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_ACCOUNT,
            concat!("txn_template!: `", stringify!($field), "` is declared as `account_id` but its sfXxx code is not an STI_ACCOUNT field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "` (defaults to the all-zero `AccountId`). Overwritten by `prepare_for_emit` if this is the required `sfAccount` field.")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction, as above
                $vis fn [<set_ $($prefix)* $field>](&mut self, value: &$crate::types::AccountId) {
                    const OFF: usize = (($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1)).wrapping_add(1);
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
                (($sfcode).code(), $crate::txn::codec::KIND_ACCOUNT_ID, (($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1)).wrapping_add(1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : empty_vl($sfcode:expr) $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_VL,
            concat!("txn_template!: `", stringify!($field), "` is declared as `empty_vl` but its sfXxx code is not an STI_VL field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

            ],
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
                (($sfcode).code(), $crate::txn::codec::KIND_EMPTY_VL, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : hash128($sfcode:expr) $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_UINT128,
            concat!("txn_template!: `", stringify!($field), "` is declared as `hash128` but its sfXxx code is not an STI_UINT128 field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "` (defaults to all-zero bytes).")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction, as above
                $vis fn [<set_ $($prefix)* $field>](&mut self, value: &[u8; 16]) {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    self.bytes[OFF..OFF.wrapping_add(16)].copy_from_slice(value);
                }
            ],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::fixed_field_size($sfcode, 16usize)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_HASH128, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : hash160($sfcode:expr) $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_UINT160,
            concat!("txn_template!: `", stringify!($field), "` is declared as `hash160` but its sfXxx code is not an STI_UINT160 field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "` (defaults to all-zero bytes).")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction, as above
                $vis fn [<set_ $($prefix)* $field>](&mut self, value: &[u8; 20]) {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    self.bytes[OFF..OFF.wrapping_add(20)].copy_from_slice(value);
                }
            ],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::fixed_field_size($sfcode, 20usize)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_HASH160, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : hash256($sfcode:expr) $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_UINT256,
            concat!("txn_template!: `", stringify!($field), "` is declared as `hash256` but its sfXxx code is not an STI_UINT256 field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "` (defaults to all-zero bytes).")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction, as above
                $vis fn [<set_ $($prefix)* $field>](&mut self, value: &$crate::types::Hash) {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    self.bytes[OFF..OFF.wrapping_add(32)].copy_from_slice(value.as_ref());
                }
            ],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::fixed_field_size($sfcode, 32usize)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_HASH256, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : currency($sfcode:expr) $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_CURRENCY,
            concat!("txn_template!: `", stringify!($field), "` is declared as `currency` but its sfXxx code is not an STI_CURRENCY field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "` (defaults to all-zero bytes).")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction, as above
                $vis fn [<set_ $($prefix)* $field>](&mut self, value: &$crate::types::CurrencyCode) {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    self.bytes[OFF..OFF.wrapping_add(20)].copy_from_slice(value.as_ref());
                }
            ],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::fixed_field_size($sfcode, 20usize)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_CURRENCY, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : amount($sfcode:expr) $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_AMOUNT,
            concat!("txn_template!: `", stringify!($field), "` is declared as `amount` but its sfXxx code is not an STI_AMOUNT field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "` to the 48-byte issued (IOU) form of `xfl`/`currency`/`issuer`.")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction, as above
                $vis fn [<set_ $($prefix)* $field>](&mut self, xfl: $crate::xfl::XFL, currency: &$crate::types::CurrencyCode, issuer: &$crate::types::AccountId) {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    self.bytes[OFF..OFF.wrapping_add($crate::types::IOU_AMOUNT_LEN)].copy_from_slice(&$crate::txn::codec::encode_iou_amount_const(xfl, currency, issuer));
                }

                #[doc = concat!("Sets only `", stringify!($field), "`'s 8-byte value region to `xfl`, keeping its currency/issuer unchanged.")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction, as above
                $vis fn [<set_ $($prefix)* $field _value>](&mut self, xfl: $crate::xfl::XFL) {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    self.bytes[OFF..OFF.wrapping_add(8)].copy_from_slice(&$crate::txn::codec::encode_iou_amount_value_const(xfl));
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
                    &$crate::txn::codec::encode_iou_amount_const($crate::xfl::XFL::from_raw_bits(0), &$crate::types::CurrencyCode::zeroed(), &$crate::types::AccountId::zeroed()),
                );
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::iou_amount_field_size($sfcode)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_IOU_AMOUNT, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : amount($sfcode:expr) = ($xfl:expr, $currency:expr, $issuer:expr) $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_AMOUNT,
            concat!("txn_template!: `", stringify!($field), "` is declared as `amount` but its sfXxx code is not an STI_AMOUNT field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "` to the 48-byte issued (IOU) form of `xfl`/`currency`/`issuer`.")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction, as above
                $vis fn [<set_ $($prefix)* $field>](&mut self, xfl: $crate::xfl::XFL, currency: &$crate::types::CurrencyCode, issuer: &$crate::types::AccountId) {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    self.bytes[OFF..OFF.wrapping_add($crate::types::IOU_AMOUNT_LEN)].copy_from_slice(&$crate::txn::codec::encode_iou_amount_const(xfl, currency, issuer));
                }

                #[doc = concat!("Sets only `", stringify!($field), "`'s 8-byte value region to `xfl`, keeping its currency/issuer unchanged.")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction, as above
                $vis fn [<set_ $($prefix)* $field _value>](&mut self, xfl: $crate::xfl::XFL) {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    self.bytes[OFF..OFF.wrapping_add(8)].copy_from_slice(&$crate::txn::codec::encode_iou_amount_value_const(xfl));
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
                    &$crate::txn::codec::encode_iou_amount_const(($xfl), &($currency), &($issuer)),
                );
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::iou_amount_field_size($sfcode)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_IOU_AMOUNT, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : native_issue($sfcode:expr) $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_ISSUE,
            concat!("txn_template!: `", stringify!($field), "` is declared as `native_issue` but its sfXxx code is not an STI_ISSUE field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

            ],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::fixed_field_size($sfcode, 20usize)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_NATIVE_ISSUE, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : issue($sfcode:expr) $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_ISSUE,
            concat!("txn_template!: `", stringify!($field), "` is declared as `issue` but its sfXxx code is not an STI_ISSUE field")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "`'s currency and issuer (defaults to all-zero bytes).")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction, as above
                $vis fn [<set_ $($prefix)* $field>](&mut self, currency: &$crate::types::CurrencyCode, issuer: &$crate::types::AccountId) {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    self.bytes[OFF..OFF.wrapping_add($crate::types::CURRENCY_CODE_LEN)].copy_from_slice(currency.as_ref());
                    self.bytes[OFF.wrapping_add($crate::types::CURRENCY_CODE_LEN)..OFF.wrapping_add($crate::types::CURRENCY_CODE_LEN).wrapping_add($crate::types::ACC_ID_LEN)].copy_from_slice(issuer.as_ref());
                }
            ],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::fixed_field_size($sfcode, 40usize)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_ISSUE, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : fixed_vl($sfcode:expr, $n:expr) $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_VL,
            concat!("txn_template!: `", stringify!($field), "` is declared as `fixed_vl` but its sfXxx code is not an STI_VL field")
        );
        const _: () = assert!(
            ($n) >= 1usize,
            concat!("txn_template!: `", stringify!($field), "`'s fixed_vl length must be at least 1 -- declare it as `empty_vl` for an empty blob")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "`'s ", stringify!($n), "-byte payload.")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction, as above
                $vis fn [<set_ $($prefix)* $field>](&mut self, value: &[u8; ($n)]) {
                    const OFF: usize = ($($prev)*)
                        .wrapping_add($crate::txn::codec::field_header($sfcode).1)
                        .wrapping_add($crate::txn::codec::vl_length_prefix($n).1);
                    self.bytes[OFF..OFF.wrapping_add($n)].copy_from_slice(value);
                }
            ],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
                $crate::txn::codec::write_vl_length_prefix(
                    &mut $($buf)*,
                    ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1),
                    $n,
                );
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::fixed_vl_field_size($sfcode, $n)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_FIXED_VL, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1).wrapping_add($crate::txn::codec::vl_length_prefix($n).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $field:ident : fixed_vl($sfcode:expr, $n:expr) = $default:expr $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_VL,
            concat!("txn_template!: `", stringify!($field), "` is declared as `fixed_vl` but its sfXxx code is not an STI_VL field")
        );
        const _: () = assert!(
            ($n) >= 1usize,
            concat!("txn_template!: `", stringify!($field), "`'s fixed_vl length must be at least 1 -- declare it as `empty_vl` for an empty blob")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Sets `", stringify!($field), "`'s ", stringify!($n), "-byte payload (default `", stringify!($default), "`).")]
                #[inline(always)]
                #[allow(clippy::indexing_slicing)] // in-bounds by construction, as above
                $vis fn [<set_ $($prefix)* $field>](&mut self, value: &[u8; ($n)]) {
                    const OFF: usize = ($($prev)*)
                        .wrapping_add($crate::txn::codec::field_header($sfcode).1)
                        .wrapping_add($crate::txn::codec::vl_length_prefix($n).1);
                    self.bytes[OFF..OFF.wrapping_add($n)].copy_from_slice(value);
                }
            ],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
                $crate::txn::codec::write_vl_length_prefix(
                    &mut $($buf)*,
                    ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1),
                    $n,
                );
                {
                    let __d: [u8; ($n)] = $default;
                    $crate::txn::codec::write_const_bytes(
                        &mut $($buf)*,
                        ($($prev)*)
                            .wrapping_add($crate::txn::codec::field_header($sfcode).1)
                            .wrapping_add($crate::txn::codec::vl_length_prefix($n).1),
                        &__d,
                    );
                }
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::fixed_vl_field_size($sfcode, $n)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_FIXED_VL, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1).wrapping_add($crate::txn::codec::vl_length_prefix($n).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $name:ident : object($sfcode:expr) { $($inner:tt)* } $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_OBJECT,
            concat!("txn_template!: `", stringify!($name), "` is declared as `object` but its sfXxx code is not an STI_OBJECT field")
        );
        const _: () = assert!(
            (($($depth)*).wrapping_add(1usize)) < $crate::sto_writer::STO_WRITER_MAX_DEPTH,
            concat!("txn_template!: `", stringify!($name), "` would nest deeper than STO_WRITER_MAX_DEPTH")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [],
            setters = [$($setters)*],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::container_header_size($sfcode)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_OBJECT, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)* $name _],
            ctx = obj,
            depth = [ (($($depth)*).wrapping_add(1usize)) ],
            stack = [ [ [$($prefix)*] [$($order)* ($sfcode).code(),] obj ] $($stack)* ],
            mode = $mode,
            fields = [ $($inner)* , @ end_object $(, $($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = arr, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $name:ident : object($sfcode:expr) { $($inner:tt)* } $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_OBJECT,
            concat!("txn_template!: `", stringify!($name), "` is declared as `object` but its sfXxx code is not an STI_OBJECT field")
        );
        const _: () = assert!(
            (($($depth)*).wrapping_add(1usize)) < $crate::sto_writer::STO_WRITER_MAX_DEPTH,
            concat!("txn_template!: `", stringify!($name), "` would nest deeper than STO_WRITER_MAX_DEPTH")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [],
            setters = [$($setters)*],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::container_header_size($sfcode)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_OBJECT, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)* $name _],
            ctx = obj,
            depth = [ (($($depth)*).wrapping_add(1usize)) ],
            stack = [ [ [$($prefix)*] [$($order)*] arr ] $($stack)* ],
            mode = $mode,
            fields = [ $($inner)* , @ end_object $(, $($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $name:ident : array($sfcode:expr) [ $Elem:ident : object($esfcode:expr) { $($efields:tt)* } ; $($n:tt)+ ] $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_ARRAY,
            concat!("txn_template!: `", stringify!($name), "` is declared as `array` but its sfXxx code is not an STI_ARRAY field")
        );
        const _: () = assert!(
            $crate::txn::codec::sti_of($esfcode) == $crate::txn::codec::sti::STI_OBJECT,
            concat!("txn_template!: `", stringify!($name), "`'s element `", stringify!($Elem), "` is declared as `object` but its sfXxx code is not an STI_OBJECT field")
        );
        const _: () = assert!(
            ($($n)*) >= 1usize,
            concat!("txn_template!: `", stringify!($name), "`'s element count must be at least 1")
        );
        const _: () = assert!(
            (($($depth)*).wrapping_add(2usize)) < $crate::sto_writer::STO_WRITER_MAX_DEPTH,
            concat!("txn_template!: `", stringify!($name), "` would nest deeper than STO_WRITER_MAX_DEPTH")
        );
        $crate::__txn_template_step! {
            @step
            name = $Elem,
            meta = [
                #[doc = concat!("One element of `", stringify!($name), "`'s homogeneous array -- see [`", stringify!($Name), "::", stringify!($name), "`].")]
            ],
            vis = $vis,
            order = [],
            setters = [],
            emit_region = [],
            buf = [__ebytes],
            init = [
                $crate::txn::codec::write_field_header(&mut __ebytes, 0usize, $esfcode);
            ],
            prev = [ $crate::txn::codec::container_header_size($esfcode) ],
            table = [],
            emit_details = [false, 0usize],
            prefix = [],
            ctx = obj,
            depth = [ (($($depth)*).wrapping_add(2usize)) ],
            stack = [ [ [] [] obj ] ],
            mode = elem,
            fields = [ $($efields)* , @ end_object ]
        }
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($order)* ($sfcode).code(),],
            setters = [
                $($setters)*

                #[doc = concat!("Returns the `", stringify!($name), "` element at `index` (`None` if out of bounds).")]
                #[inline(always)]
                #[must_use]
                $vis fn [<$($prefix)* $name>](&mut self, index: usize) -> ::core::option::Option<$Elem<'_>> {
                    const OFF: usize = ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1);
                    const ELEM_LEN: usize = $Elem::<'static>::LEN;
                    const COUNT: usize = ($($n)*);
                    if index >= COUNT {
                        return ::core::option::Option::None;
                    }
                    let start = OFF.wrapping_add(index.wrapping_mul(ELEM_LEN));
                    self.bytes
                        .get_mut(start..start.wrapping_add(ELEM_LEN))
                        .map(|bytes| $Elem { bytes })
                }
            ],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
                $crate::txn::codec::write_repeated(
                    &mut $($buf)*,
                    ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1),
                    &$Elem::<'static>::TEMPLATE,
                    ($($n)*),
                );
                $crate::txn::codec::write_const_bytes(
                    &mut $($buf)*,
                    ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1).wrapping_add((($($n)*) as usize).wrapping_mul($Elem::<'static>::LEN)),
                    &[$crate::txn::codec::ARRAY_END_MARKER],
                );
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1).wrapping_add((($($n)*) as usize).wrapping_mul($Elem::<'static>::LEN)).wrapping_add(1usize) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_ARRAY, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }
    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $name:ident : array($sfcode:expr) [ $($inner:tt)* ] $(, $($rest:tt)*)? ]
    ) => {
        const _: () = assert!(
            $crate::txn::codec::sti_of($sfcode) == $crate::txn::codec::sti::STI_ARRAY,
            concat!("txn_template!: `", stringify!($name), "` is declared as `array` but its sfXxx code is not an STI_ARRAY field")
        );
        const _: () = assert!(
            (($($depth)*).wrapping_add(1usize)) < $crate::sto_writer::STO_WRITER_MAX_DEPTH,
            concat!("txn_template!: `", stringify!($name), "` would nest deeper than STO_WRITER_MAX_DEPTH")
        );
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [],
            setters = [$($setters)*],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_field_header(&mut $($buf)*, ($($prev)*), $sfcode);
            ],
            prev = [ ($($prev)*).wrapping_add($crate::txn::codec::container_header_size($sfcode)) ],
            table = [
                $($table)*
                (($sfcode).code(), $crate::txn::codec::KIND_ARRAY, ($($prev)*).wrapping_add($crate::txn::codec::field_header($sfcode).1), ($($depth)*)),
            ],
            emit_details = [$($emit_details)*],
            prefix = [$($prefix)* $name _],
            ctx = arr,
            depth = [ (($($depth)*).wrapping_add(1usize)) ],
            stack = [ [ [$($prefix)*] [$($order)* ($sfcode).code(),] obj ] $($stack)* ],
            mode = $mode,
            fields = [ $($inner)* , @ end_array $(, $($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = $ctx:tt, depth = [$($depth:tt)*],
        stack = [ [ [$($pfx:tt)*] [$($ord:tt)*] $old_ctx:tt ] $($stack:tt)* ],
        mode = $mode:tt,
        fields = [ @ end_object $(, $($rest:tt)*)? ]
    ) => {
        const _: () = {
            const ORDER: &[u32] = &[$($order)*];
            let mut i = 1;
            while i < ORDER.len() {
                assert!(
                    ORDER[i - 1] < ORDER[i],
                    "txn_template!: fields must be declared in canonical (type, field) order inside a nested `object` (sfXxx codes must be strictly increasing)"
                );
                i = i.wrapping_add(1);
            }
        };
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($ord)*],
            setters = [$($setters)*],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_const_bytes(&mut $($buf)*, ($($prev)*), &[$crate::txn::codec::OBJECT_END_MARKER]);
            ],
            prev = [ ($($prev)*).wrapping_add(1usize) ],
            table = [$($table)*],
            emit_details = [$($emit_details)*],
            prefix = [$($pfx)*],
            ctx = $old_ctx,
            depth = [ (($($depth)*).wrapping_sub(1usize)) ],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = $ctx:tt, depth = [$($depth:tt)*],
        stack = [ [ [$($pfx:tt)*] [$($ord:tt)*] $old_ctx:tt ] $($stack:tt)* ],
        mode = $mode:tt,
        fields = [ @ end_array $(, $($rest:tt)*)? ]
    ) => {
        $crate::__txn_template_step! {
            @step
            name = $Name, meta = [$(#[$meta])*], vis = $vis,
            order = [$($ord)*],
            setters = [$($setters)*],
            emit_region = [$($emit_region)*],
            buf = [$($buf)*],
            init = [
                $($init)*
                $crate::txn::codec::write_const_bytes(&mut $($buf)*, ($($prev)*), &[$crate::txn::codec::ARRAY_END_MARKER]);
            ],
            prev = [ ($($prev)*).wrapping_add(1usize) ],
            table = [$($table)*],
            emit_details = [$($emit_details)*],
            prefix = [$($pfx)*],
            ctx = $old_ctx,
            depth = [ (($($depth)*).wrapping_sub(1usize)) ],
            stack = [$($stack)*],
            mode = $mode,
            fields = [ $($($rest)*)? ]
        }

    };
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = obj, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
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
            prefix = [$($prefix)*],
            ctx = obj,
            depth = [$($depth)*],
            stack = [$($stack)*],
            mode = $mode,
            fields = []
        }

    };

    // Base case (single, unconditional): no fields left. `table` and
    // `emit_details` bind unconditionally — `emit_details` is ALWAYS
    // exactly `[bool, expr]` (a real offset if declared, `0usize` if not),
    // so its `:expr` fragment always matches. `$Name::FIELDS` and
    // `prepare_for_emit()` are generated unconditionally; whether the
    // crate actually compiles is down to the required-field
    // presence/kind-agreement assert items below, plus every per-field
    // serialized-type check, per-container order check, and per-container
    // depth check already emitted by the arms that got here.
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$ed_p:tt, $ed_off:expr],
        prefix = [$($prefix:tt)*], ctx = $ctx:tt, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = tpl,
        fields = []
    ) => {
        $(#[$meta])*
        #[derive(Clone)]
        $vis struct $Name {
            bytes: [u8; $Name::LEN],
        }

        // `[<set_ $($prefix)* $field>]` setter names (spliced into
        // `$($setters)*` above, per-field, by each scalar kind's own arm)
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
            /// `(sfcode, kind tag, payload offset, depth)` row per declared
            /// scalar or container field (`emit_details` excluded — it has
            /// no `sfcode`). Backs the required-field presence/
            /// kind-agreement checks below and `prepare_for_emit`'s offset
            /// resolution; see [`crate::txn::codec::FieldEntry`].
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

        // Required-field checks (E0080) for the six fields every emitted
        // transaction needs, each its own `const` item so every problem is
        // reported, not just the first one found: a presence check and a
        // kind-agreement check per field (both via value-based lookup in
        // `$Name::FIELDS`, robust to how the sfcode constant was spelled at
        // the declaration site, and only ever matching a depth-0 row), plus
        // a presence check for `emit_details` (which has no sfcode, so it
        // is tracked via its own accumulator instead of the table). These
        // sit alongside the per-field serialized-type checks, per-container
        // order checks, and per-container depth checks each arm above
        // already emitted while walking the field list.
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
    // Base case for an element view type (`mode = elem`), spawned by the
    // homogeneous-array arm above: emits only the borrowed-view struct and
    // its inherent impl (`LEN`, `TEMPLATE`, the inner
    // setters, `bytes()`) -- none of the template-only items (no
    // plumbing/presence/kind asserts, no `emit_details` presence check, no
    // `prepare_for_emit`, no `TemplateBytes`/`Default`/`Clone`). The
    // element's own per-container order check was already emitted by the
    // `@end_object` arm that closed it before recursion reached here, so
    // `$($order)*` here is just the popped, unused top-level `[]`.
    (
        @step
        name = $Elem:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = $ctx:tt, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = elem,
        fields = []
    ) => {
        $(#[$meta])*
        $vis struct $Elem<'a> {
            bytes: &'a mut [u8],
        }

        $crate::__paste! {
        // `Self::LEN` can't name the array length of a sibling `Self::TEMPLATE`
        // const in the same impl (an anonymous-const-with-generic-Self
        // restriction, since `Self` here carries a lifetime parameter) --
        // this private, module-level const is the same value, named
        // outside the impl so both `LEN` and `TEMPLATE` below can use it.
        const [<__ $Elem _LEN>]: usize = $($prev)*;

        impl<'a> $Elem<'a> {
            /// Fixed serialized length of one element: this container's
            /// header, its inner fields, and the closing `0xE1`
            /// object-end marker.
            pub const LEN: usize = [<__ $Elem _LEN>];

            /// The element's default bytes -- header, every inner
            /// field's default, and the closing `0xE1` marker -- baked
            /// at compile time and copied into the parent array's
            /// reserved region once per element.
            pub const TEMPLATE: [u8; [<__ $Elem _LEN>]] = {
                let mut $($buf)* = [0u8; [<__ $Elem _LEN>]];
                $($init)*
                $($buf)*
            };

            $($setters)*

            /// Returns this element's full [`Self::LEN`]-byte region.
            #[inline(always)]
            #[must_use]
            $vis fn bytes(&self) -> &[u8] {
                &*self.bytes
            }
        }
        } // $crate::__paste!
    };

    // `emit_details` inside any container (a non-empty `stack`): a named
    // error, ahead of the catch-all below.
    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = $ctx:tt, depth = [$($depth:tt)*],
        stack = [ [ $($frame:tt)* ] $($stack:tt)* ],
        mode = $mode:tt,
        fields = [ $field:ident : emit_details $($rest:tt)* ]
    ) => {
        compile_error!(concat!(
            "txn_template!: `",
            stringify!($field),
            ": emit_details` must be the last top-level field — it cannot be declared inside a nested `object`/`array`"
        ));
    };

    (
        @step
        name = $Name:ident, meta = [$(#[$meta:meta])*], vis = $vis:vis,
        order = [$($order:tt)*], setters = [$($setters:tt)*], emit_region = [$($emit_region:tt)*],
        buf = [$($buf:tt)*], init = [$($init:tt)*], prev = [$($prev:tt)*],
        table = [$($table:tt)*], emit_details = [$($emit_details:tt)*],
        prefix = [$($prefix:tt)*], ctx = $ctx:tt, depth = [$($depth:tt)*],
        stack = [$($stack:tt)*],
        mode = $mode:tt,
        fields = [ $($bad:tt)+ ]
    ) => {
        compile_error!(concat!(
            "txn_template!: unrecognized field declaration: ",
            stringify!($($bad)*)
        ));
    };
}

#[cfg(test)]
mod tests {
    // Tests are exempt from the panic-freedom lints (see docs/DESIGN.md
    // §8); expect/indexing on known-good values is idiomatic here.
    #![allow(clippy::expect_used, clippy::indexing_slicing)]

    use crate::txn::codec;
    use crate::types::{ACC_ID_LEN, AccountId, CurrencyCode, EMIT_DETAILS_MAX_LEN};
    use crate::xfl::XFL;
    use rshooks_core::consts::tfCANONICAL;
    // The typed constants: `txn_template!` calls `.code()` on whatever it is
    // given, so its field list takes `SField`s, not raw `u32`s.
    use crate::sfield::{
        sfAccount, sfAmount, sfAmountEntry, sfAmounts, sfAuthorize, sfBaseAsset, sfBlob,
        sfClaimCurrency, sfDestination, sfDestinationTag, sfEmailHash, sfFee,
        sfFirstLedgerSequence, sfFlags, sfHook, sfHookGrant, sfHookGrants, sfHookHash, sfHooks,
        sfIndexNext, sfInvoiceID, sfLastLedgerSequence, sfMemo, sfMemoData, sfMemoType, sfMemos,
        sfSequence, sfSignerWeight, sfSigningPubKey, sfSourceTag, sfTakerPaysCurrency,
        sfTransactionResult,
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

    // -----------------------------------------------------------------
    // Nested `object`/`array` fields: `TestRemit`
    // -----------------------------------------------------------------

    crate::txn_template! {
        /// A `Remit`-shaped template exercising nested `object`/`array`
        /// fields: `sfAmounts` holds two fixed `AmountEntry`s, one
        /// `native_amount`, one `amount` with a baked `(currency, issuer)`
        /// default. See `EXPECTED_FIXED_PREFIX` below for the byte-compat
        /// proof, hand-derived from [`crate::txn::codec::field_header`]'s
        /// rule and cross-checked against `TestRemit::new().bytes()`.
        struct TestRemit {
            transaction_type = ttREMIT,
            flags: u32_field(sfFlags) = 0,
            sequence: u32_field(sfSequence) = 0,
            first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
            last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
            fee: native_amount(sfFee) = 0,
            signing_pub_key: empty_vl(sfSigningPubKey),
            account: account_id(sfAccount),
            destination: account_id(sfDestination),
            amounts: array(sfAmounts) [
                native: object(sfAmountEntry) {
                    amount: native_amount(sfAmount) = 1,
                },
                usd: object(sfAmountEntry) {
                    amount: amount(sfAmount) = (
                        XFL::from_raw_bits(0),
                        CurrencyCode::from_iso(b"USD"),
                        AccountId([0x44; ACC_ID_LEN])
                    ),
                },
            ],
            emit_details: emit_details,
        }
    }

    /// The exact 147-byte fixed prefix `TestRemit` produces, hand-derived
    /// field by field from [`crate::txn::codec::field_header`]'s rule (see
    /// the module's `KNOWN_HEADERS` table for the same rule applied to
    /// simple fields): `TransactionType(1,2)` is one byte (`0x12`);
    /// `Amounts(15,92)` and `AmountEntry(14,91)` both have `field >= 16`
    /// with `type < 16`, so their headers are the two-byte
    /// `[type << 4, field]` form (`0xF0 0x5C` and `0xE0 0x5B`); every other
    /// field here is the same one-byte or two-byte form already proven by
    /// `TestPayment`'s fixture above. The `usd` entry's `amount` value is
    /// the 48-byte issued form of XFL zero (`0x80` + 7 zero bytes),
    /// `CurrencyCode::from_iso(b"USD")`, and `AccountId([0x44; 20])`.
    #[rustfmt::skip]
    const REMIT_EXPECTED_FIXED_PREFIX: [u8; 147] = [
        0x12, 0x00, 0x5F,                                                        // TransactionType (1,2): ttREMIT = 95
        0x22, 0x00, 0x00, 0x00, 0x00,                                            // Flags (2,2)
        0x24, 0x00, 0x00, 0x00, 0x00,                                            // Sequence (2,4): required field
        0x20, 0x1A, 0x00, 0x00, 0x00, 0x00,                                      // FirstLedgerSequence (2,26): required field
        0x20, 0x1B, 0x00, 0x00, 0x00, 0x00,                                      // LastLedgerSequence (2,27): required field
        0x68, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,                    // Fee (6,8): required field, native 0 drops
        0x73, 0x00,                                                              // SigningPubKey (7,3): required field, empty VL
        0x81, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,   // Account (8,1): required field, VL(20)
        0x83, 0x14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,   // Destination (8,3): VL(20)
        0xF0, 0x5C,                                                              // Amounts (15,92): STArray header
          0xE0, 0x5B,                                                            // AmountEntry #1 (14,91): STObject header
            0x61, 0x40, 0, 0, 0, 0, 0, 0, 1,                                    // Amount (6,1): native 1 drop
          0xE1,                                                                  // object end marker
          0xE0, 0x5B,                                                            // AmountEntry #2 (14,91): STObject header
            0x61,                                                                // Amount (6,1) header
            0x80, 0, 0, 0, 0, 0, 0, 0,                                           // issued value: XFL zero
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'U', b'S', b'D', 0, 0, 0, 0, 0,  // currency: USD
            0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44,          // issuer (first 10 of 20 bytes)
            0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44,          // issuer (remaining 10 bytes)
          0xE1,                                                                  // object end marker
        0xF1,                                                                    // array end marker
    ];

    #[test]
    fn remit_matches_expected_fixed_prefix_byte_for_byte() {
        let tpl = TestRemit::new();
        assert_eq!(&tpl.bytes()[..147], &REMIT_EXPECTED_FIXED_PREFIX[..]);
    }

    #[test]
    fn remit_len_is_fixed_prefix_plus_emit_details_max() {
        assert_eq!(TestRemit::LEN, 147 + EMIT_DETAILS_MAX_LEN);
    }

    #[test]
    fn remit_nested_setters_write_at_the_expected_offsets() {
        let mut tpl = TestRemit::new();
        tpl.set_amounts_native_amount(5)
            .expect("5 drops is in range");
        assert_eq!(&tpl.bytes()[85..93], &[0x40, 0, 0, 0, 0, 0, 0, 5]);

        tpl.set_amounts_usd_amount_value(XFL::from_raw_bits(6_107_031_094_714_392_576));
        // Overwrites only the 8-byte value region; currency/issuer (the
        // baked default) are untouched.
        assert_eq!(
            &tpl.bytes()[105..145],
            &REMIT_EXPECTED_FIXED_PREFIX[105..145]
        );

        tpl.set_amounts_usd_amount(
            XFL::from_raw_bits(6_107_081_094_714_392_576),
            &CurrencyCode::from_iso(b"EUR"),
            &AccountId([0x55; ACC_ID_LEN]),
        );
        assert_eq!(&tpl.bytes()[105..125][12..15], b"EUR");
        assert_eq!(&tpl.bytes()[125..145], &[0x55; ACC_ID_LEN][..]);
    }

    #[test]
    fn remit_prepare_for_emit_propagates_host_stub_errors() {
        let mut tpl = TestRemit::new();
        // Exercise every top-level setter too (dead-code hygiene, as with
        // `TestPayment`/`QualifiedPathAccount` above).
        tpl.set_flags(tfCANONICAL);
        tpl.set_sequence(0);
        tpl.set_first_ledger_sequence(0);
        tpl.set_last_ledger_sequence(0);
        tpl.set_fee(0).expect("0 drops is in range");
        tpl.set_account(&AccountId::default());
        tpl.set_destination(&AccountId::default());
        assert_eq!(tpl.emit_details_region().len(), EMIT_DETAILS_MAX_LEN);
        assert_eq!(
            tpl.prepare_for_emit()
                .expect_err("prepare_for_emit must fail on the host stub"),
            crate::error::HookError::NotImplemented
        );
    }

    // -----------------------------------------------------------------
    // Homogeneous `array(sfX) [ Elem: object(sfY) { .. } ; N ]` fields:
    // `TestRemitIndexed`
    // -----------------------------------------------------------------

    crate::txn_template! {
        /// The same plumbing as `TestRemit`, but `sfAmounts` is a
        /// homogeneous, runtime-indexed array of three identical
        /// `AmountEntry` elements (all issued, with a baked USD/0x44
        /// default) rather than named, individually-declared entries.
        struct TestRemitIndexed {
            transaction_type = ttREMIT,
            flags: u32_field(sfFlags) = 0,
            sequence: u32_field(sfSequence) = 0,
            first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
            last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
            fee: native_amount(sfFee) = 0,
            signing_pub_key: empty_vl(sfSigningPubKey),
            account: account_id(sfAccount),
            destination: account_id(sfDestination),
            amounts: array(sfAmounts) [
                AmountEntry: object(sfAmountEntry) {
                    amount: amount(sfAmount) = (
                        XFL::from_raw_bits(0),
                        CurrencyCode::from_iso(b"USD"),
                        AccountId([0x44; ACC_ID_LEN])
                    ),
                }; 3
            ],
            emit_details: emit_details,
        }
    }

    #[test]
    fn amount_entry_len_and_template_are_byte_exact() {
        // header(sfAmountEntry) (2 bytes: type 14 >= 16? no -- type < 16,
        // field 91 >= 16, so the 2-byte `[type << 4, field]` form) + one
        // inner `amount` field (1-byte header + 48-byte issued value) + the
        // closing `0xE1` object-end marker.
        assert_eq!(AmountEntry::LEN, 2 + 1 + 48 + 1);

        #[rustfmt::skip]
        let expected: [u8; 52] = [
            0xE0, 0x5B,                                                             // AmountEntry (14,91) header
            0x61,                                                                   // Amount (6,1) header
            0x80, 0, 0, 0, 0, 0, 0, 0,                                              // issued value: XFL zero
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'U', b'S', b'D', 0, 0, 0, 0, 0,     // currency: USD
            0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44,             // issuer (first 10 of 20 bytes)
            0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44, 0x44,             // issuer (remaining 10 bytes)
            0xE1,                                                                   // object end marker
        ];
        assert_eq!(AmountEntry::TEMPLATE, expected);
    }

    /// The fixed prefix through `sfAccount`/`sfDestination` is identical to
    /// `TestRemit`'s (same fields, same order, same defaults; see
    /// `REMIT_EXPECTED_FIXED_PREFIX` above), so only `sfAmounts` onward is
    /// re-derived here: header `0xF0 0x5C`, three back-to-back 52-byte
    /// `AmountEntry::TEMPLATE` copies (`AmountEntry::LEN` above), then the
    /// `0xF1` array end marker.
    #[test]
    fn indexed_matches_expected_fixed_prefix_byte_for_byte() {
        let tpl = TestRemitIndexed::new();
        let b = tpl.bytes();
        assert_eq!(&b[..80], &REMIT_EXPECTED_FIXED_PREFIX[..80]);
        assert_eq!(&b[80..82], &[0xF0, 0x5C]);
        for i in 0..3usize {
            let start = 82usize.wrapping_add(i.wrapping_mul(AmountEntry::LEN));
            assert_eq!(
                &b[start..start.wrapping_add(AmountEntry::LEN)],
                &AmountEntry::TEMPLATE[..],
                "element {i}"
            );
        }
        assert_eq!(b[82 + 3 * AmountEntry::LEN], 0xF1);
        assert_eq!(
            TestRemitIndexed::LEN,
            (82 + 3 * AmountEntry::LEN + 1) + EMIT_DETAILS_MAX_LEN
        );
    }

    #[test]
    fn indexed_accessor_writes_each_element_without_disturbing_neighbours() {
        let mut tpl = TestRemitIndexed::new();
        for i in 0..3usize {
            let mut entry = tpl.amounts(i).expect("index in range");
            entry.set_amount_value(XFL::from_raw_bits(6_089_866_696_204_910_592)); // XFL!(1)
        }

        let b = tpl.bytes();
        for i in 0..3usize {
            let value_off = 82usize
                .wrapping_add(i.wrapping_mul(AmountEntry::LEN))
                .wrapping_add(3); // AmountEntry header (2) + Amount header (1)
            assert_eq!(
                &b[value_off..value_off.wrapping_add(8)],
                &[0xD4, 0x83, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00],
                "element {i}'s value"
            );
            // currency/issuer (the baked default) are untouched.
            let cur_off = value_off.wrapping_add(8);
            assert_eq!(
                &b[cur_off..cur_off.wrapping_add(40)],
                &AmountEntry::TEMPLATE[11..51],
                "element {i}'s currency/issuer"
            );
        }

        // `set_amount` (the full 48-byte setter) writes currency/issuer
        // too, not just the value.
        let currency = CurrencyCode::from_iso(b"EUR");
        let issuer = AccountId([0x66; ACC_ID_LEN]);
        tpl.amounts(0).expect("index in range").set_amount(
            XFL::from_raw_bits(0),
            &currency,
            &issuer,
        );
        let element0_off = 82usize;
        assert_eq!(
            &tpl.bytes()[element0_off.wrapping_add(11)..element0_off.wrapping_add(31)],
            currency.as_ref()
        );
        assert_eq!(
            &tpl.bytes()[element0_off.wrapping_add(31)..element0_off.wrapping_add(51)],
            issuer.as_ref()
        );
    }

    #[test]
    fn indexed_accessor_returns_none_out_of_bounds() {
        let mut tpl = TestRemitIndexed::new();
        assert!(tpl.amounts(3).is_none());
        assert!(tpl.amounts(usize::MAX).is_none());
    }

    #[test]
    fn indexed_element_bytes_returns_exactly_len_bytes() {
        let mut tpl = TestRemitIndexed::new();
        let entry = tpl.amounts(0).expect("index in range");
        assert_eq!(entry.bytes().len(), AmountEntry::LEN);
        assert_eq!(entry.bytes(), &AmountEntry::TEMPLATE[..]);
    }

    #[test]
    fn indexed_prepare_for_emit_propagates_host_stub_errors() {
        let mut tpl = TestRemitIndexed::new();
        tpl.set_flags(0);
        tpl.set_sequence(0);
        tpl.set_first_ledger_sequence(0);
        tpl.set_last_ledger_sequence(0);
        tpl.set_fee(0).expect("0 drops is in range");
        tpl.set_account(&AccountId::default());
        tpl.set_destination(&AccountId::default());
        let _ = tpl.emit_details_region();
        assert_eq!(
            tpl.prepare_for_emit()
                .expect_err("prepare_for_emit must fail on the host stub"),
            crate::error::HookError::NotImplemented
        );
    }

    // -----------------------------------------------------------------
    // Nested homogeneous arrays: a two-level indexed accessor
    // (`hooks(i)?.grants(j)?`)
    // -----------------------------------------------------------------

    crate::txn_template! {
        /// A minimal required-fields-only template whose `sfHooks` array
        /// holds two `HookEntry` elements, each itself holding a nested
        /// homogeneous `sfHookGrants` array of two `Grant` elements --
        /// exercises a homogeneous array declared *inside* another
        /// homogeneous array's element.
        struct HookChain {
            transaction_type = ttINVOKE,
            sequence: u32_field(sfSequence) = 0,
            first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
            last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
            fee: native_amount(sfFee) = 0,
            signing_pub_key: empty_vl(sfSigningPubKey),
            account: account_id(sfAccount),
            hooks: array(sfHooks) [
                HookEntry: object(sfHook) {
                    hook_hash: hash256(sfHookHash),
                    grants: array(sfHookGrants) [
                        Grant: object(sfHookGrant) {
                            hook_hash: hash256(sfHookHash),
                            authorize: account_id(sfAuthorize),
                        }; 2
                    ],
                }; 2
            ],
            emit_details: emit_details,
        }
    }

    /// `Grant`'s own fixed layout: header(sfHookGrant) (2 bytes: type 14 <
    /// 16, field 24 >= 16) + `hook_hash` (header(sfHookHash), 2 bytes: type
    /// 5 < 16, field 31 >= 16, plus 32 zero bytes) + `authorize`
    /// (header(sfAuthorize), 1 byte: type 8 < 16, field 5 < 16, plus a
    /// 1-byte VL length and a 20-byte `AccountId` payload -- `account_id`
    /// always reserves that VL byte, nested or not) + the closing `0xE1`
    /// marker. `authorize`'s value starts right after `hook_hash` and
    /// `authorize`'s own header/VL bytes.
    const GRANT_AUTHORIZE_VALUE_OFFSET: usize = 2 + (2 + 32) + 1 + 1;

    #[test]
    fn grant_len_matches_the_hand_derived_layout() {
        assert_eq!(
            Grant::LEN,
            GRANT_AUTHORIZE_VALUE_OFFSET + ACC_ID_LEN + 1 /* 0xE1 */
        );
    }

    /// `Grant::TEMPLATE`, hand-derived byte for byte per the layout above.
    #[rustfmt::skip]
    const GRANT_TEMPLATE_EXPECTED: [u8; 59] = [
        0xE0, 0x18,                                                             // HookGrant (14,24) header
        0x50, 0x1F,                                                             // HookHash (5,31) header
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,                          // hook_hash: zeroed (16 of 32)
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,                          // hook_hash: zeroed (remaining 16)
        0x85,                                                                   // Authorize (8,5) header
        0x14,                                                                   // VL length byte (20)
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,              // authorize: zeroed
        0xE1,                                                                   // object end marker
    ];

    #[test]
    fn grant_template_matches_expected_bytes() {
        assert_eq!(Grant::TEMPLATE, GRANT_TEMPLATE_EXPECTED);
    }

    /// `HookEntry`'s own fixed layout: header(sfHook) (1 byte: type 14 <
    /// 16, field 14 < 16 -- unlike `sfAmountEntry`'s 2-byte header, both of
    /// `sfHook`'s components fit in a nibble) + `hook_hash`
    /// (header(sfHookHash), 2 bytes, plus 32 zero bytes) + `grants`
    /// (header(sfHookGrants), 2 bytes: type 15 < 16, field 20 >= 16, plus
    /// two back-to-back `Grant::TEMPLATE` copies, plus the closing `0xF1`)
    /// + the closing `0xE1` marker.
    const HOOK_ENTRY_GRANTS_OFFSET: usize = 1 + (2 + 32) + 2;

    #[test]
    fn hook_entry_len_matches_the_hand_derived_layout() {
        assert_eq!(
            HookEntry::LEN,
            HOOK_ENTRY_GRANTS_OFFSET + 2 * Grant::LEN + 1 /* 0xF1 */ + 1 /* 0xE1 */
        );
    }

    #[test]
    fn hook_entry_template_matches_expected_bytes() {
        // The fixed header prefix, hand-derived above, followed by two
        // back-to-back copies of `GRANT_TEMPLATE_EXPECTED` (already pinned
        // byte-for-byte by `grant_template_matches_expected_bytes`) and the
        // closing `0xF1`/`0xE1` markers -- built rather than transcribed
        // twice by hand, since the wire bytes genuinely are two identical
        // copies of the same already-verified array.
        let mut full = [0u8; 157];
        #[rustfmt::skip]
        let header_prefix: [u8; 37] = [
            0xEE,                                                             // Hook (14,14) header
            0x50, 0x1F,                                                       // HookHash (5,31) header
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,                    // hook_hash: zeroed (16 of 32)
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,                    // hook_hash: zeroed (remaining 16)
            0xF0, 0x14,                                                       // HookGrants (15,20) header
        ];
        assert_eq!(header_prefix.len(), HOOK_ENTRY_GRANTS_OFFSET);
        full[..HOOK_ENTRY_GRANTS_OFFSET].copy_from_slice(&header_prefix);
        let grants_end = HOOK_ENTRY_GRANTS_OFFSET + 2 * Grant::LEN;
        full[HOOK_ENTRY_GRANTS_OFFSET..HOOK_ENTRY_GRANTS_OFFSET + Grant::LEN]
            .copy_from_slice(&GRANT_TEMPLATE_EXPECTED);
        full[HOOK_ENTRY_GRANTS_OFFSET + Grant::LEN..grants_end]
            .copy_from_slice(&GRANT_TEMPLATE_EXPECTED);
        full[grants_end] = 0xF1;
        full[grants_end + 1] = 0xE1;

        assert_eq!(HookEntry::LEN, full.len());
        assert_eq!(HookEntry::TEMPLATE, full);
    }

    #[test]
    fn nested_two_level_indexed_accessor_lands_at_the_expected_element() {
        let mut tpl = HookChain::new();
        let expected_authorize = AccountId([0x22; ACC_ID_LEN]);

        tpl.hooks(1)
            .expect("index in range")
            .grants(0)
            .expect("index in range")
            .set_authorize(&expected_authorize);

        // Written exactly where `Grant`'s own layout says `authorize`
        // lives, nowhere else.
        let mut hook1 = tpl.hooks(1).expect("index in range");
        let written = hook1.grants(0).expect("index in range");
        let off = GRANT_AUTHORIZE_VALUE_OFFSET;
        assert_eq!(
            &written.bytes()[off..off.wrapping_add(ACC_ID_LEN)],
            expected_authorize.as_ref()
        );
        // The rest of that element (headers, `hook_hash`, the `0xE1`
        // marker) still matches the baked default.
        assert_eq!(&written.bytes()[..off], &Grant::TEMPLATE[..off]);

        // Every other `Grant` slot -- the other grant in the same
        // `HookEntry`, and both grants of the other `HookEntry` -- is
        // untouched, proving the two-level accessor didn't write past its
        // own element.
        for (hook_index, grant_index) in [(0, 0), (0, 1), (1, 1)] {
            let mut hook_entry = tpl.hooks(hook_index).expect("index in range");
            let other = hook_entry.grants(grant_index).expect("index in range");
            assert_eq!(
                other.bytes(),
                &Grant::TEMPLATE[..],
                "hooks({hook_index}).grants({grant_index}) must be untouched"
            );
        }

        // Exercise the remaining generated setters too (dead-code
        // hygiene, as with the other fixtures above): `HookEntry`'s own
        // `set_hook_hash`, `Grant`'s `set_hook_hash`, `HookEntry::bytes`,
        // and every `HookChain` required-field setter.
        let hook_hash = crate::types::Hash([0x33; 32]);
        {
            let mut hook0 = tpl.hooks(0).expect("index in range");
            hook0.set_hook_hash(&hook_hash);
            assert_eq!(hook0.bytes().len(), HookEntry::LEN);
            let mut grant1 = hook0.grants(1).expect("index in range");
            grant1.set_hook_hash(&hook_hash);
        }
        tpl.set_sequence(0);
        tpl.set_first_ledger_sequence(0);
        tpl.set_last_ledger_sequence(0);
        tpl.set_fee(0).expect("0 drops is in range");
        tpl.set_account(&AccountId::default());

        let _ = tpl.emit_details_region();
        assert_eq!(
            tpl.prepare_for_emit()
                .expect_err("prepare_for_emit must fail on the host stub"),
            crate::error::HookError::NotImplemented
        );
    }

    // -----------------------------------------------------------------
    // `fixed_vl(sfX, N)`: a fixed-length VL blob
    // -----------------------------------------------------------------

    crate::txn_template! {
        /// A homogeneous, single-element `sfMemos` array whose `Memo`
        /// element declares both a `fixed_vl` field with a baked default
        /// (`memo_type`) and one without (`memo_data`, zeroed) -- exercises
        /// `fixed_vl` inside a container, at the one-byte VL-prefix size
        /// (both payloads are well under 192 bytes). `sfMemoType` (7,12) <
        /// `sfMemoData` (7,13), satisfying the element's own canonical
        /// order.
        struct TestMemoFixture {
            transaction_type = ttPAYMENT,
            sequence: u32_field(sfSequence) = 0,
            first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
            last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
            fee: native_amount(sfFee) = 0,
            signing_pub_key: empty_vl(sfSigningPubKey),
            account: account_id(sfAccount),
            memos: array(sfMemos) [
                Memo: object(sfMemo) {
                    memo_type: fixed_vl(sfMemoType, 4) = *b"note",
                    memo_data: fixed_vl(sfMemoData, 8),
                }; 1
            ],
            emit_details: emit_details,
        }
    }

    /// `Memo::TEMPLATE`, hand-derived byte for byte: header(sfMemo) (1
    /// byte: type 14 < 16, field 10 < 16) + `memo_type`
    /// (header(sfMemoType), 1 byte: type 7 < 16, field 12 < 16, a
    /// single-byte VL prefix since 4 <= 192, then the baked `*b"note"`
    /// default) + `memo_data` (header(sfMemoData), 1 byte: type 7 < 16,
    /// field 13 < 16, a single-byte VL prefix since 8 <= 192, then 8 zero
    /// bytes -- no default given) + the closing `0xE1` marker.
    #[rustfmt::skip]
    const MEMO_TEMPLATE_EXPECTED: [u8; 18] = [
        0xEA,                               // Memo (14,10) header
        0x7C, 0x04, b'n', b'o', b't', b'e', // MemoType (7,12) header, VL prefix (4), payload
        0x7D, 0x08, 0, 0, 0, 0, 0, 0, 0, 0,  // MemoData (7,13) header, VL prefix (8), zeroed payload
        0xE1,                               // object end marker
    ];

    #[test]
    fn memo_len_and_template_are_byte_exact() {
        assert_eq!(Memo::LEN, MEMO_TEMPLATE_EXPECTED.len());
        assert_eq!(Memo::TEMPLATE, MEMO_TEMPLATE_EXPECTED);
    }

    #[test]
    fn memo_data_setter_writes_at_the_expected_offset() {
        let mut tpl = TestMemoFixture::new();
        let mut memo = tpl.memos(0).expect("index in range");
        memo.set_memo_data(&[0xAB; 8]);
        // memo_data's payload starts right after the header/VL prefix
        // bytes `MEMO_TEMPLATE_EXPECTED` pins as offsets 7..9.
        assert_eq!(&memo.bytes()[9..17], &[0xAB; 8]);
        // memo_type (and the surrounding headers/markers) are untouched.
        assert_eq!(&memo.bytes()[..7], &MEMO_TEMPLATE_EXPECTED[..7]);
        assert_eq!(memo.bytes()[17], 0xE1);

        assert!(tpl.memos(1).is_none());
    }

    #[test]
    fn memo_type_setter_writes_at_the_expected_offset() {
        let mut tpl = TestMemoFixture::new();
        let mut memo = tpl.memos(0).expect("index in range");
        memo.set_memo_type(b"cccc");
        assert_eq!(&memo.bytes()[3..7], b"cccc");

        // Exercise every required-field setter too (dead-code hygiene).
        tpl.set_sequence(0);
        tpl.set_first_ledger_sequence(0);
        tpl.set_last_ledger_sequence(0);
        tpl.set_fee(0).expect("0 drops is in range");
        tpl.set_account(&AccountId::default());
        let _ = tpl.emit_details_region();
        assert_eq!(
            tpl.prepare_for_emit()
                .expect_err("prepare_for_emit must fail on the host stub"),
            crate::error::HookError::NotImplemented
        );
    }

    crate::txn_template! {
        /// A top-level `fixed_vl` field at the two-byte VL-prefix boundary
        /// (`N = 193`, the smallest length needing a two-byte prefix).
        /// `sfBlob` (7,26) sits between `sfSigningPubKey` (7,3) and
        /// `sfAccount` (8,1) in canonical order.
        struct BlobFixture {
            transaction_type = ttPAYMENT,
            sequence: u32_field(sfSequence) = 0,
            first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
            last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
            fee: native_amount(sfFee) = 0,
            signing_pub_key: empty_vl(sfSigningPubKey),
            blob: fixed_vl(sfBlob, 193),
            account: account_id(sfAccount),
            emit_details: emit_details,
        }
    }

    #[test]
    fn blob_fixture_header_prefix_and_payload_offsets() {
        let mut tpl = BlobFixture::new();
        // header: TransactionType(3) + Sequence(5) + First(6) + Last(6) +
        // Fee(9) + SigningPubKey(2) = 31.
        let header_off = 31usize;
        assert_eq!(&tpl.bytes()[header_off..header_off + 2], &[0x70, 0x1A]); // Blob (7,26) header
        let prefix_off = header_off + 2;
        assert_eq!(&tpl.bytes()[prefix_off..prefix_off + 2], &[193, 0]); // VL prefix for N = 193
        let payload_off = prefix_off + 2;
        assert_eq!(
            &tpl.bytes()[payload_off..payload_off + 193],
            &[0u8; 193][..]
        );

        let value = [0x5Au8; 193];
        tpl.set_blob(&value);
        assert_eq!(&tpl.bytes()[payload_off..payload_off + 193], &value[..]);

        // `account` (the next field) starts exactly where the blob's
        // payload ends.
        let account_off = payload_off + 193;
        assert_eq!(tpl.bytes()[account_off], 0x81); // Account (8,1) header
        tpl.set_account(&AccountId::default());

        tpl.set_sequence(0);
        tpl.set_first_ledger_sequence(0);
        tpl.set_last_ledger_sequence(0);
        tpl.set_fee(0).expect("0 drops is in range");
        let _ = tpl.emit_details_region();
        assert_eq!(
            tpl.prepare_for_emit()
                .expect_err("prepare_for_emit must fail on the host stub"),
            crate::error::HookError::NotImplemented
        );
    }

    #[test]
    fn fixed_vl_bytes_match_sto_writer_vl() {
        // Cross-check `fixed_vl`'s baked header/prefix/payload bytes
        // against `StoWriter::vl` -- the runtime counterpart, built from
        // the same `codec::vl_length_prefix` -- for both the one-byte and
        // two-byte prefix forms this module already pins by hand.
        let mut memo_buf = [0u8; 64];
        let mut memo_writer = crate::sto_writer::StoWriter::new(&mut memo_buf);
        memo_writer.vl(sfMemoType, b"note").expect("fits");
        assert_eq!(memo_writer.as_bytes(), &MEMO_TEMPLATE_EXPECTED[1..7]);

        let mut blob_buf = [0u8; 256];
        let mut blob_writer = crate::sto_writer::StoWriter::new(&mut blob_buf);
        let value = [0x5Au8; 193];
        blob_writer.vl(sfBlob, &value).expect("fits");

        let mut tpl = BlobFixture::new();
        tpl.set_blob(&value);
        // The same 31-byte header offset `blob_fixture_header_prefix_and_payload_offsets`
        // derives by hand.
        let header_off = 31usize;
        assert_eq!(
            blob_writer.as_bytes(),
            &tpl.bytes()[header_off..header_off + 2 + 2 + 193]
        );
    }

    // -----------------------------------------------------------------
    // Per-kind byte fixtures: u8/u16/u64/hash128/hash160/hash256/currency/
    // native_issue, all on non-feature-gated `sfXxx` constants.
    // -----------------------------------------------------------------

    crate::txn_template! {
        /// One field per new scalar kind, interleaved with the six
        /// required emit-plumbing fields to keep canonical `sfXxx` order.
        struct PerKindFixture {
            transaction_type = ttPAYMENT,
            signer_weight: u16_field(sfSignerWeight) = 0x1234,
            sequence: u32_field(sfSequence) = 0,
            first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
            last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
            index_next: u64_field(sfIndexNext) = 0x0102_0304_0506_0708,
            email_hash: hash128(sfEmailHash),
            invoice_id: hash256(sfInvoiceID),
            fee: native_amount(sfFee) = 0,
            signing_pub_key: empty_vl(sfSigningPubKey),
            account: account_id(sfAccount),
            transaction_result: u8_field(sfTransactionResult) = 0xAB,
            taker_pays_currency: hash160(sfTakerPaysCurrency),
            claim_currency: native_issue(sfClaimCurrency),
            base_asset: currency(sfBaseAsset),
            emit_details: emit_details,
        }
    }

    #[test]
    fn per_kind_fixture_header_and_default_bytes() {
        let tpl = PerKindFixture::new();
        let b = tpl.bytes();
        assert_eq!(&b[0..3], &[0x12, 0x00, 0x00]); // TransactionType: ttPAYMENT = 0
        assert_eq!(&b[3..6], &[0x13, 0x12, 0x34]); // SignerWeight (1,3): u16 default 0x1234
        assert_eq!(b[23], 0x31); // IndexNext (3,1) header
        assert_eq!(&b[24..32], &0x0102_0304_0506_0708u64.to_be_bytes());
        assert_eq!(b[32], 0x41); // EmailHash (4,1) header
        assert_eq!(&b[33..49], &[0u8; 16]); // hash128 default: zeroed
        assert_eq!(&b[49..51], &[0x50, 0x11]); // InvoiceID (5,17) header
        assert_eq!(&b[51..83], &[0u8; 32]); // hash256 default: zeroed
        assert_eq!(&b[116..118], &[0x30, 0x10]); // TransactionResult (16,3) header
        assert_eq!(b[118], 0xAB); // u8 default
        assert_eq!(&b[119..121], &[0x10, 0x11]); // TakerPaysCurrency (17,1) header
        assert_eq!(&b[121..141], &[0u8; 20]); // hash160 default: zeroed
        assert_eq!(&b[141..143], &[0x50, 0x18]); // ClaimCurrency (24,5) header
        assert_eq!(&b[143..163], &[0u8; 20]); // native_issue default: zeroed
        assert_eq!(&b[163..165], &[0x10, 0x1A]); // BaseAsset (26,1) header
        assert_eq!(&b[165..185], &[0u8; 20]); // currency default: zeroed
        assert_eq!(PerKindFixture::LEN, 185 + EMIT_DETAILS_MAX_LEN);
    }

    #[test]
    fn per_kind_fixture_setters_write_at_the_expected_offsets() {
        let mut tpl = PerKindFixture::new();
        tpl.set_signer_weight(0xBEEF);
        assert_eq!(&tpl.bytes()[4..6], &0xBEEFu16.to_be_bytes());

        tpl.set_index_next(0xFFFF_FFFF_FFFF_FFFF);
        assert_eq!(&tpl.bytes()[24..32], &[0xFFu8; 8]);

        tpl.set_email_hash(&[0xAB; 16]);
        assert_eq!(&tpl.bytes()[33..49], &[0xAB; 16]);

        let invoice = crate::types::Hash([0xCD; 32]);
        tpl.set_invoice_id(&invoice);
        assert_eq!(&tpl.bytes()[51..83], &[0xCD; 32]);

        tpl.set_transaction_result(0x42);
        assert_eq!(tpl.bytes()[118], 0x42);

        tpl.set_taker_pays_currency(&[0xEF; 20]);
        assert_eq!(&tpl.bytes()[121..141], &[0xEF; 20]);

        let base = CurrencyCode::from_iso(b"EUR");
        tpl.set_base_asset(&base);
        assert_eq!(&tpl.bytes()[165..185], base.as_ref());

        // Exercise every required-field setter too (dead-code hygiene).
        tpl.set_sequence(0);
        tpl.set_first_ledger_sequence(0);
        tpl.set_last_ledger_sequence(0);
        tpl.set_fee(0).expect("0 drops is in range");
        tpl.set_account(&AccountId::default());
        let _ = tpl.emit_details_region();
        assert_eq!(
            tpl.prepare_for_emit()
                .expect_err("prepare_for_emit must fail on the host stub"),
            crate::error::HookError::NotImplemented
        );
    }

    crate::txn_template! {
        /// `issue(sfX)`'s 40-byte form, exercised standalone (a single
        /// `sfcode` cannot be declared with both `native_issue` and
        /// `issue` in the same template).
        struct IssueFixture {
            transaction_type = ttPAYMENT,
            sequence: u32_field(sfSequence) = 0,
            first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
            last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
            fee: native_amount(sfFee) = 0,
            signing_pub_key: empty_vl(sfSigningPubKey),
            account: account_id(sfAccount),
            claim_currency: issue(sfClaimCurrency),
            emit_details: emit_details,
        }
    }

    #[test]
    fn issue_kind_default_is_zeroed_and_setter_writes_forty_bytes() {
        let mut tpl = IssueFixture::new();
        // Fixed prefix through `sfAccount` matches `TestPayment`'s (same
        // required fields, same order) up to `sfAccount`'s end at offset
        // 77 there; here the layout differs starting from `ClaimCurrency`.
        let off = {
            // header: TransactionType(3) + Sequence(5) + First(6) + Last(6)
            // + Fee(9) + SPK(2) + Account(22) = 53
            53usize
        };
        assert_eq!(&tpl.bytes()[off..off.wrapping_add(2)], &[0x50, 0x18]); // ClaimCurrency (24,5)
        let value_off = off.wrapping_add(2);
        assert_eq!(
            &tpl.bytes()[value_off..value_off.wrapping_add(40)],
            &[0u8; 40]
        );

        let currency = CurrencyCode::from_iso(b"GBP");
        let issuer = AccountId([0x77; ACC_ID_LEN]);
        tpl.set_claim_currency(&currency, &issuer);
        assert_eq!(
            &tpl.bytes()[value_off..value_off.wrapping_add(20)],
            currency.as_ref()
        );
        assert_eq!(
            &tpl.bytes()[value_off.wrapping_add(20)..value_off.wrapping_add(40)],
            issuer.as_ref()
        );

        // Exercise every required-field setter too (dead-code hygiene).
        tpl.set_sequence(0);
        tpl.set_first_ledger_sequence(0);
        tpl.set_last_ledger_sequence(0);
        tpl.set_fee(0).expect("0 drops is in range");
        tpl.set_account(&AccountId::default());
        let _ = tpl.emit_details_region();
        assert_eq!(
            tpl.prepare_for_emit()
                .expect_err("prepare_for_emit must fail on the host stub"),
            crate::error::HookError::NotImplemented
        );
    }

    crate::txn_template! {
        /// `amount(sfX)` with no declared default: the canonical IOU zero
        /// value (`0x80` + 7 zero bytes) with an all-zero currency/issuer.
        struct AmountNoDefaultFixture {
            transaction_type = ttPAYMENT,
            sequence: u32_field(sfSequence) = 0,
            first_ledger_sequence: u32_field(sfFirstLedgerSequence) = 0,
            last_ledger_sequence: u32_field(sfLastLedgerSequence) = 0,
            amount: amount(sfAmount),
            fee: native_amount(sfFee) = 0,
            signing_pub_key: empty_vl(sfSigningPubKey),
            account: account_id(sfAccount),
            emit_details: emit_details,
        }
    }

    #[test]
    fn amount_with_no_default_bakes_canonical_iou_zero() {
        let mut tpl = AmountNoDefaultFixture::new();
        // header: TransactionType(3) + Sequence(5) + First(6) + Last(6) = 20
        let header_off = 20usize;
        assert_eq!(tpl.bytes()[header_off], 0x61); // Amount (6,1) header
        let value_off = header_off.wrapping_add(1);
        #[rustfmt::skip]
        let expected: [u8; 48] = [
            0x80, 0, 0, 0, 0, 0, 0, 0, // issued value: XFL zero
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // currency: zeroed
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // issuer: zeroed
        ];
        assert_eq!(
            &tpl.bytes()[value_off..value_off.wrapping_add(48)],
            &expected[..]
        );

        tpl.set_amount(
            XFL::from_raw_bits(6_107_181_094_714_392_576),
            &CurrencyCode::from_iso(b"JPY"),
            &AccountId([0x11; ACC_ID_LEN]),
        );
        assert_ne!(
            &tpl.bytes()[value_off..value_off.wrapping_add(48)],
            &expected[..]
        );
        tpl.set_amount_value(XFL::from_raw_bits(6_107_031_094_714_392_576));

        // Exercise every required-field setter too (dead-code hygiene).
        tpl.set_sequence(0);
        tpl.set_first_ledger_sequence(0);
        tpl.set_last_ledger_sequence(0);
        tpl.set_fee(0).expect("0 drops is in range");
        tpl.set_account(&AccountId::default());
        let _ = tpl.emit_details_region();
        assert_eq!(
            tpl.prepare_for_emit()
                .expect_err("prepare_for_emit must fail on the host stub"),
            crate::error::HookError::NotImplemented
        );
    }

    // -----------------------------------------------------------------
    // `encode_iou_amount_value_const`: hand-derived reference vectors
    // (rshooks-testenv is not a dev-dependency of this crate, so these are
    // verified by hand against the XFL/`STAmount` bit-layout writeup in
    // `docs/TXN_TEMPLATE_FIELDS_DESIGN.md` §2.2, not against a second
    // encoder).
    // -----------------------------------------------------------------

    #[test]
    fn encode_iou_amount_value_const_zero() {
        assert_eq!(
            codec::encode_iou_amount_value_const(XFL::from_raw_bits(0)),
            [0x80, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn encode_iou_amount_value_const_one() {
        // XFL!(1): mantissa 1_000_000_000_000_000, exponent -15 -> biased
        // 82; sign bit (bit 62) set for positive. Raw bits
        // 0x5483_8D7E_A4C6_8000; OR bit 63 -> 0xD483_8D7E_A4C6_8000.
        assert_eq!(
            codec::encode_iou_amount_value_const(XFL::from_raw_bits(6_089_866_696_204_910_592)),
            [0xD4, 0x83, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00]
        );
    }

    #[test]
    fn encode_iou_amount_value_const_negative_one() {
        // XFL!(-1): identical mantissa/exponent bits as XFL!(1), sign bit
        // (bit 62) clear. Raw bits 0x1483_8D7E_A4C6_8000; OR bit 63 ->
        // 0x9483_8D7E_A4C6_8000.
        assert_eq!(
            codec::encode_iou_amount_value_const(XFL::from_raw_bits(1_478_180_677_777_522_688)),
            [0x94, 0x83, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00]
        );
    }

    #[test]
    fn encode_iou_amount_value_const_minimum_exponent() {
        // The canonical minimum unbiased exponent, -96 (stored field 1: -96
        // + 97), minimum mantissa 1_000_000_000_000_000, positive (sign bit
        // 62 set). Raw bits: (1 << 62) | (1 << 54) | 1_000_000_000_000_000
        // = 0x4043_8D7E_A4C6_8000; OR bit 63 -> 0xC043_8D7E_A4C6_8000.
        assert_eq!(
            codec::encode_iou_amount_value_const(XFL::from_raw_bits(4_630_700_416_936_869_888)),
            [0xC0, 0x43, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00]
        );
    }

    #[test]
    fn encode_iou_amount_value_const_maximum_exponent() {
        // The canonical maximum unbiased exponent, 80 (stored field 177: 80
        // + 97), minimum mantissa 1_000_000_000_000_000, positive. Raw
        // bits: (1 << 62) | (177 << 54) | 1_000_000_000_000_000 =
        // 0x6C43_8D7E_A4C6_8000; OR bit 63 -> 0xEC43_8D7E_A4C6_8000.
        assert_eq!(
            codec::encode_iou_amount_value_const(XFL::from_raw_bits(7_801_234_554_605_699_072)),
            [0xEC, 0x43, 0x8D, 0x7E, 0xA4, 0xC6, 0x80, 0x00]
        );
    }
}
