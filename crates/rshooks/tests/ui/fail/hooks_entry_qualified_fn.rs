//! A hook entry function (`#[hook]`/`#[cbak]`) must be a plain `fn` — a
//! qualifier like `const`/`async`/`unsafe`/`extern "..."` is rejected, even
//! though the same qualifiers are fine on a non-entry helper function.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    const fn main() -> i64 {
        0
    }
}

fn main() {}
