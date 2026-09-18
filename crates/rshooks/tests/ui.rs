//! Compile-time diagnostic tests, driven by `trybuild` — primarily the
//! `#[hooks]` chain-declaration macro's rejection catalog (see
//! `docs/MULTI_HOOK_STRUCT_DESIGN.md` §4.4), plus a handful of unrelated
//! diagnostics (typed slot API, `XFL!`, `txn_template!`) that predate it and
//! share this same harness.
//!
//! Every fixture under `tests/ui/fail/` is compiled and its **exact** rustc
//! output compared against the committed `.stderr` file beside it; every
//! fixture under `tests/ui/pass/` must compile *and run* cleanly. This is
//! the only kind of test that can cover a declaration macro's rejections at
//! all — a `compile_error!` has no runtime for an ordinary `#[test]` to
//! observe, and `compile_fail` doctests prove only that *something* failed,
//! not that the caller was told anything useful.
//!
//! The Hook Parameter Signature Interface draft (`docs/PARAM_SIGNATURE_DESIGN.md`)
//! sits behind the `unstable-param-sig-interface` feature, so its fixtures
//! live outside the always-run `fail`/`pass` directories above, in one of
//! two feature-conditional directories: with the feature on,
//! `tests/ui/sig/fail/*.rs` and `tests/ui/sig/pass/*.rs` exercise the
//! interface's own rejections and its one worked example, against a real
//! `rshooks::sig`. With the feature off, `tests/ui/no_sig_feature/*.rs`
//! pins the feature-hint diagnostic a `#[hook(..)]`/`#[cbak(..)]` extra
//! argument gets instead.
//!
//! The Hook State Interface draft (`docs/STATE_INTERFACE_DESIGN.md`) sits
//! behind the `unstable-state-interface` feature, mirroring the same
//! feature-conditional split: `tests/ui/si/fail/*.rs`/`tests/ui/si/pass/*.rs`
//! with the feature on, `tests/ui/no_si_feature/*.rs` with it off.
//!
//! What the pinned `.stderr` files are really guarding is the **wording and
//! the span** of each diagnostic — a vaguer, technically-correct error
//! would leave the caller staring at a macro expansion they did not write.
//! Regenerate them deliberately, with `TRYBUILD=overwrite cargo test -p
//! rshooks --test ui`, and read the diff — an unexpected span change is the
//! signal this file exists to catch.
//!
//! Fixtures are named per rule (`hooks_duplicate_hook_index.rs`,
//! `hooks_self_receiver.rs`, ...), one rule per file: `#[hooks]` bails out
//! of the whole struct/impl expansion on its first validation failure
//! (unlike the legacy declaration macros this replaced, which could report
//! several independent failures from one invocation), so grouping multiple
//! rules into one fixture would only ever exercise the first.
//!
//! Fixture output is toolchain-specific; this repo pins one stable version
//! in `rust-toolchain.toml`, which is what makes byte-exact comparison
//! practical here.

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/fail/*.rs");
    t.pass("tests/ui/pass/*.rs");
    if cfg!(feature = "unstable-param-sig-interface") {
        t.compile_fail("tests/ui/sig/fail/*.rs");
        t.pass("tests/ui/sig/pass/*.rs");
    } else {
        t.compile_fail("tests/ui/no_sig_feature/*.rs");
    }
    if cfg!(feature = "unstable-state-interface") {
        t.compile_fail("tests/ui/si/fail/*.rs");
        t.pass("tests/ui/si/pass/*.rs");
    } else {
        t.compile_fail("tests/ui/no_si_feature/*.rs");
    }
}
