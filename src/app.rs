//! Application state: focus/expansion driven by app commands and keybindings.

use std::collections::HashMap;
use std::ffi::OsString;

use tui_treelistview::{TreeListViewState, TreeQuery};

use crate::cli::ExpandSpec;
use crate::config::{AppCommand, Binding, BindingAction, Config};
use crate::jump::{Jump, JumpOutcome};
use crate::keys::Key;
use crate::tree::{NodeId, Tree};

/// What the event loop must do after a key is handled.
#[derive(Clone, PartialEq, Debug)]
pub enum Effect {
    None,
    /// Exit without output.
    Quit,
    /// The default action: print the node's source-specific value and exit.
    PrintAndExit(OsString),
    /// Run a configured shell command on the focused node.
    RunShell {
        cmd: String,
        path: OsString,
        relpath: OsString,
        bg: bool,
        exit: bool,
    },
}

/// The app's input mode. `Normal` drives the tree via the keymap; other modes
/// take over key handling and rendering until they close. Adding a mode is a
/// new variant plus one dispatch arm in `handle_key` and one in `ui::draw`.
pub enum Mode {
    Normal,
    Jump(Jump),
}

pub struct App {
    pub tree: Tree,
    pub state: TreeListViewState<NodeId>,
    pub query: TreeQuery,
    /// The current input mode; see [`Mode`].
    pub mode: Mode,
    keymap: HashMap<Key, Binding>,
    /// Temporary view roots, from the original forest to the current subtree.
    root_history: Vec<Option<NodeId>>,
    /// True after a bare `g`, waiting for the second `g` of the chord.
    pending_g: bool,
    /// Rows per screen; the UI updates this every frame.
    pub page_height: usize,
    /// Terminal default colors, when the terminal answered the startup query.
    pub palette: Option<crate::ui::Palette>,
}

impl App {
    pub fn new(tree: Tree, config: &Config, expand: Option<ExpandSpec>) -> Self {
        let mut keymap = Self::default_keymap();
        keymap.extend(config.bindings.clone());
        let mut app = Self {
            tree,
            state: TreeListViewState::with_capacity(0),
            query: TreeQuery::new(),
            mode: Mode::Normal,
            keymap,
            root_history: Vec::new(),
            pending_g: false,
            page_height: 20,
            palette: None,
        };
        match expand {
            None => {}
            Some(ExpandSpec::All) => {
                let branches: Vec<_> = app.tree.branches().collect();
                for (id, parent) in branches {
                    app.state.set_expanded(id, parent, true);
                }
            }
            Some(ExpandSpec::Depth(n)) => {
                let branches: Vec<_> = app.tree.branches().collect();
                for (id, parent) in branches {
                    if app.tree.node(id).depth < n {
                        app.state.set_expanded(id, parent, true);
                    }
                }
            }
        }
        app.state.ensure_projection(&app.tree, &app.query);
        app.state.select_first();
        app
    }

    /// The default keybindings, before user config is merged.
    pub fn default_keymap() -> HashMap<Key, Binding> {
        let cmd = |action: AppCommand| Binding {
            action: BindingAction::Cmd(action),
            exit: false,
            bg: false,
        };
        let mut map = HashMap::new();
        for (keys, action) in [
            (&["j", "down"][..], AppCommand::Down),
            (&["k", "up"], AppCommand::Up),
            (&["l", "right"], AppCommand::Expand),
            (&["h", "left"], AppCommand::Collapse),
            (&["L", "shift+right"], AppCommand::ExpandRecursively),
            (&["H", "shift+left"], AppCommand::CollapseRecursively),
            (&["enter"], AppCommand::Select),
            (&["ctrl+enter"], AppCommand::Accept),
            (&["alt+enter"], AppCommand::AcceptAlternate),
            (&["tab"], AppCommand::Root),
            (&["shift+tab"], AppCommand::PopRoot),
            (&["J"], AppCommand::NextSibling),
            (&["K"], AppCommand::PrevSibling),
            (&["ctrl+f"], AppCommand::PageDown),
            (&["ctrl+b"], AppCommand::PageUp),
            (&["ctrl+d"], AppCommand::HalfPageDown),
            (&["ctrl+u"], AppCommand::HalfPageUp),
            (&["G"], AppCommand::Last),
            (&["/"], AppCommand::Jump),
            (&["esc"], AppCommand::Back),
            (&["q", "ctrl+c"], AppCommand::Quit),
        ] {
            for key in keys {
                map.insert(Key::parse(key).expect("valid default key"), cmd(action));
            }
        }
        map
    }

