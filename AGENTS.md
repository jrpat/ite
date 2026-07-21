# ite — interactive tree explorer

A TUI for navigating a tree (by default, the file tree of a directory) and
running actions on the focused node. The default action prints the node's
absolute path to stdout and exits. The TUI renders on **stderr** so stdout can
be piped.

## Commands

- `cargo test` — run all tests
- `cargo clippy --all-targets` — must stay warning-free
- `cargo run -- [PATH]` — run against a directory

## Development rules

- **TDD is mandatory**: for any specified behavior, write a failing test
  first, then make it pass.
- This repo uses **Jujutsu**: commit with `jj commit`, never `git commit`.
- UI colors must stay within the terminal's default ANSI palette (colors
  0–16); never emit RGB values.

## Architecture

- `src/keys.rs` — `Key`: normalized key repr; parses config strings like
  `ctrl+e`. Uppercase chars absorb SHIFT (`J` == `shift+j`).
- `src/config.rs` — TOML config: keybinding tables (`sh`/`cmd` + `exit`/`bg`
  flags) and `AppCommand` names. TOML bare keys can't contain `+`, so table
  headers like `[ctrl+e]` are preprocessed into quoted keys before parsing.
- `src/cli.rs` — clap CLI: `[PATH]`, `-I/--no-ignore`, `-e/--expand <N|all>`,
  repeatable `-c/--config` (suppresses the user config at
  `$XDG_CONFIG_HOME/ite/config.toml`).
- `src/fstree.rs` — `FsTree`: eager scan via `ignore::WalkBuilder`; flat node
  arena implementing `tui_treelistview::TreeModel` (Id = `usize`, top-level
  entries are forest roots). Dirs-first, case-insensitive sibling order.
  Empty dirs are leaves.
- `src/app.rs` — `App`: keymap resolution (defaults + user overrides, `gg`
  chord) and `AppCommand` execution against `TreeListViewState`. Returns
  `Effect` (`Quit` / `PrintAndExit` / `RunShell`); no I/O here.
- `src/runner.rs` — runs `sh -c` bindings with `$path`/`$relpath` exported as
  env vars; `bg` detaches from stdio.
- `src/ui.rs` — renders `TreeListView` (scrolling is built into the widget's
  state) and records the viewport height for paging commands.
- `src/main.rs` — terminal lifecycle (raw mode + alt screen on stderr,
  best-effort kitty keyboard enhancement for `ctrl+enter`/`shift+arrow`),
  event loop, effect execution. Exit codes: 0 selection, 130 quit,
  foreground `exit` bindings propagate the command's status.

## Notes

- Piped-in tree data is a planned feature; input format undecided.
- Directory-specific configs are planned; mechanism undecided.
- Manual TUI testing headlessly: `expect` scripts must answer the terminal's
  cursor-position query (`ESC[6n`) or ratatui fails at startup (see the
  session scripts pattern: respond with `ESC[1;1R`).
