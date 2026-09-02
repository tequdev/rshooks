//! The declared-name charset rule shared by the interface drafts.
//!
//! Both `crate::sig` (Hook Parameter Signature Interface) and `crate::si`
//! (Hook State Interface) declare the same display/field-name rule for
//! their wire-format names — this is the one shared copy. Plain-text
//! paths, not intra-doc links: each module is feature-gated, so a link is
//! unresolvable in any build that excludes it.

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
