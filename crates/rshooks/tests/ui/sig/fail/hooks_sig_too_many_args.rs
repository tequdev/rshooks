//! At most 16 signature parameters are accepted per entry (index
//! `0x00..=0x0F`, `docs/PARAM_SIGNATURE_DESIGN.md` §1) — a 17th argument is
//! a compile error. The diagnostic itself is built from the 17th
//! argument's own span (`parse_sig_args`'s call site in
//! `crates/rshooks-macros/src/hooks_impl.rs`), but — like every other
//! diagnostic this macro emits — the pinned `.stderr` below reports it at
//! `#[hooks]` (column 1) regardless: an attribute-macro diagnostic built
//! from a plain token stream (not `proc_macro::Diagnostic`, nightly-only)
//! always renders at the macro's own invocation site on stable.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(
        &self,
        a0: u8, a1: u8, a2: u8, a3: u8, a4: u8, a5: u8, a6: u8, a7: u8,
        a8: u8, a9: u8, a10: u8, a11: u8, a12: u8, a13: u8, a14: u8, a15: u8,
        a16: u8,
    ) -> i64 {
        i64::from(a16)
    }
}

fn main() {}
