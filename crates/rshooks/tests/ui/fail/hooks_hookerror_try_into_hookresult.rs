//! `?` on a raw `rshooks::error::Result` / `HookError` inside a
//! `-> HookResult` entry is E0277: there is no `From<HookError> for
//! Rollback`. `HookError::code` is a 46-arm re-encode; converting through
//! it is the 3.1× WCE path this crate forbids. Map at the call site
//! instead (`.map_err(|_| MyError::…)?`).

use rshooks::exit::{Accept, HookResult};
use rshooks::hooks;

#[hooks]
struct Vault;

fn helper() -> rshooks::error::Result<u8> {
    Ok(0)
}

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let _ = helper()?;
        Ok(Accept::from_code(0))
    }
}

fn main() {}
