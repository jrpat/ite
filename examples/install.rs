//! Installs the release binary to `$XDG_BIN_HOME/ite` (default
//! `~/.local/bin/ite`). Run via `cargo local-bin`.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("run via cargo"));
    let status = Command::new("cargo")
        .args(["build", "--release", "--bin", "ite", "-q"])
        .current_dir(&root)
        .status()
        .expect("cargo build");
    assert!(status.success(), "release build failed");

    let dest_dir = std::env::var_os("XDG_BIN_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME is not set")).join(".local/bin")
        });
    std::fs::create_dir_all(&dest_dir)
        .unwrap_or_else(|e| panic!("cannot create {}: {e}", dest_dir.display()));
    let dest = dest_dir.join("ite");
    // Unlink first so replacing a currently-running binary works.
    let _ = std::fs::remove_file(&dest);
    // fs::copy preserves the source's executable permissions.
    std::fs::copy(root.join("target/release/ite"), &dest)
        .unwrap_or_else(|e| panic!("cannot install to {}: {e}", dest.display()));

    let version = Command::new(&dest).arg("--version").output().expect("run installed binary");
    println!(
        "installed {} -> {}",
        String::from_utf8_lossy(&version.stdout).trim(),
        dest.display()
    );
}
