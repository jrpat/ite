//! Repository-level contract tests for packaging and release infrastructure.
//! They inspect manifests, generated workflows, packaged file lists, installed
//! binary metadata, and the release skill without coupling product modules to
//! repository layout.
//!
//! This is also the home for structural source-policy checks that apply across
//! crates, examples, and tests rather than to one runtime module.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: impl AsRef<Path>) -> String {
    let path = root().join(path);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn parse_toml(path: impl AsRef<Path>) -> toml::Value {
    let text = read(path);
    toml::from_str(&text).expect("valid TOML")
}

fn strings(value: &toml::Value) -> Vec<&str> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|value| value.as_str().expect("string"))
        .collect()
}

fn assert_contains(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "expected text to contain {needle:?}"
    );
}

fn rust_files_under(path: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(path).expect("source directory") {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            rust_files_under(&path, files);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

#[test]
fn rust_files_begin_with_a_map_of_their_territory() {
    let mut files = Vec::new();
    for directory in ["src", "examples", "tests"] {
        rust_files_under(&root().join(directory), &mut files);
    }

    let undocumented: Vec<_> = files
        .iter()
        .filter(|path| {
            !std::fs::read_to_string(path)
                .expect("Rust source")
                .starts_with("//!")
        })
        .map(|path| path.strip_prefix(root()).expect("file under root"))
        .collect();

    assert!(
        undocumented.is_empty(),
        "Rust files without a module-level prelude: {undocumented:#?}"
    );
}

#[test]
fn package_identity_and_metadata_are_ready_to_publish() {
    let manifest = parse_toml("Cargo.toml");
    let package = manifest["package"].as_table().expect("package table");

    assert_eq!(package["name"].as_str(), Some("ite-cli"));
    assert_eq!(package["version"].as_str(), Some("0.2.2"));
    assert_eq!(package["edition"].as_str(), Some("2024"));
    assert_eq!(package["rust-version"].as_str(), Some("1.88"));
    assert_eq!(package["license"].as_str(), Some("MIT"));
    assert_eq!(
        package["repository"].as_str(),
        Some("https://github.com/jrpat/ite")
    );
    assert_eq!(
        package["homepage"].as_str(),
        Some("https://github.com/jrpat/ite")
    );
    assert_eq!(package["readme"].as_str(), Some("README.md"));
    assert!(
        !package["description"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "package description must be present"
    );
    assert!(
        strings(&package["keywords"]).contains(&"tui"),
        "keywords should identify this as a TUI"
    );
    assert!(
        strings(&package["categories"]).contains(&"command-line-utilities"),
        "crate must use the command-line-utilities category"
    );

    let binaries = manifest["bin"].as_array().expect("[[bin]] entries");
    assert_eq!(binaries.len(), 1);
    assert_eq!(binaries[0]["name"].as_str(), Some("ite"));
    assert_eq!(binaries[0]["path"].as_str(), Some("src/main.rs"));
    assert_eq!(manifest["lib"]["doc"].as_bool(), Some(false));

    assert_contains(&read("LICENSE"), "MIT License");
    assert_contains(&read("LICENSE"), "Copyright (c) 2026 Juan Patino");
    assert_contains(&read("CHANGELOG.md"), "## [Unreleased]");
    assert_contains(&read("CHANGELOG.md"), "## [0.1.1] - 2026-07-29");
}

#[test]
fn installed_binary_keeps_the_user_facing_name_and_version() {
    assert_eq!(env!("CARGO_PKG_NAME"), "ite-cli");

    let output = Command::new(env!("CARGO_BIN_EXE_ite"))
        .arg("--version")
        .output()
        .expect("run ite --version");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "ite 0.2.2"
    );
}

#[test]
fn cargo_package_contains_release_material_and_excludes_local_state() {
    let output = Command::new(env!("CARGO"))
        .args(["package", "--list", "--allow-dirty", "--locked"])
        .current_dir(root())
        .output()
        .expect("cargo package --list");
    assert!(
        output.status.success(),
        "cargo package --list failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let files = String::from_utf8(output.stdout).unwrap();

    for required in [
        "Cargo.toml",
        "Cargo.lock",
        "README.md",
        "LICENSE",
        "src/main.rs",
    ] {
        assert!(
            files.lines().any(|line| line == required),
            "{required} is missing from the Cargo package:\n{files}"
        );
    }
    for forbidden in [
        ".my/",
        ".tmp/",
        ".jj/",
        "target/",
        "dist-workspace.toml",
        "docs/",
        "examples/",
        "tests/",
    ] {
        assert!(
            files.lines().all(|line| !line.starts_with(forbidden)),
            "{forbidden} leaked into the Cargo package:\n{files}"
        );
    }
}

#[test]
fn ordinary_ci_is_reusable_and_covers_stable_and_msrv() {
    let workflow = read(".github/workflows/ci.yml");

    assert_contains(&workflow, "workflow_call:");
    assert_contains(&workflow, "push:");
    assert_contains(&workflow, "branches: [main]");
    assert!(
        !workflow.contains("pull_request:"),
        "pull requests call CI through the generated dist workflow"
    );
    assert_contains(&workflow, "stable:");
    assert_contains(&workflow, "msrv:");
    assert_contains(&workflow, "toolchain: 1.88.0");
    assert_contains(&workflow, "cargo fmt --check");
    assert_contains(&workflow, "cargo clippy --all-targets -- -D warnings");
    assert_contains(&workflow, "cargo test --locked");
    assert_contains(&workflow, "cargo publish --dry-run --locked");
    assert_contains(&workflow, "cargo install --path . --locked");
    assert_contains(&workflow, "--version");
    assert_contains(&workflow, "--help");
}

#[test]
fn dist_configuration_matches_the_distribution_contract() {
    let config = parse_toml("dist-workspace.toml");
    assert_eq!(strings(&config["workspace"]["members"]), ["cargo:."]);

    let dist = config["dist"].as_table().expect("dist table");
    assert_eq!(dist["cargo-dist-version"].as_str(), Some("0.32.0"));
    assert_eq!(strings(&dist["ci"]), ["github"]);
    assert_eq!(strings(&dist["hosting"]), ["github"]);
    assert_eq!(strings(&dist["installers"]), ["shell", "homebrew"]);
    assert_eq!(
        strings(&dist["targets"]),
        [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "aarch64-unknown-linux-musl",
            "x86_64-unknown-linux-musl",
        ]
    );
    assert_eq!(
        strings(&dist["install-path"]),
        ["$XDG_BIN_HOME/", "~/.local/bin"]
    );
    assert_eq!(dist["checksum"].as_str(), Some("sha256"));
    assert_eq!(dist["tap"].as_str(), Some("jrpat/homebrew-ite-tap"));
    assert_eq!(dist["formula"].as_str(), Some("ite"));
    assert_eq!(dist["display-name"].as_str(), Some("Ite"));
    assert_eq!(dist["dispatch-releases"].as_bool(), Some(true));
    assert_eq!(dist["create-release"].as_bool(), Some(false));
    assert_eq!(dist["pr-run-mode"].as_str(), Some("plan"));
    assert_eq!(dist["github-attestations"].as_bool(), Some(true));
    assert_eq!(dist["source-tarball"].as_bool(), Some(false));
    assert_eq!(dist["install-updater"].as_bool(), Some(false));
    assert_eq!(dist["publish-prereleases"].as_bool(), Some(false));
    assert_eq!(strings(&dist["plan-jobs"]), ["./ci"]);
    assert_eq!(
        strings(&dist["global-artifacts-jobs"]),
        ["./smoke-artifacts"]
    );
    assert_eq!(strings(&dist["publish-jobs"]), ["./publish-homebrew"]);
    assert_eq!(strings(&dist["post-announce-jobs"]), ["./publish-crates"]);

    let permissions = dist["github-custom-job-permissions"]
        .as_table()
        .expect("custom job permissions");
    assert_eq!(permissions["ci"]["contents"].as_str(), Some("read"));
    assert_eq!(
        permissions["smoke-artifacts"]["contents"].as_str(),
        Some("read")
    );
    assert_eq!(
        permissions["publish-crates"]["contents"].as_str(),
        Some("read")
    );
    assert_eq!(
        permissions["publish-crates"]["id-token"].as_str(),
        Some("write")
    );
    assert_eq!(
        permissions["publish-homebrew"]["contents"].as_str(),
        Some("read")
    );
}

#[test]
fn generated_release_workflow_supports_safe_dry_runs() {
    let workflow = read(".github/workflows/release.yml");

    assert_contains(&workflow, "This file was autogenerated by dist");
    assert_contains(&workflow, "pull_request:");
    assert_contains(&workflow, "workflow_dispatch:");
    assert_contains(&workflow, "default: dry-run");
    assert_contains(&workflow, "inputs.tag == 'dry-run'");
    assert_contains(&workflow, "custom-ci:");
    assert_contains(&workflow, "custom-smoke-artifacts:");
    assert_contains(&workflow, "custom-publish-homebrew:");
    assert_contains(&workflow, "custom-publish-crates:");
    assert_contains(&workflow, "needs:\n      - plan\n      - announce");
}

#[test]
fn native_artifacts_are_smoke_tested_before_hosting() {
    let workflow = read(".github/workflows/smoke-artifacts.yml");

    assert_contains(&workflow, "workflow_call:");
    assert_contains(
        &workflow,
        "fromJSON(inputs.plan).ci.github.artifacts_matrix",
    );
    assert_contains(&workflow, "runs-on: ${{ matrix.runner }}");
    assert_contains(
        &workflow,
        "artifacts-build-local-${{ join(matrix.targets, '_') }}",
    );
    assert_contains(&workflow, "find unpacked -type f -name CHANGELOG.md");
    assert_contains(&workflow, "find unpacked -type f -name LICENSE");
    assert_contains(&workflow, "find unpacked -type f -name README.md");
    assert_contains(&workflow, "ite --version");
    assert_contains(&workflow, "ite --help");
}

#[test]
fn crates_workflow_supports_bootstrap_and_trusted_publication() {
    let workflow = read(".github/workflows/publish-crates.yml");

    assert_contains(&workflow, "workflow_call:");
    assert_contains(&workflow, "environment: release");
    assert_contains(&workflow, "contents: read");
    assert_contains(&workflow, "id-token: write");
    assert_contains(&workflow, "secrets.CARGO_REGISTRY_TOKEN");
    assert_contains(&workflow, "rust-lang/crates-io-auth-action@v1");
    assert_contains(&workflow, "cargo publish --dry-run --locked");
    assert_contains(&workflow, "cargo publish --locked");
    assert_contains(&workflow, "ite-cli");
    assert_contains(&workflow, "already-published");
    assert_contains(&workflow, "https://github.com/jrpat/ite");
}

#[test]
fn crates_workflow_can_verify_trusted_publishing_without_publishing() {
    let workflow = read(".github/workflows/publish-crates.yml");

    assert_contains(&workflow, "workflow_dispatch:");
    assert_contains(&workflow, "oidc_dry_run:");
    assert_contains(&workflow, "verify-oidc:");
    assert_contains(&workflow, "if: ${{ inputs.oidc_dry_run }}");
    assert_contains(&workflow, "name: Verify crates.io OIDC");
    assert_contains(&workflow, "environment: release");
    assert_contains(&workflow, "id-token: write");
    assert_contains(&workflow, "id: trusted");
    assert_contains(&workflow, "uses: rust-lang/crates-io-auth-action@v1");
    assert_contains(
        &workflow,
        "CARGO_REGISTRY_TOKEN: ${{ steps.trusted.outputs.token }}",
    );
    assert_contains(&workflow, "test -n \"$CARGO_REGISTRY_TOKEN\"");
    assert_contains(
        &workflow,
        "if: ${{ !inputs.oidc_dry_run && inputs.plan != '' }}",
    );
}

#[test]
fn homebrew_workflow_is_stable_idempotent_and_tests_the_binary() {
    let workflow = read(".github/workflows/publish-homebrew.yml");

    assert_contains(&workflow, "workflow_call:");
    assert_contains(&workflow, "announcement_is_prerelease");
    assert_contains(&workflow, "jrpat/homebrew-ite-tap");
    assert_contains(&workflow, "secrets.HOMEBREW_TAP_TOKEN");
    assert_contains(&workflow, "artifacts-build-global");
    assert_contains(&workflow, "Formula/ite.rb");
    assert_contains(&workflow, "test do");
    assert_contains(&workflow, "#{bin}/ite --version");
    assert_contains(&workflow, "git diff --quiet");
    assert_contains(&workflow, "Ite release bot");
}

#[test]
fn release_skill_is_discoverable_and_covers_the_release_contract() {
    let skill = read(".agents/skills/ite-release/SKILL.md");
    let normalized_skill = skill.split_whitespace().collect::<Vec<_>>().join(" ");

    for required in [
        "name: ite-release",
        "## Guardrails",
        "empty Jujutsu working-copy commit",
        "recommend the next version and wait for agreement",
        "explicit approval",
        "Release vX.Y.Z",
        "jj bookmark set main -r @-",
        "Do not push a local tag",
        "gh workflow run Release",
        "## Verify the published release",
        "ITE_CLI_UNMANAGED_INSTALL",
        "Never use XDG_BIN_HOME or ITE_CLI_INSTALL_DIR for a temporary verification install",
        "## Recover safely",
        "short-lived OIDC credential",
    ] {
        assert_contains(&normalized_skill, required);
    }
    assert!(
        !skill.contains("TODO"),
        "the release skill must not contain scaffold placeholders"
    );

    let link = root().join("claude/skills/ite-release");
    let metadata = std::fs::symlink_metadata(&link)
        .unwrap_or_else(|error| panic!("failed to inspect {}: {error}", link.display()));
    assert!(
        metadata.file_type().is_symlink(),
        "{} must be a symlink",
        link.display()
    );
    assert_eq!(
        std::fs::read_link(&link).expect("read release skill symlink"),
        Path::new("../../.agents/skills/ite-release")
    );
}
