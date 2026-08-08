//! User-configuration boundary and application-command vocabulary. It loads
//! and merges TOML files, parses keybinding tables into actions, and names the
//! commands that `app` can execute.
//!
//! Key syntax is delegated to `keys`; shell execution remains in `runner`.
//! Before TOML parsing, table headers such as `[ctrl+e]` are quoted because `+`
//! is not valid in a TOML bare key.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::keys::Key;

/// A command understood by the application itself (the `cmd` binding kind).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppCommand {
    /// Move focus to the next on-screen line.
    Down,
    /// Move focus to the previous on-screen line.
    Up,
    /// Expand the focused non-leaf, focus its first child if already expanded,
    /// or focus the next sibling of a leaf.
    Expand,
    /// Collapse the focused expanded branch, or focus its parent.
    Collapse,
    /// Expand the focused non-leaf recursively, or focus the next sibling of
    /// a leaf.
    ExpandRecursively,
    /// Flip the focused container between expanded and collapsed.
    Toggle,
    /// Flip the focused container, expanding or collapsing recursively.
    ToggleRecursively,
    /// Collapse the focused expanded branch recursively, or collapse its
    /// parent recursively and focus it.
    CollapseRecursively,
    /// Enter semantics: expand a non-leaf, run the default action on a leaf.
    Select,
    /// Run the default action regardless of leaf-ness.
    Accept,
    /// Run the source-specific alternate action regardless of leaf-ness.
    AcceptAlternate,
    /// Descend into a non-leaf, expanding it first if collapsed.
    Descend,
    /// Make the focused node the root of the visible tree.
    Root,
    /// Restore the previous visible root.
    PopRoot,
    /// Restore the previous visible root, or quit at the original forest.
    Back,
    /// Focus next sibling, skipping over expanded children.
    NextSibling,
    /// Focus previous sibling.
    PrevSibling,
    /// Scroll down one page.
    PageDown,
    /// Scroll up one page.
    PageUp,
    /// Scroll down half a page.
    HalfPageDown,
    /// Scroll up half a page.
    HalfPageUp,
    /// Center the focused node in the viewport.
    Center,
    /// Go to the first line.
    First,
    /// Go to the last visible line.
    Last,
    /// Open the jump picker: fuzzy-find a node by path and move focus to it.
    Jump,
    /// Hand the focused leaf's filesystem path to the platform's default
    /// opener. Nodes without one — containers, JSON — are left alone.
    Open,
    /// Exit without printing anything.
    Quit,
    /// Toggle the keybinding panel. Bound to the reserved `?` key and not
    /// nameable in config, so it has no entry in [`AppCommand::parse`].
    ToggleKeybindingPanel,
}

impl AppCommand {
    pub fn parse(s: &str) -> Result<Self, String> {
        let cmd = match s {
            "down" => Self::Down,
            "up" => Self::Up,
            "expand" => Self::Expand,
            "collapse" => Self::Collapse,
            "expand-recursively" => Self::ExpandRecursively,
            "toggle" => Self::Toggle,
            "toggle-recursively" => Self::ToggleRecursively,
            "collapse-recursively" => Self::CollapseRecursively,
            "select" => Self::Select,
            "accept" => Self::Accept,
            "accept-alternate" => Self::AcceptAlternate,
            "descend" => Self::Descend,
            "root" => Self::Root,
            "pop-root" => Self::PopRoot,
            "back" => Self::Back,
            "next-sibling" => Self::NextSibling,
            "prev-sibling" => Self::PrevSibling,
            "page-down" => Self::PageDown,
            "page-up" => Self::PageUp,
            "half-page-down" => Self::HalfPageDown,
            "half-page-up" => Self::HalfPageUp,
            "center" => Self::Center,
            "first" => Self::First,
            "last" => Self::Last,
            "jump" => Self::Jump,
            "open" => Self::Open,
            "quit" => Self::Quit,
            _ => return Err(format!("unknown app command: {s:?}")),
        };
        Ok(cmd)
    }

    /// The keybinding panel's description of this command.
    pub fn description(self) -> &'static str {
        match self {
            Self::Down => "Down",
            Self::Up => "Up",
            Self::Expand => "Expand",
            Self::Collapse => "Collapse",
            Self::ExpandRecursively => "Expand all",
            Self::Toggle => "Toggle",
            Self::ToggleRecursively => "Toggle all",
            Self::CollapseRecursively => "Collapse all",
            Self::Select => "Select",
            Self::Accept => "Accept",
            Self::AcceptAlternate => "Accept alternate",
            Self::Descend => "Descend",
            Self::Root => "Root",
            Self::PopRoot => "Previous root",
            Self::Back => "Back",
            Self::NextSibling => "Next sibling",
            Self::PrevSibling => "Previous sibling",
            Self::PageDown => "Page down",
            Self::PageUp => "Page up",
            Self::HalfPageDown => "Half page down",
            Self::HalfPageUp => "Half page up",
            Self::Center => "Center",
            Self::First => "First",
            Self::Last => "Last",
            Self::Jump => "Jump",
            Self::Open => "Open",
            Self::Quit => "Quit",
            Self::ToggleKeybindingPanel => "Shortcuts",
        }
    }
}

