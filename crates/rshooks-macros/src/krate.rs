//! Resolves what name generated code should use to reach `rshooks`.
//!
//! Every derive/attribute/macro in this crate is re-exported as
//! `rshooks::...`, and every generator here builds its output as source
//! text (see [`crate::hook_data`]'s module doc comment for why), so they
//! have historically hardcoded `::rshooks::` on the assumption that every
//! invoking crate depends on `rshooks` under that exact name. Two cases
//! break that assumption: a consumer that renames the dependency (`hooks =
//! { package = "rshooks", .. }`), and `rshooks`'s own crate-internal use of
//! these macros, where there is no `::rshooks` extern crate at all — only
//! `crate`. [`resolve`] answers "what should generated code call it here?"
//! by asking `proc-macro-crate` what the invoking crate's `Cargo.toml`
//! actually says.

use proc_macro::{Ident, Punct, Spacing, Span, TokenTree};
use proc_macro_crate::{FoundCrate, crate_name};

/// How generated code should reach `rshooks` from the crate currently
/// invoking this macro.
pub(crate) enum KratePath {
    /// The macro is being expanded while compiling `rshooks` itself (its
    /// own doctests excepted — those depend on `rshooks` like any other
    /// crate): the base path is `crate`, not `::rshooks`.
    ItSelf,
    /// The invoking crate depends on `rshooks` under `name` (`"rshooks"`
    /// unless renamed via `package = ".."`): the base path is `::name`.
    Named(String),
}

/// Resolves [`KratePath`] for the crate currently being compiled, via
/// `CARGO_MANIFEST_DIR`. Falls back to `::rshooks` (the previous hardcoded
/// behavior) when resolution fails, e.g. outside a normal Cargo build.
pub(crate) fn resolve() -> KratePath {
    match crate_name("rshooks") {
        Ok(FoundCrate::Itself) if is_rustdoc_test() => {
            // A doctest belonging to `rshooks` is still an external crate
            // that depends on `rshooks` under its real name (every doc
            // example writes `use rshooks::..`) — `proc-macro-crate` can't
            // tell, since it keys "itself" detection on `CARGO_TARGET_TMPDIR`
            // (https://github.com/bkchr/proc-macro-crate), which rustdoc
            // does not set for doctests. `CARGO_CRATE_NAME` is set
            // correctly either way.
            KratePath::Named(
                std::env::var("CARGO_CRATE_NAME").unwrap_or_else(|_| "rshooks".to_string()),
            )
        }
        Ok(FoundCrate::Itself) => KratePath::ItSelf,
        Ok(FoundCrate::Name(name)) => KratePath::Named(name),
        Err(_) => KratePath::Named("rshooks".to_string()),
    }
}

/// Whether the current compilation is a doctest — rustdoc sets this env var
/// (undocumented, but stable across recent toolchains) to point panics back
/// at the original doc comment; nothing else sets it.
fn is_rustdoc_test() -> bool {
    std::env::var_os("UNSTABLE_RUSTDOC_TEST_PATH").is_some()
}

impl KratePath {
    /// The code-path prefix text, trailing `::` included (e.g. `"crate::"`,
    /// `"::rshooks::"`, `"::hooks::"`).
    fn code_prefix(&self) -> String {
        match self {
            KratePath::ItSelf => "crate::".to_string(),
            KratePath::Named(name) => format!("::{name}::"),
        }
    }

    /// The rustdoc intra-doc-link prefix text, trailing `::` included, for
    /// use right after a backtick (e.g. `` "crate::" ``, `` "hooks::" ``) —
    /// intra-doc paths never take a leading `::`.
    fn doc_prefix(&self) -> String {
        match self {
            KratePath::ItSelf => "crate::".to_string(),
            KratePath::Named(name) => format!("{name}::"),
        }
    }
}

/// Rewrites every hardcoded `::rshooks::` code path and `` `rshooks:: ``
/// doc-link prefix in `src` (a generator's fully assembled source-text
/// output, about to be `.parse::<TokenStream>()`-ed) to match what this
/// invocation's `Cargo.toml` actually calls the `rshooks` dependency.
pub(crate) fn rewrite(src: String) -> String {
    let path = resolve();
    src.replace("::rshooks::", &path.code_prefix())
        .replace("`rshooks::", &format!("`{}", path.doc_prefix()))
}

/// Appends the resolved `rshooks` base path (trailing path separator
/// included) as real `TokenTree`s, for generators that splice tokens
/// directly rather than building source text — see
/// [`crate::hooks_impl::build_entry_return_assertion`], the one call site
/// that needs span-carrying tokens rather than a reparsed string.
pub(crate) fn extend_path_tokens(out: &mut Vec<TokenTree>, span: Span) {
    let path_sep = |out: &mut Vec<TokenTree>| {
        out.push(TokenTree::Punct(Punct::new(':', Spacing::Joint)));
        out.push(TokenTree::Punct(Punct::new(':', Spacing::Alone)));
    };
    match resolve() {
        KratePath::ItSelf => {
            out.push(TokenTree::Ident(Ident::new("crate", span)));
        }
        KratePath::Named(name) => {
            path_sep(out);
            out.push(TokenTree::Ident(Ident::new(&name, span)));
        }
    }
    path_sep(out);
}