    pub fn focused_id(&mut self) -> Option<NodeId> {
        self.state.ensure_projection(&self.tree, &self.query);
        self.state.selected_id()
    }

    /// Names of currently visible rows, in on-screen order.
    pub fn visible_names(&mut self) -> Vec<String> {
        self.state.ensure_projection(&self.tree, &self.query);
        self.state
            .visible_ids()
            .map(|id| self.tree.node(id).name.clone())
            .collect()
    }

    /// Handle a normalized key, resolving chords and the keymap.
    pub fn handle_key(&mut self, key: Key) -> Effect {
        let _span = crate::profile::span("app::handle_key");
        // A modal picker takes over key handling until it closes. Accepting
        // moves focus (expanding ancestors); cancelling leaves focus untouched,
        // so the user returns to exactly where they opened it.
        if let Mode::Jump(jump) = &mut self.mode {
            return match jump.handle_key(key) {
                JumpOutcome::Stay => Effect::None,
                JumpOutcome::Cancel => {
                    self.mode = Mode::Normal;
                    Effect::None
                }
                JumpOutcome::Accept(id) => {
                    self.mode = Mode::Normal;
                    self.state.select_by_id(&self.tree, &self.query, id);
                    Effect::None
                }
            };
        }
        let g = Key::parse("g").unwrap();
        if self.pending_g {
            self.pending_g = false;
            if key == g {
                return self.run_command(AppCommand::First);
            }
            // fall through: the second key is handled normally
        } else if key == g && !self.keymap.contains_key(&g) {
            self.pending_g = true;
            return Effect::None;
        }
        match self.keymap.get(&key).cloned() {
            None => Effect::None,
            Some(binding) => match binding.action {
                BindingAction::Cmd(cmd) => self.run_command(cmd),
                BindingAction::Sh(cmd) => match self.focused_id() {
                    None => Effect::None,
                    Some(id) => Effect::RunShell {
                        cmd,
                        path: self.tree.node(id).action.path.clone(),
                        relpath: self.tree.node(id).action.relpath.clone(),
                        bg: binding.bg,
                        exit: binding.exit,
                    },
                },
            },
        }
    }

