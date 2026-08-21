//! A multi-hook chain: two `#[hook(..)]` entries at distinct indices, one
//! paired with a `#[cbak(..)]`, exercising `on`/`on_incoming`+`on_outgoing`.

use rshooks::exit::{Accept, HookResult};
use rshooks::hooks;

#[hooks(description = "two-entry chain with a callback")]
struct Vault;

#[hooks]
impl Vault {
    /// The first entry, with a callback.
    #[hook(0, name = "first", on = [Invoke])]
    fn first(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }

    #[cbak(0)]
    fn first_cbak(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }

    /// The second entry, directional and able to emit `Payment`.
    #[hook(1, name = "second", on_incoming = [Payment], on_outgoing = [Invoke], can_emit = [Payment])]
    fn second(&self) -> HookResult {
        Ok(Accept::from_code(0))
    }
}

fn main() {}
