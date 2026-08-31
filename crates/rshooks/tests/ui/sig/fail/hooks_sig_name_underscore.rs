//! A signature parameter's display name must match the Hook Parameter
//! Signature Interface's `[A-Za-z][A-Za-z0-9]*` charset
//! (`docs/PARAM_SIGNATURE_DESIGN.md` §1) — a Rust identifier containing `_`
//! is a compile error. The diagnostic itself is built with the
//! identifier's own span (`is_valid_sig_arg_name`'s call site in
//! `crates/rshooks-macros/src/hooks_impl.rs`), but the pinned `.stderr`
//! below reports it at `#[hooks]` (column 1) regardless — an
//! attribute-macro diagnostic built from a plain token stream (not
//! `proc_macro::Diagnostic`, nightly-only) always renders at the macro's
//! own invocation site on stable, whatever span the token that carries the
//! message was given.

use rshooks::hooks;

#[hooks]
struct Vault;

#[hooks]
impl Vault {
    #[hook(0, on = [Invoke])]
    fn main(&self, my_count: u16) -> i64 {
        i64::from(my_count)
    }
}

fn main() {}
