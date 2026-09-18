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
