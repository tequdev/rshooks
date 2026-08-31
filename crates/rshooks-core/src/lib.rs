//! Low-level, `no_std` Hook API bindings.
//!
//! This crate exposes C-compatible API declarations and protocol constants.
//! Use `rshooks` for ergonomic Rust wrappers.

#![no_std]
#![allow(non_upper_case_globals)]

pub mod api;
#[cfg(all(not(target_arch = "wasm32"), feature = "testenv"))]
pub mod backend;
pub mod consts;
pub mod error;
pub mod host;
pub mod lets;
pub mod ls_flags;
pub mod sfcodes;
pub mod tts;
pub mod tx_flags;

pub use api::*;
pub use consts::*;
pub use error::*;
pub use lets::*;
pub use ls_flags::*;
pub use sfcodes::*;
pub use tts::*;
pub use tx_flags::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_sfcodes_match_header() {
        assert_eq!(sfAccount, (8u32 << 16) + 1);
        assert_eq!(sfFlags, (2u32 << 16) + 2);
    }

    #[test]
    fn known_tts_match_header() {
        assert_eq!(ttPAYMENT, 0);
        assert_eq!(ttHOOK_SET, 22);
    }

    #[test]
    fn known_errors_match_header() {
        assert_eq!(SUCCESS, 0);
        assert_eq!(OUT_OF_BOUNDS, -1);
        assert_eq!(INVALID_FLOAT, -10024);
        assert_eq!(NOT_IMPLEMENTED, -14);
    }

    #[test]
    fn known_ls_flags_match_header() {
        assert_eq!(lsfGlobalFreeze, 0x0040_0000);
    }

    #[test]
    fn known_tx_flags_match_header() {
        assert_eq!(tfFullyCanonicalSig, 0x8000_0000);
        assert_eq!(tfMPTCanLock, ls_flags::lsfMPTCanLock);
    }

    #[test]
    fn known_consts_match_header() {
        assert_eq!(KEYLET_HOOK, 1);
        assert_eq!(KEYLET_CRON, 26);
        assert_eq!(tfCANONICAL, 0x8000_0000);
    }
}
