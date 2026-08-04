---
name: ite-release
description: Cut, publish, recover, and verify an Ite release through its approved GitHub Actions, crates.io trusted-publishing, and Homebrew pipeline. Use when preparing a stable or beta Ite release, rehearsing the release pipeline, checking release prerequisites, retrying a failed release stage, or auditing a published release.
---

# Release Ite

Run the release from the Ite repository. Treat the checked-in package metadata,
`dist-workspace.toml`, and `.github/workflows/` as the executable authority.
Use `.my/distribution.md` for rationale when a decision is unclear.

## Guardrails

- Use Jujutsu for source-control changes. Never run `git commit`, create a
  release commit with Git, or abandon changes that belong to the user.
- Start from an empty Jujutsu working-copy commit. Stop and ask the user what
  to do if the working copy contains changes.
- Never expose, print, copy, or persist release credentials.
- Do not push a local tag. The approved `Release` workflow publishes the draft
  release and creates the public tag.
- Do not publish a GitHub release, a crate, or a Homebrew formula manually.
- Do not edit the generated release workflow or tap formula by hand. Change
  `dist-workspace.toml` or the custom reusable workflows, regenerate, and
  review the result.
- Do not use or create a `dist` service account. `dist` is a local CLI and
  checked-in workflow generator.
- Treat `CARGO_REGISTRY_TOKEN` as bootstrap-only. Normal releases use a
  short-lived OIDC credential and should not have that secret configured.
- Treat shell-installer verification as ephemeral. Always set
  `ITE_CLI_UNMANAGED_INSTALL` to the temporary binary directory so cargo-dist
  cannot modify shell startup files or persist an install receipt. Never use
  XDG_BIN_HOME or ITE_CLI_INSTALL_DIR for a temporary verification install.
- Stop on an identity, version, commit, checksum, attestation, or asset
  mismatch. Do not improvise past a failed invariant.
- Do not start irreversible release work without explicit approval of the
  version and complete release notes.

## Understand the release shape

Inspect these files before relying on any remembered command or artifact name:

- `Cargo.toml` and `Cargo.lock` for package identity and version
- `CHANGELOG.md` for stable-release history and unreleased changes
- `dist-workspace.toml` for targets, installers, publishing order, tap, and
  the pinned `dist` version
- `.github/workflows/release.yml` for the generated orchestrator
- `.github/workflows/ci.yml`, `smoke-artifacts.yml`,
  `publish-homebrew.yml`, and `publish-crates.yml` for project-owned checks

The expected pipeline is:

1. Build and smoke-test four native archives.
2. Host archives, checksums, installers, and attestations on a draft GitHub
   release.
3. Publish that release and its tag.
4. Publish stable Homebrew formulae.
5. Publish `ite-cli` to crates.io with GitHub OIDC.

Beta releases skip Homebrew because prerelease formula publishing is disabled.

## Preflight the candidate

Perform all checks before recommending a version:

1. Run `jj status` and confirm the working-copy commit is empty.
2. Run `jj git fetch`. Confirm `main`, `main@origin`, and the intended
   candidate resolve as expected, with no divergence or unpushed release work.
3. Inspect the latest GitHub release and the commits since it. Confirm the
   candidate tag, GitHub release, and crates.io version do not already exist.
4. Confirm CI is green for the exact candidate commit.
5. Run the local release checks:

   ```sh
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   cargo test --locked
   cargo publish --dry-run --locked
   dist generate --check
   ```

6. Install the packaged binary into a temporary install root and run
   `ite --version` and `ite --help`. Do not test only the workspace binary.
7. Run `dist plan --tag vX.Y.Z` with the proposed shape once a provisional
   version is available. Confirm all four configured native targets, global
   artifacts, custom jobs, and publish ordering are present.
8. Confirm the following repository settings:
   - GitHub environment `release` exists.
   - `HOMEBREW_TAP_TOKEN` can update `jrpat/homebrew-ite-tap`.
   - crates.io trusted publishing names owner `jrpat`, repository `ite`,
     workflow `publish-crates.yml`, and environment `release`.
   - `CARGO_REGISTRY_TOKEN` is absent after bootstrap.

Some TUI and PTY tests need access to `/dev/tty`; request the necessary
sandbox permission instead of weakening or skipping those tests.

If publishing configuration or trust settings changed, verify OIDC without
publishing:

```sh
gh workflow run publish-crates.yml \
  --repo jrpat/ite \
  --ref main \
  -f oidc_dry_run=true
```

Watch the dispatched run to completion and confirm the `verify-oidc` job
obtained a nonempty short-lived OIDC credential through
`rust-lang/crates-io-auth-action`, then revoked it. A successful dry run must
not run `cargo publish`.

## Agree on the version

Read the changes since the previous release and recommend the next version.
Before 1.0, use:

- MINOR for user-facing features or breaking behavior
- PATCH for fixes that preserve the existing behavior contract
- `-beta.N` for prereleases

After 1.0, apply normal SemVer. `Cargo.toml` is the source of truth. Present
the recommendation and reasoning, then recommend the next version and wait for
agreement. Do not change files merely to explore a version.

After agreement:

1. Update the package version in `Cargo.toml`.
2. Refresh the root package version in `Cargo.lock`.
3. Confirm `cargo run -- --version` prints `ite X.Y.Z`.
4. Run `dist plan --tag vX.Y.Z` and repeat any preflight check affected by the
   version edit.

## Draft and approve the notes

Derive notes from the previous release commit through the candidate commit.
Inspect commit bodies or diffs when subjects do not explain the user impact.

