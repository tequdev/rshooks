//! An entry returning a type implementing neither `i64` nor `HookResult`
//! fails to compile. `EntryReturn` is sealed to exactly those two return
//! shapes (`.claude/design/TYPED_ENTRY_RESULTS_DESIGN.md` §1.3/§3) and the
//! macro does not parse or validate the return type itself — the ordinary
//! trait-bound diagnostic naming `EntryReturn`, from the generated body's
//! `EntryReturn::finish(..)` call, is the whole story.
//!
//! Three entries in the same chain: one valid (`i64`), one with an explicit
//! non-`EntryReturn` return type (`-> u32`), and one with an implicit
//! `-> ()` (`()` doesn't implement `EntryReturn` either). Each bad entry's
//! error lands on that entry's own line — a span-carrying `const _: fn(<ret>)
//! -> i64 = ..;` assertion built from the entry fn's own captured `-> Ty`
//! tokens (see `build_entry_return_assertion` in
//! `rshooks-macros/src/hooks_impl.rs`) is spliced in beside the generated
//! `mod __rshooks_entries`, so the diagnostic doesn't fall back to pointing
//! at the `#[hooks]` attribute the way the generated body's own
//! `EntryReturn::finish(..)` call does (that call is assembled through a
//! `format!` + `TokenStream::parse` round-trip, which only ever produces
//! call-site spans) — hence each bad entry below reports twice, once at its
//! own line (the assertion) and once at `#[hooks]` (the generated body).

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn good(&self) -> i64 {
        0
    }

    #[hook(1, on = [Invoke])]
    fn bad_typed(&self) -> u32 {
        0
    }

    #[hook(2, on = [Invoke])]
    fn bad_missing(&self) {}
}

fn main() {}
