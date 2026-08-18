#![no_std]

use rshooks::*;

#[hooks(description = "Accepts every transaction selected by HookOn.")]
pub struct AcceptAll;

#[hooks]
impl AcceptAll {
    /// Accepts every triggering transaction.
    #[hook(0, name = "accept", on = [Invoke])]
    fn main(&self) -> i64 {
        trace!(b"accept-all: accepting transaction");
        accept!()
    }
}