    /// Execute an app command.
    pub fn run_command(&mut self, cmd: AppCommand) -> Effect {
        self.state.ensure_projection(&self.tree, &self.query);
        match cmd {
            AppCommand::Down => {
                self.state.select_next();
            }
            AppCommand::Up => {
                self.state.select_prev();
            }
            AppCommand::Expand => {
                if let Some(id) = self.focused_id() {
                    let parent = self.tree.view_parent(id);
                    if self.tree.is_leaf(id) {
                        // Nothing to open: step along to the next sibling.
                        self.move_sibling(1);
                    } else if self.state.node_is_expanded(id, parent) {
                        self.state.select_id(Some(self.tree.node(id).children[0]));
                    } else {
                        self.state.set_expanded(id, parent, true);
                    }
                }
            }
            AppCommand::Collapse => {
                if let Some(id) = self.focused_id() {
                    let parent = self.tree.view_parent(id);
                    if !self.tree.is_leaf(id) && self.state.node_is_expanded(id, parent) {
                        self.state.set_expanded(id, parent, false);
                    } else if let Some(parent) = parent {
                        self.state.select_id(Some(parent));
                    }
                }
            }
            AppCommand::ExpandRecursively => {
                if let Some(id) = self.focused_id() {
                    if self.tree.is_leaf(id) {
                        // Nothing to open: step along to the next sibling.
                        self.move_sibling(1);
                    } else {
                        self.set_expanded_recursively(id, true);
                    }
                }
            }
            AppCommand::CollapseRecursively => {
                if let Some(id) = self.focused_id() {
                    let parent = self.tree.view_parent(id);
                    if !self.tree.is_leaf(id) && self.state.node_is_expanded(id, parent) {
                        self.set_expanded_recursively(id, false);
                    } else if let Some(parent) = parent {
                        // Nothing to collapse here: fold up the enclosing
                        // container and land on it.
                        self.set_expanded_recursively(parent, false);
                        self.state.ensure_projection(&self.tree, &self.query);
                        self.state.select_id(Some(parent));
                    }
                }
            }
            AppCommand::Select => {
                if let Some(id) = self.focused_id() {
                    if self.tree.is_leaf(id) {
                        return Effect::PrintAndExit(self.tree.node(id).action.output.clone());
                    }
                    self.state.set_expanded(id, self.tree.view_parent(id), true);
                }
            }
            AppCommand::Accept => {
                if let Some(id) = self.focused_id() {
                    return Effect::PrintAndExit(self.tree.node(id).action.output.clone());
                }
            }
            AppCommand::AcceptAlternate => {
                if let Some(id) = self.focused_id() {
                    return Effect::PrintAndExit(
                        self.tree.node(id).action.alternate_output.clone(),
                    );
                }
            }
            AppCommand::Descend => {
                if let Some(id) = self.focused_branch() {
                    self.state.set_expanded(id, self.tree.view_parent(id), true);
                    self.state.ensure_projection(&self.tree, &self.query);
                    let first_child = self.tree.node(id).children[0];
                    self.state.select_id(Some(first_child));
                }
            }
            AppCommand::Root => self.push_root(),
            AppCommand::PopRoot => {
                self.pop_root();
            }
            AppCommand::Back => {
                if !self.pop_root() {
                    return Effect::Quit;
                }
            }
            AppCommand::NextSibling => self.move_sibling(1),
            AppCommand::PrevSibling => self.move_sibling(-1),
            AppCommand::PageDown => self.move_focus_by(self.page_height as isize),
            AppCommand::PageUp => self.move_focus_by(-(self.page_height as isize)),
            AppCommand::HalfPageDown => self.move_focus_by((self.page_height / 2) as isize),
            AppCommand::HalfPageUp => self.move_focus_by(-((self.page_height / 2) as isize)),
            AppCommand::First => {
                self.state.select_first();
            }
            AppCommand::Last => {
                self.state.select_last();
            }
            AppCommand::Jump => {
                self.mode = Mode::Jump(Jump::open(&self.tree));
            }
            AppCommand::Quit => return Effect::Quit,
        }
        Effect::None
    }

    /// The focused node if it is expandable.
    fn focused_branch(&mut self) -> Option<NodeId> {
        self.focused_id().filter(|&id| !self.tree.is_leaf(id))
    }

