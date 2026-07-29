# ite

**i**nteractive **t**ree **e**xplorer — a terminal UI for walking a tree,
poking at it, and doing something useful with whatever you land on.

It is designed for two distict workflows:
- In a pipeline it behaves like `fzf`: you open it, pick one thing, and it hands that
  thing to whatever comes next.
- It can also behave like a sidebar, and you can easily bind keys to run shell commands
  against the focused node.

Used in a pipeline, simply press enter on a leaf and its value goes to stdout — the
absolute path for a filesystem tree, the node's JSON Pointer for a JSON document.

Use it like you do `fzf`:

```sh
vim "$(ite)"          # pick a file, edit it
cd "$(ite ~/src)"     # pick a directory (ctrl+enter), go there
ite --json response.json      # explore a JSON document
curl -s api.example.com/users | ite   # pipe JSON straight in
```

## Installation

```sh
brew install jrpat/ite-tap/ite   # Homebrew (macOS/Linux)
cargo install ite-cli            # from crates.io, installs the `ite` binary
```

Or grab a prebuilt binary:

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/jrpat/ite/releases/latest/download/ite-cli-installer.sh | sh
```

Prebuilt archives for macOS and Linux (x86_64/aarch64) are also available on
the [releases page](https://github.com/jrpat/ite/releases).

## Usage

```sh
ite [OPTIONS] [PATH]
```

| Flag | Meaning |
|------|---------|
| `PATH` | Directory to explore (default: `.`) |
| `-j`, `--json <PATH>` | Explore a JSON file instead of a directory (`-` reads stdin) |
| `-I`, `--no-ignore` | Show ignored files by disabling ignore-file rules |
| `-e`, `--expand <N\|all>` | Start with N levels expanded (`-e 1` opens top-level containers), or all of them |
| `-c`, `--config <FILE>` | Use this config instead of the user config; repeatable, later files win |

### Filesystem

By default, `ite` explores `PATH` (or `.`). It shows dotfiles while respecting
`.gitignore` and friends. `-I` also shows ignored files.

### JSON

Pass `--json PATH` (or `-j PATH`) to explore one JSON document instead.
Objects become tree branches, arrays keep their input order under indexed
children, and scalar values are leaves:

```text
$ ite --json users.json
▼ users [2]
├ ▼ [0] {2} id: 12 · name: "Ada"
│ ├ • id: 12
│ └ • name: "Ada"
└ • [1]: null
```

Accepting a JSON node writes its canonical JSON Pointer, such as `/users/0/name`; the
root pointer is empty. `$path` and `$relpath` in shell bindings contain the same value.

JSON can also arrive on a pipe. When stdin is not a terminal and no directory is given,
`ite` reads a JSON document from stdin; `--json -` requests the same thing explicitly.

```sh
curl -s api.example.com/users | ite
kubectl get pod mypod -o json | ite --json - --expand 1
```

To explore a directory while something is piped in, name it: `producer | ite .`
ignores the pipe and explores the filesystem.

## Exit Codes

Exit codes are honest: `0` means a value was printed, `130` means you quit
without choosing, and a keybinding configured with `exit = true` passes its
command's status through.

## Keys

Navigation is vim-flavored:

| Key | Action |
|-----|--------|
| `j` / `↓`, `k` / `↑` | Move focus down / up, one visible line |
| `l` / `→` | Expand a collapsed container; on an expanded container, focus its first child |
| `h` / `←` | Collapse an expanded container; otherwise focus its parent |
| `L` / `shift+→` | Expand recursively |
| `H` / `shift+←` | Collapse an expanded container recursively; otherwise collapse its parent recursively and focus it |
| `enter` | Expand a collapsed container; on a leaf, print its path or JSON Pointer and exit |
| `ctrl+enter` | Print the focused path or JSON Pointer and exit, container or not |
| `alt+enter` | Print the focused basename or compact JSON value and exit |
| `tab` / `shift+tab` | Make the focused node the root / restore the previous root |
| `J`, `K` | Next / previous sibling, hurdling expanded subtrees |
| `ctrl+f` / `ctrl+b` | Page down / up |
| `ctrl+d` / `ctrl+u` | Half-page down / up |
| `gg`, `G` | First line, last visible line |
| `esc` | Restore the previous root; quit from the original tree |
| `q`, `ctrl+c` | Quit |

<sub>
Note: <tt>ctrl+enter</tt> and <tt>shift+arrow</tt> require a terminal
that speaks the kitty keyboard protocol.
</sub>

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

`sh` commands run via `sh -c` with two environment variables set. For a
filesystem tree, `$path` is absolute and `$relpath` is relative to the
explored root. For JSON, both are the selected node's JSON Pointer. No string
splicing, no quoting accidents — the shell expands them the way shells do.
Without `bg`, the TUI steps aside while your command runs and returns when it
finishes; editors work exactly as you'd hope.

`cmd` accepts any built-in command: `down`, `up`, `expand`, `collapse`,
`expand-recursively`, `collapse-recursively`, `select`, `accept`,
`accept-alternate`, `descend`, `root`, `pop-root`, `back`, `next-sibling`,
`prev-sibling`, `page-down`, `page-up`, `half-page-down`, `half-page-up`,
`first`, `last`, `jump`, `quit`.

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

### JSON

To play with a JSON sample directly from a checkout:

```sh
./examples/json-demo.sh
./examples/json-demo.sh --expand all  # start with everything open
```

The script quietly builds the current working copy and opens
[`examples/sample.json`](examples/sample.json). Edit that file to try your own
shapes.
