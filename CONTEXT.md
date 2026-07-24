# ite

A TUI for navigating a tree (by default a directory's file tree, or a JSON
document via `--json`) and running an action on the focused node.

## Language

**Node**:
One entry in the source-neutral tree — a file, a directory, or a JSON value.
Carries display text, hierarchy, and the values used when it is acted upon; it
knows nothing about its input format.

**Container**:
A node that can hold children (a directory, or a JSON object/array). A container
with no children is still a container.
_Avoid_: folder, branch

**Leaf**:
A node that cannot be expanded (a file, or a JSON scalar).

**Focus**:
The single node the cursor is on. App commands act on the focused node.
_Avoid_: selection, current node

**Jump**:
Fuzzy-match a node by its path and move [Focus](#language) to it, expanding the
[Containers](#language) along the way. Invoked with `/`.
_Avoid_: filter, search, find, goto

**Jump picker**:
The overlay that hosts a [Jump](#language): a query line plus a ranked, flat
list of candidate paths. It renders into any region, so its surface (full-screen
today; a floating window or split pane later) is swappable.
_Avoid_: finder, fuzzy finder, palette

**Candidate**:
A node offered to the [Jump picker](#language) for matching — every node is one.
Matched on its path (the root-relative path for a dir scan, the JSON Pointer for
`--json`).