    fn set_expanded_recursively(&mut self, root: NodeId, expanded: bool) {
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if !self.tree.is_leaf(id) {
                self.state
                    .set_expanded(id, self.tree.view_parent(id), expanded);
                stack.extend_from_slice(&self.tree.node(id).children);
            }
        }
    }

    fn push_root(&mut self) {
        let Some(id) = self.focused_id() else {
            return;
        };
        if self.tree.view_root() == Some(id) {
            return;
        }

        self.root_history.push(self.tree.view_root());
        self.tree.set_view_root(Some(id));
        if !self.tree.is_leaf(id) {
            self.state.set_expanded(id, None, true);
        }
        self.state.ensure_projection(&self.tree, &self.query);
        self.state.select_id(Some(id));
    }

    fn pop_root(&mut self) -> bool {
        let Some(previous_root) = self.root_history.pop() else {
            return false;
        };
        let selected = self.state.selected_id();

        if let Some(current_root) = self.tree.view_root() {
            let expanded = self.state.node_is_expanded(current_root, None);
            self.state
                .set_expanded(current_root, self.tree.node(current_root).parent, expanded);
        }

        self.tree.set_view_root(previous_root);
        self.state.ensure_projection(&self.tree, &self.query);
        if !selected.is_some_and(|id| self.state.select_by_id(&self.tree, &self.query, id)) {
            self.state.select_first();
        }
        true
    }

    fn move_sibling(&mut self, delta: isize) {
        let Some(id) = self.focused_id() else { return };
        if self.tree.view_root() == Some(id) {
            return;
        }
        let siblings = match self.tree.node(id).parent {
            Some(parent) => self.tree.node(parent).children.as_slice(),
            None => self.tree.root_ids(),
        };
        let pos = siblings.iter().position(|&s| s == id).unwrap_or(0) as isize;
        let target = pos + delta;
        if (0..siblings.len() as isize).contains(&target) {
            let target = siblings[target as usize];
            self.state.select_id(Some(target));
        }
    }

    fn move_focus_by(&mut self, delta: isize) {
        let len = self.state.visible_len();
        if len == 0 {
            return;
        }
        let current = self.state.selected_index().unwrap_or(0) as isize;
        let target = (current + delta).clamp(0, len as isize - 1);
        self.state.select_index(Some(target as usize));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fstree;

    /// Builds:
    ///   root/
    ///     a/
    ///       aa/
    ///         aaa.txt
    ///       ab.txt
    ///     b/
    ///       ba.txt
    ///     c.txt
    fn fixture() -> (tempfile::TempDir, Tree) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir_all(p.join("a/aa")).unwrap();
        std::fs::write(p.join("a/aa/aaa.txt"), "").unwrap();
        std::fs::write(p.join("a/ab.txt"), "").unwrap();
        std::fs::create_dir(p.join("b")).unwrap();
        std::fs::write(p.join("b/ba.txt"), "").unwrap();
        std::fs::write(p.join("c.txt"), "").unwrap();
        let tree = fstree::scan(p, false).unwrap();
        (dir, tree)
    }

    fn app() -> (tempfile::TempDir, App) {
        let (dir, tree) = fixture();
        (dir, App::new(tree, &Config::default(), None))
    }

    /// The main fixture has no leaf with a following sibling. Builds:
    ///   root/
    ///     d/
    ///       d1.txt
    ///       d2.txt
    fn app_with_leaf_siblings() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir(p.join("d")).unwrap();
        std::fs::write(p.join("d/d1.txt"), "").unwrap();
        std::fs::write(p.join("d/d2.txt"), "").unwrap();
        let tree = fstree::scan(p, false).unwrap();
        (dir, App::new(tree, &Config::default(), None))
    }

    fn focused_name(app: &mut App) -> String {
        let id = app.focused_id().expect("something focused");
        app.tree.node(id).name.clone()
    }

    #[test]
    fn starts_focused_on_first_row_all_collapsed() {
        let (_d, mut app) = app();
        assert_eq!(app.visible_names(), ["a", "b", "c.txt"]);
        assert_eq!(focused_name(&mut app), "a");
    }

    #[test]
    fn down_and_up_move_focus_clamped() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Down);
        assert_eq!(focused_name(&mut app), "b");
        app.run_command(AppCommand::Down);
        assert_eq!(focused_name(&mut app), "c.txt");
        app.run_command(AppCommand::Down);
        assert_eq!(focused_name(&mut app), "c.txt");
        app.run_command(AppCommand::Up);
        assert_eq!(focused_name(&mut app), "b");
    }

    #[test]
    fn expand_reveals_children_and_down_enters_them() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Expand);
        assert_eq!(app.visible_names(), ["a", "aa", "ab.txt", "b", "c.txt"]);
        app.run_command(AppCommand::Down);
        assert_eq!(focused_name(&mut app), "aa");
    }

    #[test]
    fn l_on_expanded_branch_descends_to_first_child() {
        let (_d, mut app) = app();
        app.handle_key(Key::parse("l").unwrap());

        app.handle_key(Key::parse("l").unwrap());

        assert_eq!(focused_name(&mut app), "aa");
    }

    #[test]
    fn l_on_leaf_focuses_next_sibling() {
        let (_d, mut app) = app_with_leaf_siblings();
        app.run_command(AppCommand::Expand); // expand "d"
        app.run_command(AppCommand::Down); // focus "d1.txt"

        app.handle_key(Key::parse("l").unwrap());

        assert_eq!(focused_name(&mut app), "d2.txt");
        assert_eq!(app.visible_names(), ["d", "d1.txt", "d2.txt"]);
    }

    #[test]
    fn l_on_last_leaf_of_a_container_stays_inside_it() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Expand); // expand "a"
        app.run_command(AppCommand::Down); // focus "aa"
        app.run_command(AppCommand::Down); // focus "ab.txt", last child of "a"

        app.handle_key(Key::parse("l").unwrap());

        // Does not spill over to "b": siblings, not the next visible row.
        assert_eq!(focused_name(&mut app), "ab.txt");
        assert_eq!(app.visible_names(), ["a", "aa", "ab.txt", "b", "c.txt"]);
    }

    #[test]
    fn expand_on_the_last_top_level_leaf_is_a_noop() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Last);
        assert_eq!(focused_name(&mut app), "c.txt");
        assert_eq!(app.run_command(AppCommand::Expand), Effect::None);
        assert_eq!(focused_name(&mut app), "c.txt");
        assert_eq!(app.visible_names(), ["a", "b", "c.txt"]);
    }

    #[test]
    fn collapse_hides_children() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Expand);
        app.run_command(AppCommand::Collapse);
        assert_eq!(app.visible_names(), ["a", "b", "c.txt"]);
    }

    #[test]
    fn h_on_leaf_focuses_parent_without_collapsing_it() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Expand);
        app.run_command(AppCommand::Down); // focus collapsed "aa"
        app.run_command(AppCommand::Expand);
        app.run_command(AppCommand::Down); // focus "aaa.txt"

        app.handle_key(Key::parse("h").unwrap());

        assert_eq!(focused_name(&mut app), "aa");
        assert_eq!(
            app.visible_names(),
            ["a", "aa", "aaa.txt", "ab.txt", "b", "c.txt"]
        );
    }

    #[test]
    fn h_on_collapsed_branch_focuses_parent_without_collapsing_it() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Expand);
        app.run_command(AppCommand::Down); // focus collapsed "aa"

        app.handle_key(Key::parse("h").unwrap());

        assert_eq!(focused_name(&mut app), "a");
        assert_eq!(app.visible_names(), ["a", "aa", "ab.txt", "b", "c.txt"]);
    }

    #[test]
    fn expand_recursively_expands_whole_subtree() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::ExpandRecursively);
        assert_eq!(
            app.visible_names(),
            ["a", "aa", "aaa.txt", "ab.txt", "b", "c.txt"]
        );
    }

    #[test]
    fn shift_l_on_a_container_leaves_focus_on_it() {
        let (_d, mut app) = app();

        app.handle_key(Key::parse("L").unwrap());

        assert_eq!(focused_name(&mut app), "a");
    }

    #[test]
    fn shift_l_on_leaf_focuses_next_sibling() {
        let (_d, mut app) = app_with_leaf_siblings();
        app.run_command(AppCommand::Expand); // expand "d"
        app.run_command(AppCommand::Down); // focus "d1.txt"

        app.handle_key(Key::parse("L").unwrap());

        assert_eq!(focused_name(&mut app), "d2.txt");
        assert_eq!(app.visible_names(), ["d", "d1.txt", "d2.txt"]);
    }

    #[test]
    fn shift_l_on_the_last_top_level_leaf_is_a_noop() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Last); // focus "c.txt"

        app.handle_key(Key::parse("L").unwrap());

        assert_eq!(focused_name(&mut app), "c.txt");
        assert_eq!(app.visible_names(), ["a", "b", "c.txt"]);
    }

    #[test]
    fn collapse_recursively_collapses_whole_subtree() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::ExpandRecursively);
        app.run_command(AppCommand::CollapseRecursively);
        assert_eq!(app.visible_names(), ["a", "b", "c.txt"]);
        // Descendant expansion was cleared, not just hidden.
        app.run_command(AppCommand::Expand);
        assert_eq!(app.visible_names(), ["a", "aa", "ab.txt", "b", "c.txt"]);
    }

    #[test]
    fn shift_h_on_leaf_collapses_parent_and_focuses_it() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::ExpandRecursively);
        app.run_command(AppCommand::Down); // focus expanded "aa"
        app.run_command(AppCommand::Down); // focus "aaa.txt"

        app.handle_key(Key::parse("H").unwrap());

        assert_eq!(focused_name(&mut app), "aa");
        assert_eq!(app.visible_names(), ["a", "aa", "ab.txt", "b", "c.txt"]);
    }

    #[test]
    fn shift_h_on_collapsed_branch_collapses_parent_and_focuses_it() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Expand);
        app.run_command(AppCommand::Down); // focus collapsed "aa"

        app.handle_key(Key::parse("H").unwrap());

        assert_eq!(focused_name(&mut app), "a");
        assert_eq!(app.visible_names(), ["a", "b", "c.txt"]);
    }

    #[test]
    fn shift_h_collapses_the_parent_recursively() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::ExpandRecursively);
        app.run_command(AppCommand::Down); // focus expanded "aa"
        app.run_command(AppCommand::Down); // focus "aaa.txt"
        app.run_command(AppCommand::Down); // focus "ab.txt"

        app.handle_key(Key::parse("H").unwrap());

        assert_eq!(focused_name(&mut app), "a");
        assert_eq!(app.visible_names(), ["a", "b", "c.txt"]);
        // "aa" was collapsed along with "a", not just hidden by it.
        app.run_command(AppCommand::Expand);
        assert_eq!(app.visible_names(), ["a", "aa", "ab.txt", "b", "c.txt"]);
    }

    #[test]
    fn shift_h_on_a_top_level_leaf_leaves_focus_alone() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Last); // focus top-level leaf "c.txt"

        app.handle_key(Key::parse("H").unwrap());

        assert_eq!(focused_name(&mut app), "c.txt");
        assert_eq!(app.visible_names(), ["a", "b", "c.txt"]);
    }

    #[test]
    fn select_expands_collapsed_dir_and_prints_leaf() {
        let (_d, mut app) = app();
        assert_eq!(app.run_command(AppCommand::Select), Effect::None);
        assert_eq!(app.visible_names(), ["a", "aa", "ab.txt", "b", "c.txt"]);
        app.run_command(AppCommand::Last);
        let effect = app.run_command(AppCommand::Select);
        let Effect::PrintAndExit(path) = effect else {
            panic!("expected PrintAndExit, got {effect:?}");
        };
        assert!(std::path::Path::new(&path).is_absolute());
        assert!(std::path::Path::new(&path).ends_with("c.txt"));
    }

    #[test]
    fn select_does_not_descend_into_an_expanded_branch() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Select);

        app.run_command(AppCommand::Select);

        assert_eq!(focused_name(&mut app), "a");
    }

    #[test]
    fn accept_prints_even_on_dir() {
        let (_d, mut app) = app();
        let effect = app.run_command(AppCommand::Accept);
        let Effect::PrintAndExit(path) = effect else {
            panic!("expected PrintAndExit, got {effect:?}");
        };
        assert!(std::path::Path::new(&path).ends_with("a"));
    }

    #[test]
    fn alt_enter_prints_the_filesystem_basename() {
        let (_d, mut app) = app();

        assert_eq!(
            app.handle_key(Key::parse("alt+enter").unwrap()),
            Effect::PrintAndExit(OsString::from("a"))
        );
    }

    #[test]
    fn descend_expands_and_focuses_first_child() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Descend);
        assert_eq!(focused_name(&mut app), "aa");
    }

    #[test]
    fn tab_pushes_focused_nodes_as_view_roots_and_shift_tab_pops_them() {
        let (_d, mut app) = app();

        app.handle_key(Key::parse("tab").unwrap());
        assert_eq!(focused_name(&mut app), "a");
        assert_eq!(app.visible_names(), ["a", "aa", "ab.txt"]);

        app.handle_key(Key::parse("l").unwrap()); // focus "aa"
        app.handle_key(Key::parse("tab").unwrap());
        assert_eq!(focused_name(&mut app), "aa");
        assert_eq!(app.visible_names(), ["aa", "aaa.txt"]);

        // The temporary root is a navigation boundary.
        app.handle_key(Key::parse("h").unwrap()); // collapse root "aa"
        assert_eq!(app.visible_names(), ["aa"]);
        app.handle_key(Key::parse("h").unwrap()); // cannot focus parent "a"
        assert_eq!(focused_name(&mut app), "aa");

        app.handle_key(Key::parse("l").unwrap()); // expand root "aa"
        assert_eq!(app.visible_names(), ["aa", "aaa.txt"]);

        app.handle_key(Key::parse("shift+tab").unwrap());
        assert_eq!(focused_name(&mut app), "aa");
        assert_eq!(app.visible_names(), ["a", "aa", "aaa.txt", "ab.txt"]);

        app.handle_key(Key::parse("shift+tab").unwrap());
        assert_eq!(focused_name(&mut app), "aa");
        assert_eq!(
            app.visible_names(),
            ["a", "aa", "aaa.txt", "ab.txt", "b", "c.txt"]
        );

        // There is no root before the original forest.
        app.handle_key(Key::parse("shift+tab").unwrap());
        assert_eq!(focused_name(&mut app), "aa");
        assert_eq!(
            app.visible_names(),
            ["a", "aa", "aaa.txt", "ab.txt", "b", "c.txt"]
        );
    }

    #[test]
    fn tab_can_make_a_leaf_the_view_root() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Last);

        app.handle_key(Key::parse("tab").unwrap());

        assert_eq!(focused_name(&mut app), "c.txt");
        assert_eq!(app.visible_names(), ["c.txt"]);
    }

    #[test]
    fn escape_pops_one_root_at_a_time_then_quits() {
        let (_d, mut app) = app();
        app.handle_key(Key::parse("tab").unwrap()); // root "a"
        app.handle_key(Key::parse("l").unwrap()); // focus "aa"
        app.handle_key(Key::parse("tab").unwrap()); // root "aa"

        assert_eq!(app.handle_key(Key::parse("esc").unwrap()), Effect::None);
        assert_eq!(focused_name(&mut app), "aa");
        assert_eq!(app.visible_names(), ["a", "aa", "aaa.txt", "ab.txt"]);

        assert_eq!(app.handle_key(Key::parse("esc").unwrap()), Effect::None);
        assert_eq!(focused_name(&mut app), "aa");
        assert_eq!(
            app.visible_names(),
            ["a", "aa", "aaa.txt", "ab.txt", "b", "c.txt"]
        );

        assert_eq!(app.handle_key(Key::parse("esc").unwrap()), Effect::Quit);
    }

    #[test]
    fn sibling_navigation_skips_expanded_children() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Expand); // "a" expanded, children visible
        app.run_command(AppCommand::NextSibling);
        assert_eq!(focused_name(&mut app), "b");
        app.run_command(AppCommand::PrevSibling);
        assert_eq!(focused_name(&mut app), "a");
        // No previous sibling: no-op.
        app.run_command(AppCommand::PrevSibling);
        assert_eq!(focused_name(&mut app), "a");
    }

    #[test]
    fn first_and_last() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Last);
        assert_eq!(focused_name(&mut app), "c.txt");
        app.run_command(AppCommand::First);
        assert_eq!(focused_name(&mut app), "a");
    }

    #[test]
    fn paging_moves_focus_by_page_amounts() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::ExpandRecursively); // 6 visible rows
        app.page_height = 4;
        app.run_command(AppCommand::HalfPageDown);
        assert_eq!(focused_name(&mut app), "aaa.txt"); // moved 2
        app.run_command(AppCommand::PageDown);
        assert_eq!(focused_name(&mut app), "c.txt"); // clamped at end
        app.run_command(AppCommand::HalfPageUp);
        assert_eq!(focused_name(&mut app), "ab.txt");
        app.run_command(AppCommand::PageUp);
        assert_eq!(focused_name(&mut app), "a");
    }

    #[test]
    fn default_keys_drive_commands() {
        let (_d, mut app) = app();
        app.handle_key(Key::parse("j").unwrap());
        assert_eq!(focused_name(&mut app), "b");
        app.handle_key(Key::parse("k").unwrap());
        assert_eq!(focused_name(&mut app), "a");
        app.handle_key(Key::parse("l").unwrap());
        assert_eq!(app.visible_names().len(), 5);
        app.handle_key(Key::parse("h").unwrap());
        assert_eq!(app.visible_names().len(), 3);
        assert_eq!(app.handle_key(Key::parse("q").unwrap()), Effect::Quit);
        assert_eq!(app.handle_key(Key::parse("esc").unwrap()), Effect::Quit);
        assert_eq!(app.handle_key(Key::parse("ctrl+c").unwrap()), Effect::Quit);
    }

    #[test]
    fn gg_chord_goes_to_first_line() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Last);
        assert_eq!(app.handle_key(Key::parse("g").unwrap()), Effect::None);
        app.handle_key(Key::parse("g").unwrap());
        assert_eq!(focused_name(&mut app), "a");
        // A non-g key cancels the pending chord.
        app.run_command(AppCommand::Last);
        app.handle_key(Key::parse("g").unwrap());
        app.handle_key(Key::parse("j").unwrap());
        assert_eq!(focused_name(&mut app), "c.txt");
    }

    #[test]
    fn shift_g_goes_to_last_visible_line() {
        let (_d, mut app) = app();
        app.handle_key(Key::parse("G").unwrap());
        assert_eq!(focused_name(&mut app), "c.txt");
    }

    #[test]
    fn user_binding_produces_shell_effect_with_paths() {
        let (_d, tree) = fixture();
        let config = Config::parse("[ctrl+e]\nsh = \"vim $path\"\nexit = true\n").unwrap();
        let mut app = App::new(tree, &config, None);
        app.run_command(AppCommand::Down); // focus "b"
        let effect = app.handle_key(Key::parse("ctrl+e").unwrap());
        let Effect::RunShell {
            cmd,
            path,
            relpath,
            bg,
            exit,
        } = effect
        else {
            panic!("expected RunShell, got {effect:?}");
        };
        assert_eq!(cmd, "vim $path");
        assert!(std::path::Path::new(&path).is_absolute());
        assert!(std::path::Path::new(&path).ends_with("b"));
        assert_eq!(relpath, OsString::from("b"));
        assert!(!bg);
        assert!(exit);
    }

    #[test]
    fn user_binding_overrides_default() {
        let (_d, tree) = fixture();
        let config = Config::parse("[j]\ncmd = \"quit\"\n").unwrap();
        let mut app = App::new(tree, &config, None);
        assert_eq!(app.handle_key(Key::parse("j").unwrap()), Effect::Quit);
    }

    #[test]
    fn unbound_key_is_noop() {
        let (_d, mut app) = app();
        assert_eq!(app.handle_key(Key::parse("x").unwrap()), Effect::None);
    }

    #[test]
    fn initial_expand_depth_one_expands_top_level_only() {
        let (_d, tree) = fixture();
        let mut app = App::new(tree, &Config::default(), Some(ExpandSpec::Depth(1)));
        assert_eq!(
            app.visible_names(),
            ["a", "aa", "ab.txt", "b", "ba.txt", "c.txt"]
        );
    }

    #[test]
    fn initial_expand_all_expands_everything() {
        let (_d, tree) = fixture();
        let mut app = App::new(tree, &Config::default(), Some(ExpandSpec::All));
        assert_eq!(
            app.visible_names(),
            ["a", "aa", "aaa.txt", "ab.txt", "b", "ba.txt", "c.txt"]
        );
    }

    fn in_jump(app: &App) -> bool {
        matches!(app.mode, Mode::Jump(_))
    }

    #[test]
    fn slash_opens_the_jump_picker() {
        let (_d, mut app) = app();
        assert!(!in_jump(&app));
        app.handle_key(Key::parse("/").unwrap());
        assert!(in_jump(&app));
    }

    #[test]
    fn cancelling_the_picker_leaves_focus_untouched() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Down); // focus "b"
        assert_eq!(focused_name(&mut app), "b");
        app.handle_key(Key::parse("/").unwrap());
        app.handle_key(Key::parse("a").unwrap()); // type into the query
        app.handle_key(Key::parse("esc").unwrap());
        assert!(!in_jump(&app));
        assert_eq!(focused_name(&mut app), "b");
    }

    #[test]
    fn accepting_jumps_focus_and_expands_ancestors() {
        let (_d, mut app) = app();
        // Everything starts collapsed: only the top level is visible.
        assert_eq!(app.visible_names(), ["a", "b", "c.txt"]);
        app.handle_key(Key::parse("/").unwrap());
        for k in ["a", "a", "a"] {
            app.handle_key(Key::parse(k).unwrap()); // query "aaa" -> a/aa/aaa.txt
        }
        app.handle_key(Key::parse("enter").unwrap());
        assert!(!in_jump(&app));
        assert_eq!(focused_name(&mut app), "aaa.txt");
        // The path to it was expanded so the focused node is visible.
        assert!(app.visible_names().contains(&"aaa.txt".to_string()));
    }

    #[test]
    fn a_user_can_rebind_jump_off_slash() {
        let (_d, tree) = fixture();
        let config = Config::parse("[ctrl+p]\ncmd = \"jump\"\n").unwrap();
        let mut app = App::new(tree, &config, None);
        app.handle_key(Key::parse("ctrl+p").unwrap());
        assert!(in_jump(&app));
    }
}
