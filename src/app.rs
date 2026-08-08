//! I/O-free interaction coordinator. `App` owns the tree-list state, effective
//! keymap, expansion/focus behavior, modal jump picker, keybinding-panel state,
//! and the execution of configured `AppCommand`s.
//!
//! Key handling mutates application state or returns an `Effect` for the
//! executable to perform; this module neither draws nor touches the terminal,
//! filesystem, stdout, or subprocesses. Modal state machines live in their own
//! modules and are routed from here.

use std::collections::HashMap;
use std::ffi::OsString;

use tui_treelistview::{TreeListViewState, TreeQuery};

use crate::cli::ExpandSpec;
use crate::config::{AppCommand, Binding, BindingAction, Config};
use crate::jump::{Jump, JumpOutcome};
use crate::keybindings::{
    CLOSE_KEY, KeybindingEntry, KeybindingPanelState, TOGGLE_KEY, build_entries,
};
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
    /// Hand a filesystem path to the platform's default opener.
    Open(OsString),
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
    /// Blocking on index completion before the jump picker can open; a
    /// progress line renders until the sweep finishes (esc cancels).
    Indexing,
}

/// Nodes loaded per cooperative work quantum in [`App::do_work`].
const WORK_QUANTUM: usize = 500;

pub struct App {
    pub tree: Tree,
    pub state: TreeListViewState<NodeId>,
    pub query: TreeQuery,
    /// The current input mode; see [`Mode`].
    pub mode: Mode,
    keymap: HashMap<Key, Binding>,
    /// User entries followed by non-overridden built-ins, built once at startup.
    pub(crate) panel_entries: Vec<KeybindingEntry>,
    /// The leading entries that came from configuration rather than defaults.
    pub(crate) panel_user_entry_count: usize,
    pub keybinding_panel: KeybindingPanelState,
    /// Temporary view roots, from the original forest to the current subtree.
    root_history: Vec<Option<NodeId>>,
    /// Rows per screen; the UI updates this every frame.
    pub page_height: usize,
    /// Terminal default colors, when the terminal answered the startup query.
    pub palette: Option<crate::ui::Palette>,
}

