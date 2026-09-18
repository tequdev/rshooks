//! `HookStatic` under `testenv`: take-once-per-invocation, a fresh buffer
//! across separate `invoke` calls, and independence across threads running
//! separate `TestEnv`s concurrently (proving the thread-local backend
//! design, design §4/§7).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::arithmetic_side_effects,
    missing_docs
)]

use rshooks::exit::HookResult;
use rshooks::static_cell::HookStatic;
use rshooks::*;
use rshooks_testenv::prelude::*;

static SCRATCH: HookStatic<[u8; 4]> = HookStatic::new([0, 0, 0, 0]);

#[hooks]
pub struct StaticUser {
    #[state(key = b"scratch")]
    scratch: State<u64>,
}

#[hooks]
impl StaticUser {
    /// Takes the static cell twice: the second take within the same
    /// invocation must be `None`. Reports `10` when the first take succeeds
    /// and the second does not, anything else otherwise.
    #[hook(0, on = [Invoke])]
    fn take_twice(&self) -> HookResult {
        let first = SCRATCH.take();
        let second = SCRATCH.take();
        let code = i64::from(first.is_some()) * 10 + i64::from(second.is_some());
        accept!(b"", code)
    }

    /// Takes the cell once and mutates it, proving the leaked clone is a
    /// genuinely independent, writable buffer.
    #[hook(1, on = [Invoke])]
    fn take_and_mutate(&self) -> HookResult {
        match SCRATCH.take() {
            Some(buf) => {
                buf[0] = 0xAB;
                accept!(b"", 0)
            }
            None => accept!(b"", 1),
        }
    }
}

#[test]
fn second_take_within_one_invocation_is_none() {
    let env = TestEnv::new();
    let exit = env.invoke::<StaticUser>(0);
    assert_eq!(
        exit.code, 10,
        "first take must succeed, second must be None"
    );
}

#[test]
fn every_invocation_gets_a_fresh_buffer() {
    let env = TestEnv::new();
    let first = env.invoke::<StaticUser>(0);
    let second = env.invoke::<StaticUser>(0);
    assert_eq!(first.code, 10);
    assert_eq!(
        second.code, 10,
        "the second invoke must see a fresh, untaken buffer again"
    );
}

#[test]
fn the_leaked_clone_is_independently_mutable() {
    let env = TestEnv::new();
    let exit = env.invoke::<StaticUser>(1);
    assert_eq!(exit.code, 0);
}

#[test]
fn parallel_threads_are_independent() {
    let handles: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                let env = TestEnv::new();
                let exit = env.invoke::<StaticUser>(0);
                exit.code
            })
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().expect("thread panicked"), 10);
    }
}
