//! `?` on a raw `rshooks::error::Result` / `HookError` inside a
//! `-> HookResult` entry is E0277: there is no `From<HookError> for
//! Rollback`. A Hook API error code is not the hook's own `HookReturnCode`;
//! converting one implicitly would publish the host's code as the hook's
//! verdict. Map at the call site instead (`.map_err(|_| MyError::…)?`).

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
