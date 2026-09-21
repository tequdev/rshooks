//! A `#[cbak(<index>)]` fn may declare one argument after `&self` — the
//! callback outcome, populated from the host's `cbak(u32)` argument via
//! `Into::into`. Index 0's cbak takes the typed `EmitOutcome`; index 1's
//! takes the raw `u32` — both forms compile and run.

use rshooks::hooks;
use rshooks::prelude::*;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }

    #[cbak(0)]
    fn cbak(&self, outcome: EmitOutcome) -> HookResult {
        match outcome {
            EmitOutcome::Applied => Ok(Accept::from_code(0)),
            EmitOutcome::EmitFailure => Ok(Accept::from_code(1)),
        }
    }

    #[hook(1, on = [Invoke])]
    fn second(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }

    #[cbak(1)]
    fn second_cbak(&self, what: u32) -> HookResult {
        Ok(Accept::from_code(what as i64))
    }
}

fn main() {}