/// What a keybinding does.
#[derive(Clone, PartialEq, Debug)]
pub enum BindingAction {
    /// Run a shell command; `$path` / `$relpath` env vars point at the focused node.
    Sh(String),
    /// Run an app command.
    Cmd(AppCommand),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Binding {
    pub action: BindingAction,
    /// Optional panel description supplied by the user, stored verbatim; the
    /// panel displays its cleaned first line.
    pub help: Option<String>,
    /// Exit the program after running (only meaningful for `sh`).
    pub exit: bool,
    /// Run in the background without suspending the TUI.
    pub bg: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub bindings: HashMap<Key, Binding>,
}

impl Config {
    /// Parse a single TOML config document.
    pub fn parse(toml_src: &str) -> Result<Self, String> {
        let toml_src = quote_key_table_headers(toml_src);
        let doc: toml::Table = toml_src.parse().map_err(|e| format!("invalid TOML: {e}"))?;
        let mut bindings = HashMap::new();
        for (name, value) in doc {
            // Tables are keybindings; other top-level values are options
            // (none defined yet; tolerated and ignored).
            if let toml::Value::Table(table) = value {
                let key = Key::parse(&name)?;
                bindings.insert(key, parse_binding(&name, &table)?);
            }
        }
        Ok(Self { bindings })
    }

    /// Merge `other` into `self`; bindings in `other` win.
    pub fn merge(&mut self, other: Config) {
        self.bindings.extend(other.bindings);
    }

    /// Load and merge the given config files in order (later files win).
    pub fn load_files(paths: &[PathBuf]) -> Result<Self, String> {
        let mut config = Self::default();
        for path in paths {
            let src = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let parsed = Self::parse(&src).map_err(|e| format!("{}: {e}", path.display()))?;
            config.merge(parsed);
        }
        Ok(config)
    }

    /// The default user config path: `$XDG_CONFIG_HOME/ite/config.toml`
    /// (falling back to `~/.config/ite/config.toml`).
    pub fn user_config_path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(base.join("ite").join("config.toml"))
    }
}