impl App {
    pub fn new(tree: Tree, config: &Config, expand: Option<ExpandSpec>) -> Self {
        let mut builtin_keymap = Self::default_keymap();
        // `?` is reserved for the panel toggle: a configured `[?]` table is
        // accepted but silently ignored.
        let user_keymap: HashMap<_, _> = config
            .bindings
            .iter()
            .filter(|&(&key, _)| key != TOGGLE_KEY)
            .map(|(&key, binding)| (key, binding.clone()))
            .collect();
        for key in user_keymap.keys() {
            builtin_keymap.remove(key);
        }

        let mut panel_entries = build_entries(&user_keymap);
        let panel_user_entry_count = panel_entries.len();
        panel_entries.extend(build_entries(&builtin_keymap));

        let mut keymap = builtin_keymap;
        keymap.extend(user_keymap);
        let mut app = Self {
            tree,
            state: TreeListViewState::with_capacity(0),
            query: TreeQuery::new(),
            mode: Mode::Normal,
            keymap,
            panel_entries,
            panel_user_entry_count,
            keybinding_panel: KeybindingPanelState::default(),
            root_history: Vec::new(),
            page_height: 20,
            palette: None,
        };
        match expand {
            None => {}
            Some(spec) => {
                // Explicit expansion wants the whole tree present up front.
                app.tree.index_all();
                let branches: Vec<_> = app.tree.branches().collect();
                for (id, parent) in branches {
                    let expand = match spec {
                        ExpandSpec::All => true,
                        ExpandSpec::Depth(n) => app.tree.depth(id) < n,
                    };
                    if expand {
                        app.state.set_expanded(id, parent, true);
                    }
                }
            }
        }
        // A single-rooted tree opens with its first level showing.
        if app.tree.root_ids().len() == 1 {
            let root = app.tree.root_ids()[0];
            app.tree.ensure_children(root);
            if !app.tree.is_leaf(root) {
                app.state
                    .set_expanded(root, app.tree.view_parent(root), true);
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
            help: None,
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
            (&["space"], AppCommand::Toggle),
            (&["ctrl+space"], AppCommand::ToggleRecursively),
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
            (&["z"], AppCommand::Center),
            (&["g"], AppCommand::First),
            (&["G"], AppCommand::Last),
            (&["/"], AppCommand::Jump),
            (&["o"], AppCommand::Open),
            (&["?"], AppCommand::ToggleKeybindingPanel),
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
            .map(|id| self.tree.name(id))
            .collect()
    }

    /// Handle a normalized key through the active mode and effective keymap.
    pub fn handle_key(&mut self, key: Key) -> Effect {
        let _span = crate::profile::span("app::handle_key");
        // Only cancellation means anything while the index builds for `/`.
        if let Mode::Indexing = self.mode {
            if key == Key::parse("esc").unwrap() || key == Key::parse("ctrl+c").unwrap() {
                self.mode = Mode::Normal;
            }
            return Effect::None;
        }
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

        if self.keybinding_panel.is_open() && key == CLOSE_KEY {
            self.keybinding_panel.close();
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
                        path: self.tree.path(id),
                        relpath: self.tree.relpath(id),
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
                    self.tree.ensure_children(id);
                    let parent = self.tree.view_parent(id);
                    if self.tree.is_leaf(id) {
                        // Nothing to open: step along to the next sibling.
                        self.move_sibling(1);
                    } else if self.state.node_is_expanded(id, parent) {
                        self.state.select_id(Some(self.tree.children_of(id)[0]));
                    } else {
                        self.expand_and_reveal_children(id);
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
                    self.tree.ensure_children(id);
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
            AppCommand::Toggle => self.toggle_focused(false),
            AppCommand::ToggleRecursively => self.toggle_focused(true),
            AppCommand::Select => {
                if let Some(id) = self.focused_id() {
                    self.tree.ensure_children(id);
                    if self.tree.is_leaf(id) {
                        return Effect::PrintAndExit(self.tree.output(id));
                    }
                    self.expand_and_reveal_children(id);
                }
            }
            AppCommand::Accept => {
                if let Some(id) = self.focused_id() {
                    return Effect::PrintAndExit(self.tree.output(id));
                }
            }
            AppCommand::AcceptAlternate => {
                if let Some(id) = self.focused_id() {
                    return Effect::PrintAndExit(self.tree.alternate_output(id));
                }
            }
            AppCommand::Descend => {
                if let Some(id) = self.focused_branch() {
                    self.expand_and_reveal_children(id);
                    let first_child = self.tree.children_of(id)[0];
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
            AppCommand::Center => self.center_focused(),
            AppCommand::First => {
                self.state.select_first();
            }
            AppCommand::Last => {
                self.state.select_last();
            }
            AppCommand::Jump => {
                // The picker only ever sees a complete index; block on the
                // progress screen until the sweep catches up.
                if self.tree.fully_indexed() {
                    self.mode = Mode::Jump(Jump::open(&self.tree));
                } else {
                    self.mode = Mode::Indexing;
                }
            }
            AppCommand::Open => {
                // Containers expand rather than open, and a JSON Pointer is
                // not a path the desktop knows how to follow.
                if let Some(id) = self.focused_id()
                    && self.tree.is_leaf(id)
                    && let Some(path) = self.tree.filesystem_path(id)
                {
                    return Effect::Open(path);
                }
            }
            AppCommand::ToggleKeybindingPanel => self.keybinding_panel.toggle(),
            AppCommand::Quit => return Effect::Quit,
        }
        Effect::None
    }

    /// The focused node if it is expandable, its children materialized.
    fn focused_branch(&mut self) -> Option<NodeId> {
        let id = self.focused_id()?;
        self.tree.ensure_children(id);
        (!self.tree.is_leaf(id)).then_some(id)
    }

    /// One cooperative quantum of index building, run by the event loop
    /// between input polls: load what is on screen first, then advance the
    /// arena-order sweep. Returns true while more work remains. This is the
    /// background indexer — cooperative rather than threaded, so the tree
    /// stays single-threaded and lock-free; a worker thread could later slot
    /// in behind this same seam.
    pub fn do_work(&mut self) -> bool {
        let _span = crate::profile::span("app::do_work");
        if !self.tree.fully_indexed() {
            self.state.ensure_projection(&self.tree, &self.query);
            let visible: Vec<NodeId> = self.state.visible_ids().collect();
            let mut budget = WORK_QUANTUM;
            for id in visible {
                if budget == 0 {
                    break;
                }
                if self.tree.ensure_children(id) {
                    budget -= 1;
                }
            }
            self.tree.index_some(budget);
        }
        if matches!(self.mode, Mode::Indexing) && self.tree.fully_indexed() {
            self.mode = Mode::Jump(Jump::open(&self.tree));
        }
        !self.tree.fully_indexed()
    }

    /// Record a non-fatal failure from an effect the event loop performed. It
    /// joins the source errors in the banner and the exit report, so a missing
    /// opener is reported rather than ending the session.
    pub fn report_error(&mut self, message: String) {
        self.tree.record_error(message);
    }

    /// Whether the event loop should keep scheduling work quanta.
    pub fn has_work(&self) -> bool {
        !self.tree.fully_indexed()
    }

    /// Flip the focused container's expansion, leaving focus on it. A leaf has
    /// nothing to toggle. The direction comes from the focused node's own
    /// state, so a recursive toggle on an open container shuts it rather than
    /// opening what is still shut underneath.
    fn toggle_focused(&mut self, recursive: bool) {
        let Some(id) = self.focused_id() else { return };
        self.tree.ensure_children(id);
        if self.tree.is_leaf(id) {
            return;
        }
        let parent = self.tree.view_parent(id);
        let expand = !self.state.node_is_expanded(id, parent);
        if recursive {
            self.set_expanded_recursively(id, expand);
        } else if expand {
            self.expand_and_reveal_children(id);
        } else {
            self.state.set_expanded(id, parent, false);
        }
    }

    fn set_expanded_recursively(&mut self, root: NodeId, expanded: bool) {
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            if expanded {
                self.tree.ensure_children(id);
            }
            if !self.tree.is_leaf(id) {
                self.state
                    .set_expanded(id, self.tree.view_parent(id), expanded);
                stack.extend_from_slice(self.tree.children_of(id));
            }
        }
        if expanded {
            self.reveal_expanded_children(root);
        }
    }

    fn expand_and_reveal_children(&mut self, id: NodeId) {
        self.state.set_expanded(id, self.tree.view_parent(id), true);
        self.reveal_expanded_children(id);
    }

    /// If expansion pushes a direct child below the viewport, keep both ends
    /// visible when they fit. Otherwise anchor the expanded container at the
    /// top so the user can read into the oversized branch from its beginning.
    fn reveal_expanded_children(&mut self, id: NodeId) {
        let height = self.page_height;
        if height == 0 {
            return;
        }
        self.state.ensure_projection(&self.tree, &self.query);
        let Some(container_index) = self.state.visible_index_of(id) else {
            return;
        };
        let Some(last_child_index) = self
            .tree
            .children_of(id)
            .last()
            .and_then(|&child| self.state.visible_index_of(child))
        else {
            return;
        };
        if last_child_index < self.state.offset().saturating_add(height) {
            return;
        }

        let branch_rows = last_child_index.saturating_sub(container_index) + 1;
        let offset = if branch_rows <= height {
            last_child_index + 1 - height
        } else {
            container_index
        };
        self.state.set_offset(offset);
    }

    fn push_root(&mut self) {
        let Some(id) = self.focused_id() else {
            return;
        };
        if self.tree.view_root() == Some(id) {
            return;
        }
        self.tree.ensure_children(id);

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
                .set_expanded(current_root, self.tree.parent(current_root), expanded);
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
        let siblings = match self.tree.parent(id) {
            Some(parent) => self.tree.children_of(parent),
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

    fn center_focused(&mut self) {
        if self.page_height == 0 {
            return;
        }
        let Some(selected) = self.state.selected_index() else {
            return;
        };
        let maximum = self.state.visible_len().saturating_sub(self.page_height);
        let centered = selected.saturating_sub(self.page_height / 2);
        self.state.set_offset(centered.min(maximum));
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

    /// Builds a branch below two leaves so expansion can overflow a viewport:
    ///   a/
    ///   b/
    ///   d/
    ///     d1.txt
    ///     d2.txt
    fn app_with_branch_at_bottom() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir(p.join("a")).unwrap();
        std::fs::create_dir(p.join("b")).unwrap();
        std::fs::create_dir(p.join("d")).unwrap();
        std::fs::write(p.join("d/d1.txt"), "").unwrap();
        std::fs::write(p.join("d/d2.txt"), "").unwrap();
        let tree = fstree::scan(p, false).unwrap();
        (dir, App::new(tree, &Config::default(), None))
    }

    fn focused_name(app: &mut App) -> String {
        let id = app.focused_id().expect("something focused");
        app.tree.name(id)
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
    fn expanding_scrolls_just_enough_to_reveal_last_child_when_branch_fits() {
        let (_d, mut app) = app_with_branch_at_bottom();
        app.page_height = 4;
        app.run_command(AppCommand::Down);
        app.run_command(AppCommand::Down); // focus "d" on the viewport's last row

        app.run_command(AppCommand::Expand);

        assert_eq!(app.state.offset(), 1);
    }

    #[test]
    fn expanding_an_oversized_branch_places_container_at_viewport_top() {
        let (_d, mut app) = app_with_branch_at_bottom();
        app.page_height = 2;
        app.run_command(AppCommand::Down);
        app.run_command(AppCommand::Down); // focus "d" on the viewport's last row

        app.run_command(AppCommand::Expand);

        assert_eq!(app.state.offset(), 2);
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
    fn space_toggles_a_container_open_and_shut() {
        let (_d, mut app) = app();

        app.handle_key(Key::parse("space").unwrap());
        assert_eq!(app.visible_names(), ["a", "aa", "ab.txt", "b", "c.txt"]);
        assert_eq!(focused_name(&mut app), "a");

        app.handle_key(Key::parse("space").unwrap());
        assert_eq!(app.visible_names(), ["a", "b", "c.txt"]);
        assert_eq!(focused_name(&mut app), "a");
    }

    #[test]
    fn space_on_a_leaf_does_nothing() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Last); // focus "c.txt"

        app.handle_key(Key::parse("space").unwrap());

        assert_eq!(focused_name(&mut app), "c.txt");
        assert_eq!(app.visible_names(), ["a", "b", "c.txt"]);
    }

    #[test]
    fn ctrl_space_toggles_a_container_recursively() {
        let (_d, mut app) = app();

        app.handle_key(Key::parse("ctrl+space").unwrap());
        assert_eq!(
            app.visible_names(),
            ["a", "aa", "aaa.txt", "ab.txt", "b", "c.txt"]
        );
        assert_eq!(focused_name(&mut app), "a");

        app.handle_key(Key::parse("ctrl+space").unwrap());
        assert_eq!(app.visible_names(), ["a", "b", "c.txt"]);
        // Descendant expansion was cleared, not just hidden.
        app.run_command(AppCommand::Expand);
        assert_eq!(app.visible_names(), ["a", "aa", "ab.txt", "b", "c.txt"]);
    }

    #[test]
    fn ctrl_space_direction_follows_the_focused_container() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Expand); // "a" expanded, "aa" still shut

        // "a" is open, so the recursive toggle shuts it rather than reopening.
        app.handle_key(Key::parse("ctrl+space").unwrap());

        assert_eq!(app.visible_names(), ["a", "b", "c.txt"]);
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
    fn o_opens_the_focused_leaf_with_its_absolute_path() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Last); // focus leaf "c.txt"

        let effect = app.handle_key(Key::parse("o").unwrap());

        let Effect::Open(path) = effect else {
            panic!("expected Open, got {effect:?}");
        };
        assert!(std::path::Path::new(&path).is_absolute());
        assert!(std::path::Path::new(&path).ends_with("c.txt"));
    }

    #[test]
    fn o_on_a_container_does_nothing() {
        let (_d, mut app) = app();
        assert_eq!(focused_name(&mut app), "a"); // a directory

        assert_eq!(app.handle_key(Key::parse("o").unwrap()), Effect::None);
    }

    #[test]
    fn o_on_a_json_leaf_does_nothing() {
        // A JSON Pointer is not something the desktop can open.
        let tree = crate::json_tree::from_reader(r#"{"a": 1}"#.as_bytes()).unwrap();
        let mut app = App::new(tree, &Config::default(), None);
        app.run_command(AppCommand::Last);

        assert_eq!(app.handle_key(Key::parse("o").unwrap()), Effect::None);
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
    fn z_centers_the_focused_node_in_the_viewport() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::ExpandRecursively); // 6 visible rows
        app.page_height = 3;
        for _ in 0..3 {
            app.run_command(AppCommand::Down); // focus row 3
        }
        app.state.set_offset(0);

        app.handle_key(Key::parse("z").unwrap());

        assert_eq!(focused_name(&mut app), "ab.txt");
        assert_eq!(app.state.offset(), 2);
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
    fn g_goes_to_first_line_without_a_chord() {
        let (_d, mut app) = app();
        app.run_command(AppCommand::Last);
        assert_eq!(app.handle_key(Key::parse("g").unwrap()), Effect::None);
        assert_eq!(focused_name(&mut app), "a");
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
    fn user_can_override_the_new_g_binding() {
        let (_d, tree) = fixture();
        let config = Config::parse("[g]\ncmd = \"quit\"\n").unwrap();
        let mut app = App::new(tree, &config, None);
        assert_eq!(app.handle_key(Key::parse("g").unwrap()), Effect::Quit);
    }

    #[test]
    fn question_mark_is_reserved_and_toggles_the_panel() {
        let (_d, tree) = fixture();
        let config = Config::parse("[?]\ncmd = \"quit\"\nhelp = \"Wrong\"\n").unwrap();
        let mut app = App::new(tree, &config, None);

        // The `[?]` table was dropped: the panel lists the reserved binding,
        // not the configured one.
        let question: Vec<_> = app
            .panel_entries
            .iter()
            .filter(|entry| entry.key == Key::parse("?").unwrap())
            .collect();
        assert_eq!(question.len(), 1);
        assert_eq!(question[0].description, "Shortcuts");

        assert!(!app.keybinding_panel.is_open());
        assert_eq!(app.handle_key(Key::parse("?").unwrap()), Effect::None);
        assert!(app.keybinding_panel.is_open());
        assert_eq!(app.handle_key(Key::parse("?").unwrap()), Effect::None);
        assert!(!app.keybinding_panel.is_open());
    }

    #[test]
    fn open_panel_stays_open_while_bindings_run_and_escape_only_closes_it() {
        let (_d, mut app) = app();
        app.handle_key(Key::parse("?").unwrap());

        app.handle_key(Key::parse("j").unwrap());
        assert_eq!(focused_name(&mut app), "b");
        assert!(app.keybinding_panel.is_open());

        assert_eq!(app.handle_key(Key::parse("esc").unwrap()), Effect::None);
        assert!(!app.keybinding_panel.is_open());
        assert_eq!(focused_name(&mut app), "b");
    }

    #[test]
    fn jump_owns_question_mark_and_escape_without_dismissing_the_panel() {
        let (_d, mut app) = app();
        while app.do_work() {}
        app.handle_key(Key::parse("?").unwrap());
        app.handle_key(Key::parse("/").unwrap());
        app.handle_key(Key::parse("?").unwrap());

        let Mode::Jump(jump) = &app.mode else {
            panic!("expected jump mode");
        };
        assert_eq!(jump.query(), "?");
        assert!(app.keybinding_panel.is_open());

        app.handle_key(Key::parse("esc").unwrap());
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.keybinding_panel.is_open());
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
        while app.do_work() {} // the picker waits for a complete index
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
        while app.do_work() {} // the picker waits for a complete index
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
        while app.do_work() {} // the picker waits for a complete index
        app.handle_key(Key::parse("ctrl+p").unwrap());
        assert!(in_jump(&app));
    }

    #[test]
    fn a_single_rooted_tree_opens_with_its_first_level_expanded() {
        use crate::tree::ActionValues;
        let mut tree = Tree::new();
        let root = tree.push(None, "root", true, ActionValues::new("", "", ""));
        tree.push(Some(root), "child", false, ActionValues::new("", "", ""));
        let mut app = App::new(tree, &Config::default(), None);

        assert_eq!(app.visible_names(), ["root", "child"]);
        assert_eq!(focused_name(&mut app), "root");
    }

    #[test]
    fn expanding_an_unindexed_container_materializes_its_children() {
        let tree =
            crate::json_tree::from_reader(r#"{"users": [1, 2], "n": 3}"#.as_bytes()).unwrap();
        let mut app = App::new(tree, &Config::default(), None);
        assert_eq!(app.visible_names(), ["users [2]", "n: 3"]);

        app.handle_key(Key::parse("l").unwrap());

        assert_eq!(
            app.visible_names(),
            ["users [2]", "[0]: 1", "[1]: 2", "n: 3"]
        );
    }

    #[test]
    fn jump_blocks_on_an_incomplete_index_and_opens_when_done() {
        let tree =
            crate::json_tree::from_reader(r#"{"a": {"b": 1}, "c": {"d": 2}}"#.as_bytes()).unwrap();
        let mut app = App::new(tree, &Config::default(), None);
        assert!(app.has_work());

        app.handle_key(Key::parse("/").unwrap());
        assert!(matches!(app.mode, Mode::Indexing));
        // Keys other than cancel are ignored while the index builds.
        assert_eq!(app.handle_key(Key::parse("j").unwrap()), Effect::None);
        assert!(matches!(app.mode, Mode::Indexing));

        while app.do_work() {}

        assert!(matches!(app.mode, Mode::Jump(_)));
        assert!(!app.has_work());
    }

    #[test]
    fn escape_cancels_the_indexing_wait() {
        let tree =
            crate::json_tree::from_reader(r#"{"a": {"b": 1}, "c": {"d": 2}}"#.as_bytes()).unwrap();
        let mut app = App::new(tree, &Config::default(), None);
        app.handle_key(Key::parse("/").unwrap());
        assert!(matches!(app.mode, Mode::Indexing));

        app.handle_key(Key::parse("esc").unwrap());

        assert!(matches!(app.mode, Mode::Normal));
    }
}
