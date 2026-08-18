#![no_std]

use rshooks::*;

hook_errors! {
    /// Errors returned by the counter hook.
    pub enum StateCounterError {
        /// The updated counter could not be stored.
        StateSetFailed = 1,
    }
}

#[hooks]
pub struct StateCounter {
    /// Persistent invocation counter, stored at the fixed key `"counter"`.
    #[state(key = b"counter")]
    counter: State<u64>,
}

#[hooks]
impl StateCounter {
    /// Increments the persistent counter and accepts with the new count.
    #[hook(0, on = [Invoke])]
    fn main(&self) -> i64 {
        // Falls back to 0 on any read failure, not just absence — see
        // example 12's `config()` helper for the same masking behavior.
        let count = self.counter.get().unwrap_or(Some(0)).unwrap_or(0);

        let next = count.wrapping_add(1);
        if self.counter.set(&next).is_err() {
            rollback!(
                b"state-counter: state_set failed",
                StateCounterError::StateSetFailed
            );
        }

        accept!(b"state-counter: incremented", next as i64)
    }
}
