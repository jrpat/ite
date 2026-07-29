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
  written off as silent keys). Spans come from `src/profile.rs` and are
  enabled by `ITE_PROFILE=<output-path>`; add `profile::span("label")`
  guards to instrument new hot paths.

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

## Architecture

- `src/keys.rs` — `Key`: normalized key repr; parses config strings like
  `ctrl+e`. Uppercase chars absorb SHIFT (`J` == `shift+j`).
- `src/config.rs` — TOML config: keybinding tables (`sh`/`cmd` + `exit`/`bg`
  flags) and `AppCommand` names. TOML bare keys can't contain `+`, so table
  headers like `[ctrl+e]` are preprocessed into quoted keys before parsing.
- `src/cli.rs` — clap CLI: mutually exclusive `[PATH]`, `-j/--json <PATH>`,
  and `-l/--jsonl <PATH>` (`-` for stdin), `-I/--no-ignore`,
  `-e/--expand <N|all>`, repeatable `-c/--config` (suppresses the user config
  at `$XDG_CONFIG_HOME/ite/config.toml`).
- `src/tree.rs` — source-neutral flat node arena implementing
  `tui_treelistview::TreeModel` (Id = `usize`). Consumers read nodes only
  through accessor methods (`name`, `detail`, `output`, `path`, `relpath`,
  `jump_key`, …) which return owned values; the stored representation is a
  private per-source `Payload` (fs: raw basename; JSON: byte span + key;
  `Explicit` for tests) plus per-tree context (fs scan root / retained JSON
  bytes), and everything else is derived on demand. `relpath` is relative to
  the source's natural unit (scan root / document / JSONL record);
  `jump_key` is always the document-global address. The app and UI consume
  the accessors and know nothing about input formats.
- `src/fstree.rs` — lazy filesystem source: `scan` walks only the top level
  (targeted depth-1 `ignore::WalkBuilder`), and each directory's contents
  come from the same targeted walk when expanded or when the sweep reaches
  it — `WalkBuilder`'s standard filters read ancestor ignore files, so lazy
  subdirectory walks honor the same rules the eager scan did. Top-level
  entries are forest roots; siblings are directories-first and
  case-insensitively sorted per list. An unwalked directory renders the ◇
  glyph; an empty one becomes a leaf once walked. Unreadable directories are
  recorded in the tree's error list (banner + stderr on exit). Alternate
  output is the basename; nodes store only the raw file name, paths derive
  by ancestor-join from the canonicalized scan root.
- `src/json_tree.rs` — the complete JSON boundary: reads the input to the
  end, detects JSON vs JSONL from the content, and discovers structure
  *shallowly* into the retained bytes (no serde DOM is kept; serde parses
  only scalars/keys during derivation). Scanning a container records only
  its immediate children's spans — each child subtree is structurally
  skipped, which validates it and yields its child count for free; deeper
  levels materialize on demand via `materialize`, the single work unit
  behind expansion and the app's background sweep. Object members retain
  input order (and duplicate keys), arrays use indexed children, default
  output and `$path` are canonical JSON Pointers, `$relpath` is the
  within-record pointer for JSONL, and alternate output is compact JSON
  re-serialized from the span. JSONL startup is just the newline scan: one
  record per non-blank line under a virtual array root (ordinal indices,
  `{…}` counts until validated); a truncated final record is dropped
  eagerly, while any other corrupt record surfaces during validation as a
  selectable ⚠ error leaf plus a banner message and stderr on exit. The
  derivation helpers at the bottom are what `Tree`'s accessors call; no
  JSON values escape this module.
- `src/app.rs` — `App`: keymap resolution (defaults + user overrides, `gg`
  chord; `?` is reserved for the keybinding panel) and `AppCommand` execution
  against `TreeListViewState`. Returns `Effect` (`Quit` / `PrintAndExit` /
  `RunShell`); no I/O here. Holds a `Mode` (`Normal` / `Jump` / `Indexing`);
  a modal picker takes over key handling in `handle_key` until it closes. `/`
  opens the picker only over a complete index — `Mode::Indexing` shows
  blocking progress until the sweep finishes. `App::do_work` is one
  cooperative quantum of index building (visible rows first, then the
  arena-order sweep), scheduled by the event loop between input polls; it is
  deliberately cooperative rather than threaded so the tree stays
  single-threaded and unit-testable — a worker thread could later slot in
  behind the same seam. Single-rooted trees open with their first level
  expanded.
