# ite — interactive tree explorer

A TUI for navigating a tree (by default, the file tree of a directory) and
running actions on the focused node. `--json <PATH>` selects a JSON document
instead; piped stdin (or `--json -`) reads the document from the pipe,
`fzf`-style. JSONL input is detected from the content (first line is a
complete JSON value and more content follows) and presents as a virtual
array of records; `--jsonl <PATH|->` forces that reading for ambiguous
content. The default action prints the node's source-specific value to
stdout and exits. The TUI renders on **stderr** so stdout can be piped.

## Commands

- `cargo test` — run all tests
- `cargo clippy --all-targets` — must stay warning-free
- `cargo run -- [PATH]` — run against a directory
- `cargo run -- --json <PATH>` — run against a JSON document
- `cargo local-bin` — release-build and install to `$XDG_BIN_HOME/ite`
  (default `~/.local/bin/ite`); alias in `.cargo/config.toml` running
  `examples/install.rs` (cargo aliases can't expand env vars themselves)
- `cargo profile-tui [PATH] [ITERS]` — headless performance profile (alias in
  `.cargo/config.toml`): runs the release binary in a real PTY
  (`examples/profile_driver.rs`), answers its terminal queries, simulates
  keypresses, and prints per-key round-trip latency plus the app's internal
  span table. PATH may be a directory (run eagerly via `-e all`, the
  historical baseline) or a JSON/JSONL file (passed via `--json` so ite's
  content detection picks the mode; startup stays lazy, the report splits
  first paint from index settle, and expansion phases get long response
  deadlines so synchronous-materialization hitches are measured rather than
  written off as silent keys). Spans come from `src/profile.rs`, compiled
  only under the `profile` cargo feature (the alias passes it) and enabled
  at runtime by `ITE_PROFILE=<output-path>`; add `profile::span("label")`
  guards to instrument new hot paths. Default builds ship zero
  instrumentation, so when touching profiling code also lint/test with
  `--features profile` (CI runs both configurations).

## Development rules

- **TDD is mandatory**: for any specified behavior, write a failing test
  first, then make it pass.
- This repo uses **Jujutsu**: commit with `jj commit`, never `git commit`.
- UI colors must stay within the terminal's default ANSI palette (colors
  0–16); never emit hardcoded RGB values. The one sanctioned exception is an
  RGB blend *derived from the terminal's own colors* (queried via OSC 10/11
  through terminal-colorsaurus at startup), used by surfaces that want a
  translucent-looking background: the focus bar, the jump picker's selected
  row, and the keybinding panel body. All three go through
  `ui::focus_style`/`Palette::focus_bg`, which falls back to reverse video
  when the terminal doesn't answer.

## Notes

- ite depends only on released crates, and must keep doing so: `cargo publish`
  rejects path and git dependencies alike ("all dependencies must have a
  version requirement specified"), and a `[patch.crates-io]` is stripped at
  package time so the verification build fails on whatever the patch added.
  That is why `render_scrollbar` repaints over the widget instead of using a
  patched `TreeListViewStyle`. The upstream-shaped version of that patch lives
  on the `scrollbar-style` bookmark of the fork at `~/src/_forked/tui-treelistview`
  (jrpat/tui-treelistview); once it lands upstream and is released, drop
  `render_scrollbar` in favour of the real `vertical_scrollbar` field.
- Directory-specific configs are planned; mechanism undecided.
- Manual TUI testing headlessly: `expect` scripts must answer the terminal's
  cursor-position query (`ESC[6n`) or ratatui fails at startup (see the
  session scripts pattern: respond with `ESC[1;1R`).