/// TOML bare keys cannot contain `+`, but the config format wants headers like
/// `[ctrl+e]`. Quote the inside of any unquoted table header so both `[ctrl+e]`
/// and `["ctrl+e"]` parse.
fn quote_key_table_headers(src: &str) -> String {
    src.lines()
        .map(|line| {
            let trimmed = line.trim();
            if let Some(inner) = trimmed
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                let inner = inner.trim();
                if !inner.starts_with(['"', '\'']) {
                    return format!("[\"{inner}\"]");
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_binding(key: &str, table: &toml::Table) -> Result<Binding, String> {
    let sh = get_str(key, table, "sh")?;
    let cmd = get_str(key, table, "cmd")?;
    let help = get_str(key, table, "help")?;
    let action = match (sh, cmd) {
        (Some(sh), None) => BindingAction::Sh(sh),
        (None, Some(cmd)) => BindingAction::Cmd(AppCommand::parse(&cmd)?),
        (Some(_), Some(_)) => {
            return Err(format!("[{key}]: `sh` and `cmd` are mutually exclusive"));
        }
        (None, None) => return Err(format!("[{key}]: needs either `sh` or `cmd`")),
    };
    Ok(Binding {
        action,
        help,
        exit: get_bool(key, table, "exit")?.unwrap_or(false),
        bg: get_bool(key, table, "bg")?.unwrap_or(false),
    })
}

fn get_str(key: &str, table: &toml::Table, field: &str) -> Result<Option<String>, String> {
    match table.get(field) {
        None => Ok(None),
        Some(toml::Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("[{key}]: `{field}` must be a string")),
    }
}

fn get_bool(key: &str, table: &toml::Table, field: &str) -> Result<Option<bool>, String> {
    match table.get(field) {
        None => Ok(None),
        Some(toml::Value::Boolean(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("[{key}]: `{field}` must be a boolean")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sh_binding_with_flags() {
        let cfg = Config::parse(
            r#"
[ctrl+e]
sh = "vim $path"
exit = true
"#,
        )
        .unwrap();
        let b = &cfg.bindings[&Key::parse("ctrl+e").unwrap()];
        assert_eq!(b.action, BindingAction::Sh("vim $path".into()));
        assert!(b.exit);
        assert!(!b.bg);
    }

    #[test]
    fn parses_bg_binding() {
        let cfg = Config::parse(
            r#"
[alt+s]
sh = "some-command $relpath"
bg = true
"#,
        )
        .unwrap();
        let b = &cfg.bindings[&Key::parse("alt+s").unwrap()];
        assert!(b.bg);
        assert!(!b.exit);
    }

    #[test]
    fn parses_cmd_binding() {
        let cfg = Config::parse(
            r#"
[ctrl+l]
cmd = "expand-recursively"
"#,
        )
        .unwrap();
        let b = &cfg.bindings[&Key::parse("ctrl+l").unwrap()];
        assert_eq!(b.action, BindingAction::Cmd(AppCommand::ExpandRecursively));
    }

    #[test]
    fn parses_optional_help_verbatim() {
        // Cleaning for display is the panel's job, not the parser's.
        let cfg = Config::parse(
            r#"
[ctrl+l]
cmd = "expand-recursively"
help = "  Custom\tCOPY\u0007\nignored"
"#,
        )
        .unwrap();
        let b = &cfg.bindings[&Key::parse("ctrl+l").unwrap()];
        assert_eq!(b.help.as_deref(), Some("  Custom\tCOPY\u{7}\nignored"));
    }

    #[test]
    fn help_must_be_a_string() {
        let error = Config::parse("[x]\nsh = \"printf x\"\nhelp = true\n").unwrap_err();
        assert!(error.contains("`help` must be a string"), "{error}");
    }

    #[test]
    fn accepts_quoted_key_headers() {
        let cfg = Config::parse("[\"ctrl+e\"]\nsh = \"x\"\n").unwrap();
        assert!(cfg.bindings.contains_key(&Key::parse("ctrl+e").unwrap()));
    }

    #[test]
    fn rejects_binding_with_both_sh_and_cmd() {
        assert!(Config::parse("[ctrl+e]\nsh = \"x\"\ncmd = \"up\"\n").is_err());
    }

    #[test]
    fn rejects_binding_with_neither_sh_nor_cmd() {
        assert!(Config::parse("[ctrl+e]\nexit = true\n").is_err());
    }

    #[test]
    fn rejects_bad_key_name() {
        assert!(Config::parse("[bogus+e]\nsh = \"x\"\n").is_err());
    }

    #[test]
    fn rejects_unknown_app_command() {
        assert!(Config::parse("[ctrl+e]\ncmd = \"frobnicate\"\n").is_err());
    }

    #[test]
    fn tolerates_top_level_options() {
        // Top-level scalar keys are options; unknown ones are ignored for now.
        let cfg = Config::parse("some_option = false\n[ctrl+e]\nsh = \"x\"\n").unwrap();
        assert_eq!(cfg.bindings.len(), 1);
    }

    #[test]
    fn merge_later_wins() {
        let mut a = Config::parse("[ctrl+e]\nsh = \"first\"\n").unwrap();
        let b = Config::parse("[ctrl+e]\nsh = \"second\"\n[ctrl+x]\ncmd = \"quit\"\n").unwrap();
        a.merge(b);
        let key = Key::parse("ctrl+e").unwrap();
        assert_eq!(a.bindings[&key].action, BindingAction::Sh("second".into()));
        assert_eq!(a.bindings.len(), 2);
    }

    #[test]
    fn app_command_names_parse() {
        for (name, cmd) in [
            ("down", AppCommand::Down),
            ("up", AppCommand::Up),
            ("expand", AppCommand::Expand),
            ("collapse", AppCommand::Collapse),
            ("expand-recursively", AppCommand::ExpandRecursively),
            ("collapse-recursively", AppCommand::CollapseRecursively),
            ("toggle", AppCommand::Toggle),
            ("toggle-recursively", AppCommand::ToggleRecursively),
            ("select", AppCommand::Select),
            ("accept", AppCommand::Accept),
            ("accept-alternate", AppCommand::AcceptAlternate),
            ("descend", AppCommand::Descend),
            ("root", AppCommand::Root),
            ("pop-root", AppCommand::PopRoot),
            ("back", AppCommand::Back),
            ("next-sibling", AppCommand::NextSibling),
            ("prev-sibling", AppCommand::PrevSibling),
            ("page-down", AppCommand::PageDown),
            ("page-up", AppCommand::PageUp),
            ("half-page-down", AppCommand::HalfPageDown),
            ("half-page-up", AppCommand::HalfPageUp),
            ("center", AppCommand::Center),
            ("first", AppCommand::First),
            ("last", AppCommand::Last),
            ("jump", AppCommand::Jump),
            ("open", AppCommand::Open),
            ("quit", AppCommand::Quit),
        ] {
            assert_eq!(AppCommand::parse(name).unwrap(), cmd, "{name}");
        }
    }

    #[test]
    fn open_has_a_keybinding_panel_description() {
        assert_eq!(AppCommand::Open.description(), "Open");
    }

    #[test]
    fn toggle_commands_have_keybinding_panel_descriptions() {
        assert_eq!(AppCommand::Toggle.description(), "Toggle");
        assert_eq!(AppCommand::ToggleRecursively.description(), "Toggle all");
    }

    #[test]
    fn load_files_merges_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("a.toml");
        let p2 = dir.path().join("b.toml");
        std::fs::write(&p1, "[ctrl+e]\nsh = \"first\"\n").unwrap();
        std::fs::write(&p2, "[ctrl+e]\nsh = \"second\"\n").unwrap();
        let cfg = Config::load_files(&[p1, p2]).unwrap();
        let key = Key::parse("ctrl+e").unwrap();
        assert_eq!(
            cfg.bindings[&key].action,
            BindingAction::Sh("second".into())
        );
    }
}