- `src/keybindings.rs` — the keybinding panel: the reserved `?`/`esc` key
  constants, display entries derived once at startup from the effective keymap
  (`build_entries`), the column-grid layout, and the panel's open/scroll/
  recorded-area state (mouse-wheel routing tests against that area). `ui.rs`
  docks it above the bottom edge, shrinks the tree viewport to fit, and fills
  it with the terminal-derived focus blend inside an ANSI-blue border; without
  a palette the body falls back to reverse video while the border stays blue
  (reversing it would move the blue onto the background).
- `src/jump.rs` — the `/` jump picker (`AppCommand::Jump`): a pure state
  machine that fuzzy-matches every node's `jump_key` with `nucleo-matcher` and
  returns `Accept`/`Cancel`/`Stay`. Accept drives `select_by_id` to move focus
  and expand ancestors. The query line is a `tui_input::Input`: non-navigation
  keys are repackaged as crossterm events and passed to tui-input's own
  `handle_event` (its `crossterm` feature is pinned to the same crossterm we
  use), so cursor movement and word edits come for free without us duplicating
  its key map; re-ranks only when the input value changes.
  Single-threaded full-recompute scorer; incremental narrowing and parallelism
  deliberately deferred (ADR-0001).
- `src/runner.rs` — runs `sh -c` bindings with `$path`/`$relpath` exported as
  env vars; `bg` detaches from stdio. Foreground commands get `/dev/tty` as
  stdin when ite's own stdin was a pipe (fzf's execute behavior).
- `src/ui.rs` — renders `TreeListView` (scrolling is built into the widget's
  state) and records the viewport height for paging commands. Beware
  `ColumnWidth::flexible(min, ideal)`: `ideal` is a layout target, not a cap —
  a huge value makes the widget render a virtual canvas that wide every frame
  (this was a ~300ms/frame debug-build regression; horizontal scroll is
  disabled for the same reason). Guarded by the `repeated_draws_are_fast`
  test. `render_scrollbar` repaints the vertical bar the widget drew: color 8
  dithered to a half-tone (`▒` thumb, `░` track) with no `▲`/`▼` caps — opacity
  by glyph coverage rather than RGB blend, so it needs no palette exception.
  `TreeListViewStyle` exposes no scrollbar knobs, so we paint into the gutter
  the widget reserves, deriving geometry from the same state it uses;
  `scrollbar_thumb_tracks_the_viewport` guards that coupling. Also
  `render_jump`: draws the jump picker into any `Rect` (placement via
  `jump_area`, the identity today), so its surface is swappable (ADR-0002).
- `src/profile.rs` — span profiler (`Registry`, `Stats`), gated on
  `ITE_PROFILE`; the driver example reuses its `Stats`/formatting.
- `src/main.rs` — chooses the tree source (`choose_source`: an explicit
  `--json`/`--jsonl` file or `-`, a positional directory, else piped stdin
  means JSON-family with content detection and a tty stdin means `.`); owns
  terminal lifecycle (raw mode + alt screen
  on stderr, best-effort kitty keyboard enhancement for `ctrl+enter`/
  `shift+arrow`), event loop, effect execution. Exit codes: 0 selection, 130
  quit, foreground `exit` bindings propagate the command's status.
  Piped-stdin support rests on three legs (regression-tested in
  `tests/piped_stdin.rs`): stdin is consumed to EOF before the TUI starts and
  crossterm then reads events from `/dev/tty`; the crossterm `use-dev-tty`
  feature makes that polling use select() because kqueue cannot watch
  `/dev/tty` on macOS; and while the TUI is active fd 1 is dup2'd onto stderr
  because crossterm writes terminal queries (keyboard-enhancement probe,
  cursor position) to stdout, which would otherwise pollute the piped output
  and stall startup. For the same reason `Tui::resume` never calls
  `Terminal::clear` (it round-trips a cursor-position query); it rebuilds the
  `Terminal` so fresh buffers force a full repaint.

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
