//! Non-entry helper functions inside a `#[hooks] impl` block may carry
//! `const`/`unsafe`/`extern "C"`/`async` qualifiers (unlike an entry
//! function, which must be a plain `fn`) — they pass through untouched.
//! They may also take `&self` (HOOKS_SELF_RECEIVER_DESIGN.md §3.3),
//! including combined with a qualifier where that's syntactically valid.

use rshooks::exit::{Accept, HookResult};
use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> HookResult {
        let a = Self::helper_const();
        let b = unsafe { Self::helper_unsafe() };
        let c = Self::helper_extern();
        let d = Vault.helper_self_ref();
        let e = unsafe { Vault.helper_self_ref_unsafe() };
        Ok(Accept::from_code(
            i64::from(a) + i64::from(b) + i64::from(c) + i64::from(d) + i64::from(e),
        ))
    }

    const fn helper_const() -> u8 {
        1
    }

    unsafe fn helper_unsafe() -> u8 {
        2
    }

    extern "C" fn helper_extern() -> u8 {
        3
    }

    #[allow(dead_code)]
    async fn helper_async() -> u8 {
        4
    }

    /// `&self` helper, called via `self.` (§3.3).
    fn helper_self_ref(&self) -> u8 {
        5
    }

    /// `&self` combined with a qualifier — still syntactically valid.
    unsafe fn helper_self_ref_unsafe(&self) -> u8 {
        6
    }
}

fn main() {
    // Never `.await`ed — just proving the qualified `fn` item was parsed
    // and re-emitted as a real associated function, not consumed or
    // misclassified as an associated `const`.
    let _future = Vault::helper_async();
}