Use this shape:

```markdown
## Highlights

- The few capabilities or improvements most meaningful to an Ite user.

## All changes

- One useful bullet for every landed change worth reporting.

---
**Install**: `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/jrpat/ite/releases/download/vX.Y.Z/ite-cli-installer.sh | sh`
**Homebrew**: `brew install jrpat/ite-tap/ite`
**Cargo**: `cargo install ite-cli --locked`
**Verify**: `gh attestation verify <asset> --repo jrpat/ite`
```

Keep highlights user-facing. Omit dead ends and purely internal details unless
they materially affect users. Put the approved notes in
`.tmp/release-notes.md`, show the user the complete text, and wait for explicit
approval.

For a stable release, copy the approved release content above the footer into
`CHANGELOG.md` under `## [X.Y.Z] - YYYY-MM-DD`, immediately after
`## [Unreleased]`. Reset the Unreleased section for future work.

For a beta, do not add a versioned changelog section. The beta notes live on
the GitHub prerelease.

Approval of the version and complete notes authorizes the remaining standard
pipeline steps below. It does not authorize changing the release contents to
work around a failure.

## Land the release commit

Review the complete diff and rerun affected checks. Commit only the intended
version and changelog changes:

```sh
jj commit -m "Release vX.Y.Z"
```

Jujutsu creates a new empty working-copy commit; the release commit is now
`@-`. Verify its exact commit ID and tree before moving `main`.

```sh
jj bookmark set main -r @-
jj git push --bookmark main
```

Fetch and confirm `main`, the Git view of `main`, and `main@origin` resolve to
the exact release commit. Wait for CI on that exact commit to finish green.

## Create the draft and dispatch

Create one draft GitHub release whose tag, title, target commit, and notes all
match the approved release:

```sh
gh release create vX.Y.Z \
  --repo jrpat/ite \
  --draft \
  --title vX.Y.Z \
  --target RELEASE_COMMIT_SHA \
  --notes-file .tmp/release-notes.md
```

Add `--prerelease` for a beta. Do not push a local tag. Re-read the draft
through the GitHub API and verify all identity fields and the full notes before
dispatch.

Start the checked-in workflow from `main`:

```sh
gh workflow run Release \
  --repo jrpat/ite \
  --ref main \
  -f tag=vX.Y.Z
```

Identify the new run by workflow, event, tag input, and creation time. Watch it
to a terminal result with `gh run watch RUN_ID --exit-status`. Inspect failed
job logs before deciding whether any retry is safe.

## Verify the published release

Do not call the release complete merely because Actions is green. Verify every
public channel independently:

1. GitHub release
   - It is public, with the correct prerelease flag and approved notes.
   - Its public tag resolves to the exact release commit.
   - Its asset set matches `dist plan`: four native archives and their
     checksums plus the configured global installer, formula, checksum, and
     manifest artifacts.
   - GitHub attestations exist for the expected artifacts.
2. Native artifact
   - Download the host-native archive and checksum into a temporary directory.
   - Verify the SHA-256 checksum and
     `gh attestation verify ASSET --repo jrpat/ite`.
   - Extract it and run its `ite --version` and `ite --help`.
3. Shell installer
   - Create a temporary installation root and invoke the installer in
     cargo-dist's unmanaged mode:

     ```sh
     install_root="$(mktemp -d "${TMPDIR:-/tmp}/ite-vX.Y.Z-shell.XXXXXX")"
     ITE_CLI_UNMANAGED_INSTALL="$install_root/bin" \
       sh "$asset_dir/ite-cli-installer.sh"
     ```

   - Do not substitute `XDG_BIN_HOME` or `ITE_CLI_INSTALL_DIR`; those select a
     managed installation path and may add a temporary `bin/env` source line
     to `.profile`, `.zshrc`, and other startup files.
   - Run the installed binary's version and help commands.
4. crates.io
   - Confirm `ite-cli` version `X.Y.Z` is public, unyanked, and links back to
     `https://github.com/jrpat/ite`.
   - Compare the registry checksum with a clean local `cargo package --locked`
     archive for the release commit.
   - In a clean temporary install root, run
     `cargo install ite-cli --version X.Y.Z --locked`, then test the installed
     binary's version and help.
5. Homebrew, for stable releases only
   - Confirm `jrpat/homebrew-ite-tap` contains the generated formula for the
     exact version and checksum.
   - Install `jrpat/ite-tap/ite`, run version and help, and run `brew test ite`.
   - Do not disturb unrelated installed formulae just to perform verification.
6. Optional installers
   - Exercise any advertised mise or ubi path when its tool is available.
     Clearly report it as unverified when the tool is absent.
7. Repository state
   - Confirm the Jujutsu working copy is empty and local/remote `main` still
     resolve to the release commit.

Report the exact run, commit, tag, crate version, tap commit or formula, assets,
attestations, and installation paths that were actually verified.

## Recover safely

Classify the failure before retrying:

- Before GitHub publication, rerun a failed job or workflow only when the
  source, version, draft identity, and generated artifacts are unchanged. If a
  source change is necessary, delete the draft, land a new release commit,
  repeat preflight and approval, and create a new exact draft.
- After GitHub publication, retry only the failed downstream crates.io or tap
  stage. Never rebuild, replace, or mutate published attested assets.
- Treat an already-published crate with the expected checksum as a successful
  idempotent result.
- If published code or artifacts are wrong, prepare a patch release. Never
  reuse a crates.io version. Yank only for a serious defect and document why.

Do not declare success until the independent verification checklist passes or
the final report explicitly names each unverified or failed channel.
