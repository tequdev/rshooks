//! The declared-name charset rule and XAS-010d type-code table shared by the
//! interface drafts.
//!
//! Both `crate::sig` (Hook Parameter Signature Interface) and `crate::si`
//! (Hook State Interface) declare the same display/field-name rule for
//! their wire-format names, and the same type codes for the fixed-width
//! types both implement — this is the one shared copy. Plain-text
//! paths, not intra-doc links: each module is feature-gated, so a link is
//! unresolvable in any build that excludes it.

/// XAS-010d type codes for the fixed-width types both
/// `crate::si::SiFieldType` and `crate::sig::SigParamType` implement (see
/// `docs/STATE_INTERFACE_DESIGN.md` §1.5 / `docs/PARAM_SIGNATURE_DESIGN.md`
/// §2's tables). Values match the rippled/xahaud `STI_*` numbering; `XFL`
/// has no `STI_*` counterpart, since it is an XAS-010d-only extension.
pub(crate) mod xas010d {
    pub(crate) const UINT8: u8 = 0x10;
    pub(crate) const UINT16: u8 = 0x01;
    pub(crate) const UINT32: u8 = 0x02;
    pub(crate) const UINT64: u8 = 0x03;
    pub(crate) const UINT128: u8 = 0x04;
    pub(crate) const UINT256: u8 = 0x05;
    pub(crate) const ACCOUNT: u8 = 0x08;
    pub(crate) const UINT160: u8 = 0x11;
    pub(crate) const CURRENCY: u8 = 0x1A;
    pub(crate) const XFL: u8 = 0x80;
}

/// Whether `name` matches the interface drafts' shared charset:
/// `[A-Za-z][A-Za-z0-9]*`, 1..=16 bytes. Every caller is `const { .. }`
/// -evaluated, so this never compiles into hook wasm — it only runs during
/// `rustc`'s own const evaluator.
#[allow(clippy::indexing_slicing)] // in-bounds by the `i < name.len()` loop condition, const-evaluated only
#[must_use]
pub const fn is_valid_name(name: &[u8]) -> bool {
    if name.is_empty() || name.len() > 16 {
        return false;
    }
    let mut i = 0;
    while i < name.len() {
        let b = name[i];
        let ok = if i == 0 {
            b.is_ascii_alphabetic()
        } else {
            b.is_ascii_alphanumeric()
        };
        if !ok {
            return false;
        }
        i = i.wrapping_add(1);
    }
    true
}
