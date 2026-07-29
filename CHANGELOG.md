# Changelog

All notable changes to Ite will be documented in this file.

## [Unreleased]

## [0.1.1] - 2026-07-29

### Highlights

- Press `?` to open a bottom-docked, scrollable shortcut panel showing the
  effective keymap, including user overrides and custom help.
- Press Tab to make the focused node a temporary tree root, then Shift-Tab or
  Escape to unwind roots while keeping navigation and fuzzy jump scoped to the
  active subtree.
- Press Space to toggle a container or Ctrl-Space to toggle its whole subtree
  without moving focus.
- Navigate leaves more fluidly with `l`/`L` stepping to the next sibling and
  `H` recursively collapsing the enclosing parent when the focused node cannot
  collapse.

### All changes

- Fixed toggle-command labels in the shortcut panel and strengthened the
  related UI and release regressions.
- Codified Ite's guarded, verifiable release procedure as a reusable agent
  skill.
- Added the `?` shortcut panel, simplified first-line navigation from `gg` to
  `g`, and supported optional help text on configured bindings.
- Added plain and recursive container toggles on Space and Ctrl-Space.
- Made `l` and `L` step to the next sibling when focused on a leaf.
- Made `H` recursively collapse and focus the parent when the focused node has
  nothing to collapse.
- Replaced the bright default scrollbar with a quieter ANSI-gray dithered bar.
- Added temporary tree-root navigation with Tab, Shift-Tab, and Escape.
- Refreshed the README around pipeline/sidebar workflows and installation.
- Added a non-publishing crates.io OIDC credential rehearsal.

## [0.1.0] - 2026-07-27

### Highlights

- Explore filesystem directories and JSON documents through the same fast
  terminal interface.
- Read JSON from a file or stdin while keeping selections cleanly pipeable
  through stdout.
- Navigate with Vim-style keys, mouse scrolling, recursive expansion, and `/`
  fuzzy path search.
- Run configurable shell actions against the focused node.
- Install native builds on Apple Silicon/Intel macOS and ARM64/x86_64 Linux.

### All changes

- Created the initial Rust project scaffold.
- Added key parsing, configuration loading, and the command-line interface.
- Added the filesystem tree model backed by the ignore walker.
- Added the application state machine, default keymap, and effects.
- Added the TUI event loop, rendering, and shell command runner.
- Added the user-facing README.
- Added headless TUI profiling and fixed a 300ms-per-frame rendering regression.
- Made tree glyphs more compact by removing horizontal tails.
- Derived the focus bar from terminal colors with a safe fallback.
- Connected tree guides to leaf markers.
- Restored spaced fork-only tree guides.
- Added local installation under `$XDG_BIN_HOME`.
- Made dotfiles visible by default while retaining ignore-file support.
- Added JSON documents as a source-neutral tree input.
- Added alternate output selection with Alt+Enter.
- Aligned tree stems beneath container triangles.
- Added leaf markers for top-level nodes.
- Added mouse-wheel scrolling without moving focus.
- Added piped-stdin JSON input.
- Made left navigation focus a collapsed node's parent.
- Added `/` fuzzy jump mode.
- Made right navigation descend into expanded branches.
- Styled secondary tree chrome with ANSI color 8.
- Added the automated four-target release and distribution pipeline.
