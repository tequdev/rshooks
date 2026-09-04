//! Regression test for the `rshooks` dependency-rename bug: `rshooks`'s
//! generated code used to hardcode `::rshooks::` paths, which broke as
//! soon as a consumer renamed the dependency (`hooks = { package =
//! "rshooks", .. }`). `tests/rename-fixture` is a standalone crate (its own
//! `[workspace]`, so it never joins this repo's workspace) that depends on
//! `rshooks` exactly that way and exercises every derive/attribute/macro
//! this crate implements.
//!
//! `trybuild` can't express this: it derives its `--extern` flags from the
//! host crate's own dependencies by their real name, so it has no way to
//! expose `rshooks` under an alias. Shelling out to a real `cargo check`
//! against an independent fixture crate is the only way to prove the
//! renamed dependency actually resolves.

use std::io;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn renamed_dependency_still_compiles() -> io::Result<()> {
    let fixture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/rename-fixture");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());

    let output = Command::new(&cargo)
        .arg("check")
        .current_dir(&fixture_dir)
        .output()?;

    assert!(
        output.status.success(),
        "cargo check on the renamed-dependency fixture failed:\n--- stdout ---\n{}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    Ok(())
}
