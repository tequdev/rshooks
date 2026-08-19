//! An entry returning a type implementing neither `i64` nor `HookResult`
//! fails to compile. `EntryReturn` is sealed to exactly those two return
//! shapes (`.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md` §1.3/§3) and the
//! macro does not parse or validate the return type itself — the ordinary
//! trait-bound diagnostic naming `EntryReturn`, from the generated body's
//! `EntryReturn::finish(..)` call, is the whole story.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self) -> u32 {
        0
    }
}

fn main() {}
