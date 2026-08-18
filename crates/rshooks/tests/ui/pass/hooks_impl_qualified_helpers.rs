//! Non-entry helper functions inside a `#[hooks] impl` block may carry
//! `const`/`unsafe`/`extern "C"`/`async` qualifiers (unlike an entry
//! function, which must be a plain `fn`) — they pass through untouched.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main() -> i64 {
        let a = Self::helper_const();
        let b = unsafe { Self::helper_unsafe() };
        let c = Self::helper_extern();
        i64::from(a) + i64::from(b) + i64::from(c)
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
}

fn main() {
    // Never `.await`ed — just proving the qualified `fn` item was parsed
    // and re-emitted as a real associated function, not consumed or
    // misclassified as an associated `const`.
    let _future = Vault::helper_async();
}
