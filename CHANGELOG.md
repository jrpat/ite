# Changelog

All notable changes to Ite will be documented in this file.

## [Unreleased]

## [0.2.2] - 2026-08-05

### Highlights

- Open `.json` and `.jsonl` documents by passing their paths directly, without
  `--json` or `--jsonl`.
- Keep newly expanded children in view with minimal scrolling, while anchoring
  oversized branches at the top of the viewport.
- Hide `.git` and `.jj` repository metadata during normal filesystem scans;
  `--no-ignore` can still reveal them.

### All changes

- Recognized positional `.json` paths as JSON and `.jsonl` paths as forced
  JSONL, while preserving explicit source flags and piped input.
- Adjusted every expansion command to reveal an expanded container's direct
  children when they fit and start oversized branches from their first row.
- Excluded `.git` and `.jj` directories from eager and lazy filesystem scans
  when ignore handling is enabled.

## [0.2.1] - 2026-07-31

### Highlights

- Press `o` on a filesystem leaf to open it with your desktop's default
  application while Ite keeps running.

### All changes

- Added the configurable `open` command, bound to `o` by default; it opens real
  filesystem leaves with the platform's default handler, leaves containers and
  JSON nodes alone, detaches launched applications, and reports launch failures
  without ending the session.

## [0.2.0] - 2026-07-31

### Highlights

- Explore JSON Lines from files or stdin through a virtual array, with automatic
  content detection and `--jsonl` for ambiguous input.
- Reach the first frame much sooner on large filesystem, JSON, and JSONL trees:
  Ite now scans only the immediately visible structure, then fills in the rest
  cooperatively while you browse.
- Use substantially less memory on large trees by deriving filesystem paths and
  JSON values on demand instead of storing repeated paths or a full JSON DOM.
- Keep working through imperfect data: malformed JSONL records remain selectable,
  unreadable directories are reported, and a truncated final JSONL record is
  safely dropped.

### All changes

- Added JSONL input with content-based detection across files and stdin, plus
  `-l`/`--jsonl <PATH|->` to force JSONL when a single-record input is ambiguous.
- Presented JSONL as a virtual array with document-global paths for output and
  fuzzy jump, alongside record-relative paths for configured actions.
- Reduced filesystem-tree memory by storing each entry's basename once and
  deriving absolute and relative paths from its ancestors.
- Replaced the retained JSON DOM with a compact byte-span index over the original
  input, deriving labels, previews, pointers, and selected output on demand.
- Materialized JSON and JSONL containers lazily, with a cooperative background
  indexer that prioritizes visible rows; opening fuzzy jump waits for indexing
  with progress and can be cancelled.
- Made directory scanning lazy as well: startup reads the root, while expansion
  and the background sweep discover deeper levels without losing ancestor ignore
  rules.
- Surfaced malformed JSONL records as selectable warning nodes and unreadable
  directories through the persistent error banner and exit report.
- Rendered unloaded and empty containers consistently as collapsible branches,
  avoiding startup glyph churn as indexing advances.
- Compiled profiling instrumentation out of normal release binaries while
  extending `cargo profile-tui` to measure JSON and JSONL startup, indexing,
  expansion, and picker latency.
- Improved the keybinding panel's readability with bold key labels and dimmed
  descriptions using terminal palette colors.

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
