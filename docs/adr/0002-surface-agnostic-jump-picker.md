# Surface-agnostic jump picker

The jump picker renders into a `Rect` and knows nothing about where that `Rect`
is. Full-screen — the only surface today — is the *identity* placement
(`jump_area(screen) = screen`). `render_jump` reads only `area.width`/`.height`
and clips to them; the `Jump` state carries no placement knowledge.

## Why

We want to move the picker to a floating window or a bottom split later without
disturbing its logic. With this split, that change touches **only** the
placement function (`jump_area`) plus any border/clear chrome — the `Jump` state
machine and `render_jump` stay untouched. A dimmed-tree-behind overlay was ruled
out separately: dimming needs colors outside the terminal's ANSI palette (0–16),
which the project forbids (see AGENTS.md), so it is not a candidate surface.

## Consequences

- Placement is one small, swappable function; the picker is unit-testable by
  rendering into an arbitrary `Rect`.
- Full-screen "hides" the tree simply by drawing over the whole area — no
  compositing, no dimming.

## Update (2026-07-28)

The palette rule has since widened: the sanctioned exception is the
terminal-derived blend itself rather than one bar (see AGENTS.md), and the
keybinding panel uses it too. Dimming toward the terminal's own background is
therefore no longer forbidden, so a dimmed-tree overlay is a live candidate
again. The decision above is unaffected — placement stays one swappable
function, which is what makes trying that surface cheap.
