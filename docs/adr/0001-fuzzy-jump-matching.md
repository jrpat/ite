# Fuzzy-jump matching: plain per-keystroke recompute, single-threaded

The `/` jump picker scores every candidate node against the query on each
keystroke, single-threaded, via `nucleo-matcher`'s `Pattern`. We deliberately
did **not** build incremental narrowing or parallel scoring, though both were
designed out in full.

## Considered options

- **Incremental narrowing** — re-score only the previous keystroke's survivors
  (fuzzy matches are monotone under appending). Rejected for now: it speeds up
  only keystrokes 2..N, never the first full scan, and it adds a survivor cache
  plus correctness fallbacks for negation (`!atom`) and non-append edits that
  plain recompute does not need.
- **Parallel scoring** (`std::thread::scope` or `rayon`) — a constant-factor win
  that only matters past ~100k candidates.
- **High-level `nucleo` engine** (threaded, streaming, incremental) — would force
  ite's blocking `draw → read → handle` loop into a non-blocking poll loop so
  background results could be drawn. Real complexity, no benefit at this scale.

## Why

At ite's realistic sizes (a `.gitignore`-filtered dir scan or a JSON document —
thousands to low-tens-of-thousands of nodes) a full recompute is imperceptible,
and plain recompute is trivially always-correct. Both optimizations are
ripple-free later additions: the scorer is a pure `rank(query, candidates)`
function, so incremental narrowing is a cache layer around it and parallelism is
a swap of its body — neither touches the picker's architecture. Revisit only if
profiling a genuinely large tree shows per-keystroke lag.
