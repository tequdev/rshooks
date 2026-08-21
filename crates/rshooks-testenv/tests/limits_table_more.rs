//! More boundary tests for design §5's fidelity rules — split from
//! `limits_table.rs` because `#[hooks]` generates crate-global symbols per
//! entry index, so two independent chains need two independent test
//! binaries (`cargo test` already compiles each `tests/*.rs` file as its
//! own crate).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use rshooks::exit::HookResult;
use rshooks::*;
use rshooks_testenv::prelude::*;

#[hooks]
pub struct LimitsMore {
    #[state(key = b"scratch")]
    scratch: State<u64>,
}

#[hooks]
impl LimitsMore {
    #[hook(0, on = [Invoke])]
    fn key_zero_length(&self) -> HookResult {
        let code = match rshooks::api::state::state_set(b"v", &[] as &[u8]) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }

    #[hook(1, on = [Invoke])]
    fn key_too_long(&self) -> HookResult {
        let key = [0u8; 33];
        let code = match rshooks::api::state::state_set(b"v", &key) {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }

    #[hook(2, on = [Invoke])]
    fn value_too_big(&self) -> HookResult {
        let data = [0u8; 257];
        let code = match rshooks::api::state::state_set(&data, b"K") {
            Ok(_) => 0,
            Err(e) => e.code(),
        };
        accept!(b"", code)
    }

    #[hook(3, on = [Invoke])]
    fn key_padding_equivalence(&self) -> HookResult {
        let mut out = [0u8; 8];
        let code = match rshooks::api::state::state(&mut out, b"RR") {
            Ok(n) => i64::from(n as u32),
            Err(e) => e.code(),
        };
        accept!(&out, code)
    }
}

#[test]
fn zero_byte_key_is_too_small() {
    let env = TestEnv::new();
    let exit = env.invoke::<LimitsMore>(0);
    assert_eq!(exit.code, rshooks_core::TOO_SMALL);
}

#[test]
fn oversized_key_is_too_big() {
    let env = TestEnv::new();
    let exit = env.invoke::<LimitsMore>(1);
    assert_eq!(exit.code, rshooks_core::TOO_BIG);
}

#[test]
fn oversized_value_is_too_big() {
    let env = TestEnv::new();
    let exit = env.invoke::<LimitsMore>(2);
    assert_eq!(exit.code, rshooks_core::TOO_BIG);
}

#[test]
fn short_key_and_left_padded_key_address_the_same_entry() {
    let mut padded = [0u8; 32];
    padded[30] = b'R';
    padded[31] = b'R';
    let env = TestEnv::new().state_entry(&padded, &[9, 9, 9, 9]);
    let exit = env.invoke::<LimitsMore>(3);
    assert_eq!(exit.exit, ExitType::Accept);
    assert_eq!(exit.code, 4);
    assert_eq!(&exit.msg[..4], &[9, 9, 9, 9]);
}
