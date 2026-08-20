#![no_std]

use rshooks::exit::{Accept, HookResult};
use rshooks::*;

#[hooks(description = "Accepts every transaction selected by HookOn.")]
pub struct AcceptAll;

#[hooks]
impl AcceptAll {
    /// Accepts every triggering transaction.
    #[hook(0, name = "accept", on = [Invoke])]
    fn main(&self) -> HookResult {
        trace!(b"accept-all: accepting transaction");
        Ok(Accept::from_code(0))
    }
}
