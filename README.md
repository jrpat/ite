# ite

**i**nteractive **t**ree **e**xplorer — a terminal UI for walking a directory
tree, poking at it, and doing something useful with whatever you land on.

The elevator pitch: `tree` shows you everything and scrolls off the screen;
`ite` shows you a collapsed tree and lets you open exactly the doors you care
about. Press enter on a file and its absolute path lands on stdout. That's the
whole trick, and it composes beautifully:

```sh
vim "$(ite)"          # pick a file, edit it
cd "$(ite ~/src)"     # pick a directory (ctrl+enter), go there
```

The interface draws on **stderr**, so stdout stays clean for the path. If you
have ever piped `fzf`, you already know this dance.

## Usage

```sh
ite [OPTIONS] [PATH]
```

| Flag | Meaning |
|------|---------|
| `PATH` | Directory to explore (default: `.`) |
| `-I`, `--no-ignore` | Show ignored files by disabling ignore-file rules |
| `-e`, `--expand <N\|all>` | Start with N levels expanded (`-e 1` opens top-level dirs), or all of them |
| `-c`, `--config <FILE>` | Use this config instead of the user config; repeatable, later files win |

By default `ite` shows dotfiles while respecting `.gitignore` and friends (it
uses the same filesystem walker as ripgrep). `-I` also shows ignored files.

Exit codes are honest: `0` means a path was printed, `130` means you quit
without choosing, and a keybinding configured with `exit = true` passes its
command's status through.

## Keys

Navigation is vim-flavored, with arrows for the unconverted:

| Key | Action |
|-----|--------|
| `j` / `↓`, `k` / `↑` | Move focus down / up, one visible line |
| `l` / `→`, `h` / `←` | Expand / collapse a directory |
| `L` / `shift+→` | Expand recursively |
| `H` / `shift+←` | Collapse recursively |
| `enter` | Expand a collapsed directory; on a file, print its path and exit |
| `ctrl+enter` | Print the focused path and exit, directory or not |
| `tab` | Descend into a directory (expanding it if needed) |
| `J`, `K` | Next / previous sibling, hurdling expanded subtrees |
| `ctrl+f` / `ctrl+b` | Page down / up |
| `ctrl+d` / `ctrl+u` | Half-page down / up |
| `gg`, `G` | First line, last visible line |
| `q`, `esc`, `ctrl+c` | Quit |

A note for the fine print: `ctrl+enter` and `shift+arrow` require a terminal
that speaks the kitty keyboard protocol (kitty, WezTerm, foot, recent
iTerm2...). Elsewhere, the synonyms — `tab`, `L`, `H` — have you covered.

## Configuration

`ite` reads `$XDG_CONFIG_HOME/ite/config.toml` (usually
`~/.config/ite/config.toml`). Each table is a keybinding; the table name is
the key:

```toml
[ctrl+e]
sh = "vim $path"     # run a shell command on the focused node
exit = true          # then leave ite (default: false)

[alt+s]
sh = "attach-to-review $relpath"
bg = true            # run detached, without leaving the TUI (default: false)

[ctrl+l]
cmd = "expand-recursively"   # or run an ite command instead
```

`sh` commands run via `sh -c` with two environment variables set: `$path`
(absolute) and `$relpath` (relative to the explored root). No string
splicing, no quoting accidents — the shell expands them the way shells do.
Without `bg`, the TUI steps aside while your command runs and returns when it
finishes; editors work exactly as you'd hope.

`cmd` accepts any built-in command: `down`, `up`, `expand`, `collapse`,
`expand-recursively`, `collapse-recursively`, `select`, `accept`, `descend`,
`next-sibling`, `prev-sibling`, `page-down`, `page-up`, `half-page-down`,
`half-page-up`, `first`, `last`, `quit`.

User bindings override the defaults, so if you bind `j` to something exotic,
`ite` assumes you meant it.

## Development

You need a Rust toolchain; everything else is `cargo`:

```sh
cargo build            # compile
cargo run -- ~/src     # run against a directory
cargo test             # the test suite (fast, no terminal needed)
cargo clippy --all-targets   # lints; the build is kept warning-free
cargo profile-tui      # headless perf profile: real pty, simulated keys
cargo local-bin        # release-build and install to $XDG_BIN_HOME/ite
```

`cargo profile-tui` (a cargo alias — cargo's answer to npm scripts) spawns
the release binary in a genuine PTY, drives it with keypresses, and prints
per-key latency plus an internal span table. If a keystroke ever feels
sluggish, run it before theorizing; it has already caught one absurd
regression.

The codebase separates decisions from I/O: `app.rs` turns keys into `Effect`
values (print this, run that, quit) and is fully unit-tested without a
terminal; `main.rs` owns the actual terminal and executes effects. If you're
adding behavior, this project is developed **test-first** — write the failing
test, then the code. `AGENTS.md` has the module-by-module map.

For a tight loop, run the tests on save with your watcher of choice:

```sh
cargo watch -x test -x clippy    # cargo install cargo-watch
# or: bacon test                 # cargo install bacon
```

Testing the TUI itself by hand is best done in a real terminal. If you must
script it, use `expect` and be prepared to answer the terminal's
cursor-position query (`ESC[6n`) yourself — ratatui asks at startup and will
wait politely, then give up. See AGENTS.md for the incantation.

This repository uses [Jujutsu](https://github.com/jj-vcs/jj) (`jj commit`,
not `git commit`).
