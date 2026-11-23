#![no_std]

use rshooks::exit::{Accept, HookResult};
use rshooks::prelude::*;
use rshooks::*;

/// Sides of the die this hook rolls.
const SIDES: u32 = 6;

hook_errors! {
    /// Rollback codes returned by this hook.
    pub enum RngError {
        /// The ledger holds too little entropy to draw from.
        NoEntropy = 1,
        /// A draw failed for another reason.
        DrawFailed = 2,
        /// A result could not be stored.
        StateSetFailed = 3,
    }
}

#[hooks(description = "Rolls a die and draws random bytes from the consensus RNG.")]
pub struct Rng {
    /// The most recent roll, in `0..SIDES`.
    #[state(key = b"roll")]
    roll: State<u64>,
    /// The most recent 8 random bytes, big-endian.
    #[state(key = b"bits")]
    bits: State<u64>,
}

/// Rolls back with the code and message for a failed draw.
fn draw_failed(e: HookError) -> ! {
    match e {
        HookError::TooLittleEntropy => rollback!(b"rng: no entropy", RngError::NoEntropy),
        _ => rollback!(b"rng: draw failed", RngError::DrawFailed),
    }
}

#[hooks]
impl Rng {
    /// Rolls a die and draws 8 random bytes, stores the roll and the bytes,
    /// and accepts with the roll. Rolls back with [`RngError::NoEntropy`] when the ledger has
    /// no usable entropy.
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let roll = match dice(SIDES) {
            Ok(v) => v,
            Err(e) => draw_failed(e),
        };

        let mut buf = [0u8; 8];
        if let Err(e) = util_random(&mut buf) {
            draw_failed(e);
        }
        let bits = u64::from_be_bytes(buf);

        if self.state.roll.set(&roll).is_err() || self.state.bits.set(&bits).is_err() {
            rollback!(b"rng: state_set failed", RngError::StateSetFailed);
        }

        Ok(Accept::new(b"rng: rolled", roll as i64))
    }
}
