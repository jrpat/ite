//! The jump picker: a full-screen fuzzy finder over every node's path.
//!
//! Pressing `/` opens a [`Jump`] over the current tree. Typing filters a flat,
//! score-ranked list of candidate paths; accepting one asks the app to move
//! focus to that node (expanding the containers along the way). The picker owns
//! no I/O and no rendering — it is a pure state machine, so it is unit-tested
//! exactly like the rest of `app.rs`. Where it is drawn is the renderer's
//! concern (see `ui::render_jump` and ADR-0002); the picker only exposes state.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::keys::Key;
use crate::tree::{NodeId, Tree};

/// A node offered to the picker. Matched on `path` (the node's `relpath`: a
/// root-relative path for a dir scan, a JSON Pointer for `--json`).
struct Candidate {
    id: NodeId,
    path: String,
}

/// One ranked result: the matched node, its score, and the character indices in
/// its path that matched (sorted + deduplicated, for highlighting).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub id: NodeId,
    pub score: u32,
    pub indices: Vec<u32>,
}

/// What the caller must do after handing a key to the picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JumpOutcome {
    /// Stay in the picker and redraw.
    Stay,
    /// Move focus to this node (expanding its ancestors) and close the picker.
    Accept(NodeId),
    /// Close the picker; focus is untouched, so the caller returns to it.
    Cancel,
}

/// The jump picker's state: the query line (an editable `tui_input::Input`), the
/// candidate set (built once on open), the ranked results, and view state
/// (selection + scroll).
pub struct Jump {
    input: Input,
    candidates: Vec<Candidate>,
    results: Vec<Match>,
    /// Index into `results` of the highlighted row.
    selected: usize,
    /// First `results` row shown; kept so the selection stays visible.
    scroll: usize,
    /// Result rows the viewport can show; the renderer updates it each frame.
    page_height: usize,
    matcher: Matcher,
}

impl Jump {
    /// Open a picker over every node in `tree`, ranked for the empty query
    /// (i.e. all candidates in tree order).
    pub fn open(tree: &Tree) -> Self {
        let candidates = (0..tree.len())
            .map(|id| Candidate {
                id,
                path: tree.node(id).action.relpath.to_string_lossy().into_owned(),
            })
            .collect();
        let mut jump = Self {
            input: Input::default(),
            candidates,
            results: Vec::new(),
            selected: 0,
            scroll: 0,
            page_height: 10,
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
        };
        jump.rank();
        jump
    }

    /// Route a key to the picker. Result-navigation, accept, and cancel keys are
    /// handled here; everything else is offered to the query line's `tui_input`
    /// editor. All of these are fixed (not the user's configurable keymap — see
    /// the v1 boundary in `app.rs`).
    pub fn handle_key(&mut self, key: Key) -> JumpOutcome {
        let ctrl = key.mods == KeyModifiers::CONTROL;
        match key.code {
            KeyCode::Esc => return JumpOutcome::Cancel,
            KeyCode::Char('c') if ctrl => return JumpOutcome::Cancel,
            KeyCode::Enter => {
                return match self.results.get(self.selected) {
                    Some(m) => JumpOutcome::Accept(m.id),
                    None => JumpOutcome::Stay,
                };
            }
            KeyCode::Down => self.move_selection(1),
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('j' | 'n') if ctrl => self.move_selection(1),
            KeyCode::Char('k' | 'p') if ctrl => self.move_selection(-1),
            // Query editing: repackage the key as a crossterm event and let
            // tui-input's own backend classify and apply it — we don't duplicate
            // its map of which keys are edits. Re-rank only when the value
            // actually changed (cursor-only moves don't re-filter).
            _ => {
                let event = Event::Key(KeyEvent::new(key.code, key.mods));
                if self.input.handle_event(&event).is_some_and(|change| change.value) {
                    self.rank();
                }
            }
        }
        JumpOutcome::Stay
    }

    // Fuzzy scoring recomputes over the full candidate list on every keystroke.
    // Incremental narrowing (re-scoring only the previous keystroke's survivors)
    // was considered and deliberately deferred: it speeds up only keystrokes
    // 2..N, never the first full scan, and it adds a survivor cache plus
    // correctness fallbacks for negation (`!atom`) and non-append edits. At
    // ite's realistic sizes a full recompute is imperceptible, and parallel
    // scoring is likewise deferred. See docs/adr/0001-fuzzy-jump-matching.md.
    fn rank(&mut self) {
        self.selected = 0;
        self.scroll = 0;
        self.results.clear();
        if self.input.value().is_empty() {
            // Empty query: every candidate, in tree order, unhighlighted.
            self.results = self
                .candidates
                .iter()
                .map(|c| Match {
                    id: c.id,
                    score: 0,
                    indices: Vec::new(),
                })
                .collect();
            return;
        }
        let pattern = Pattern::parse(self.input.value(), CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        for c in &self.candidates {
            let mut indices = Vec::new();
            let haystack = Utf32Str::new(&c.path, &mut buf);
            if let Some(score) = pattern.indices(haystack, &mut self.matcher, &mut indices) {
                // `indices` are appended per atom, unsorted; sort+dedup so the
                // renderer can group matched runs.
                indices.sort_unstable();
                indices.dedup();
                self.results.push(Match {
                    id: c.id,
                    score,
                    indices,
                });
            }
        }
        // Rank: score desc, then shorter path, then tree order — deterministic.
        let candidates = &self.candidates;
        self.results.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| candidates[a.id].path.len().cmp(&candidates[b.id].path.len()))
                .then_with(|| a.id.cmp(&b.id))
        });
    }

    fn move_selection(&mut self, delta: isize) {
        if self.results.is_empty() {
            return;
        }
        let last = self.results.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
        self.follow_selection();
    }

    /// Scroll the minimum amount to keep the selection inside the viewport.
    fn follow_selection(&mut self) {
        let height = self.page_height.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + height {
            self.scroll = self.selected + 1 - height;
        }
    }

    // --- Accessors for the renderer ---

    pub fn query(&self) -> &str {
        self.input.value()
    }

    /// The caret's display column within the query text (accounts for wide
    /// characters); pairs with [`Jump::visual_scroll`] for rendering.
    pub fn visual_cursor(&self) -> usize {
        self.input.visual_cursor()
    }

    /// The horizontal scroll offset (in display columns) that keeps the caret
    /// visible in a query field `width` columns wide.
    pub fn visual_scroll(&self, width: usize) -> usize {
        self.input.visual_scroll(width)
    }

    pub fn results(&self) -> &[Match] {
        &self.results
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Number of candidates matched by the current query.
    pub fn matched(&self) -> usize {
        self.results.len()
    }

    /// Total number of candidates (every node).
    pub fn total(&self) -> usize {
        self.candidates.len()
    }

    /// The path text for a node id (what was matched, and what is displayed).
    pub fn path(&self, id: NodeId) -> &str {
        &self.candidates[id].path
    }

    /// Tell the picker how many result rows the viewport can show, keeping the
    /// selection visible. Called by the renderer each frame.
    pub fn set_viewport(&mut self, rows: usize) {
        self.page_height = rows.max(1);
        self.follow_selection();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::ActionValues;

    /// A flat tree whose nodes carry the given relpaths (hierarchy is irrelevant
    /// to ranking, which matches on `relpath` alone). Node ids are `0..paths.len`.
    fn jump_over(paths: &[&str]) -> Jump {
        let mut tree = Tree::new();
        for p in paths {
            tree.push(None, *p, false, ActionValues::new(*p, *p, *p));
        }
        Jump::open(&tree)
    }

    fn ch(c: char) -> Key {
        Key::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> Key {
        Key::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn key(code: KeyCode) -> Key {
        Key::new(code, KeyModifiers::NONE)
    }

    fn type_str(j: &mut Jump, s: &str) {
        for c in s.chars() {
            j.handle_key(ch(c));
        }
    }

    fn result_ids(j: &Jump) -> Vec<NodeId> {
        j.results().iter().map(|m| m.id).collect()
    }

    #[test]
    fn empty_query_lists_all_candidates_in_tree_order() {
        let j = jump_over(&["a", "b", "c"]);
        assert_eq!(result_ids(&j), vec![0, 1, 2]);
        assert_eq!(j.total(), 3);
        assert_eq!(j.matched(), 3);
    }

    #[test]
    fn typing_filters_to_fuzzy_matches() {
        let mut j = jump_over(&["src/app.rs", "apple/pie", "README"]);
        type_str(&mut j, "apprs");
        // Only "src/app.rs" contains a-p-p-r-s as a subsequence.
        assert_eq!(result_ids(&j), vec![0]);
    }

    #[test]
    fn ranking_puts_the_stronger_match_first() {
        let mut j = jump_over(&["app.rs", "zz/app.rs"]);
        type_str(&mut j, "app");
        // Both match; the shorter, prefix-anchored path outranks the buried one.
        assert_eq!(result_ids(&j), vec![0, 1]);
    }

    #[test]
    fn extended_syntax_prefix_anchor() {
        let mut j = jump_over(&["src/a", "x/src"]);
        type_str(&mut j, "^src");
        assert_eq!(result_ids(&j), vec![0]);
    }

    #[test]
    fn extended_syntax_negation_excludes() {
        let mut j = jump_over(&["app.rs", "app_test.rs"]);
        type_str(&mut j, "app !test");
        assert_eq!(result_ids(&j), vec![0]);
    }

    #[test]
    fn space_is_a_second_and_atom() {
        let mut j = jump_over(&["src/app.rs", "src/xx"]);
        type_str(&mut j, "src rs");
        // Needs both "src" AND "rs"; only src/app.rs has both.
        assert_eq!(result_ids(&j), vec![0]);
    }

    #[test]
    fn match_indices_are_recorded_for_highlighting() {
        let mut j = jump_over(&["abcd"]);
        type_str(&mut j, "abc");
        assert_eq!(j.results()[0].indices, vec![0, 1, 2]);
    }

    #[test]
    fn navigation_keys_move_and_clamp_selection() {
        let mut j = jump_over(&["a1", "a2", "a3"]);
        type_str(&mut j, "a");
        assert_eq!(j.selected(), 0);
        j.handle_key(ctrl('n'));
        assert_eq!(j.selected(), 1);
        j.handle_key(key(KeyCode::Down));
        assert_eq!(j.selected(), 2);
        j.handle_key(ctrl('n')); // clamp at end
        assert_eq!(j.selected(), 2);
        j.handle_key(ctrl('p'));
        assert_eq!(j.selected(), 1);
        j.handle_key(ctrl('k'));
        assert_eq!(j.selected(), 0);
        j.handle_key(key(KeyCode::Up)); // clamp at start
        assert_eq!(j.selected(), 0);
    }

    #[test]
    fn enter_accepts_the_selected_result() {
        let mut j = jump_over(&["a1", "a2", "a3"]);
        type_str(&mut j, "a");
        j.handle_key(ctrl('n')); // select id 1
        assert_eq!(j.handle_key(key(KeyCode::Enter)), JumpOutcome::Accept(1));
    }

    #[test]
    fn enter_with_no_matches_is_a_noop() {
        let mut j = jump_over(&["a", "b"]);
        type_str(&mut j, "zzzz");
        assert_eq!(j.matched(), 0);
        assert_eq!(j.handle_key(key(KeyCode::Enter)), JumpOutcome::Stay);
    }

    #[test]
    fn esc_and_ctrl_c_cancel() {
        let mut j = jump_over(&["a"]);
        assert_eq!(j.handle_key(key(KeyCode::Esc)), JumpOutcome::Cancel);
        assert_eq!(j.handle_key(ctrl('c')), JumpOutcome::Cancel);
    }

    #[test]
    fn backspace_edits_the_query_and_reranks() {
        let mut j = jump_over(&["app.rs", "api.rs"]);
        type_str(&mut j, "app");
        assert_eq!(result_ids(&j), vec![0]);
        j.handle_key(key(KeyCode::Backspace)); // query now "ap"
        assert_eq!(j.query(), "ap");
        assert_eq!(result_ids(&j), vec![0, 1]);
    }

    #[test]
    fn a_new_query_char_resets_the_selection() {
        let mut j = jump_over(&["a1", "a2", "a3"]);
        type_str(&mut j, "a");
        j.handle_key(ctrl('n'));
        j.handle_key(ctrl('n'));
        assert_eq!(j.selected(), 2);
        type_str(&mut j, "3"); // query "a3" reranks
        assert_eq!(j.selected(), 0);
    }

    #[test]
    fn scroll_follows_selection_within_a_small_viewport() {
        let mut j = jump_over(&["a1", "a2", "a3", "a4", "a5"]);
        type_str(&mut j, "a");
        j.set_viewport(2); // only two rows visible
        for _ in 0..4 {
            j.handle_key(ctrl('n'));
        }
        assert_eq!(j.selected(), 4);
        // Selection must be within [scroll, scroll + 2).
        assert!(j.scroll() <= 4 && 4 < j.scroll() + 2, "scroll={}", j.scroll());
    }

    #[test]
    fn cursor_can_move_and_edit_mid_query() {
        let mut j = jump_over(&["abc"]);
        type_str(&mut j, "ac");
        j.handle_key(key(KeyCode::Left)); // caret between 'a' and 'c'
        assert_eq!(j.visual_cursor(), 1);
        type_str(&mut j, "b"); // insert at the caret
        assert_eq!(j.query(), "abc");
        assert_eq!(result_ids(&j), vec![0]);
    }

    #[test]
    fn home_end_move_the_caret_to_the_bounds() {
        let mut j = jump_over(&["x"]);
        type_str(&mut j, "abc");
        j.handle_key(key(KeyCode::Home));
        assert_eq!(j.visual_cursor(), 0);
        j.handle_key(key(KeyCode::End));
        assert_eq!(j.visual_cursor(), 3);
    }

    #[test]
    fn ctrl_w_deletes_the_previous_word() {
        let mut j = jump_over(&["foo", "bar"]);
        type_str(&mut j, "foo bar");
        j.handle_key(ctrl('w')); // kill "bar"
        assert!(j.query().starts_with("foo"));
        assert!(!j.query().contains("bar"), "query: {:?}", j.query());
    }

    #[test]
    fn moving_the_caret_does_not_rerank_or_reset_selection() {
        let mut j = jump_over(&["a1", "a2", "a3"]);
        type_str(&mut j, "a");
        j.handle_key(ctrl('n'));
        assert_eq!(j.selected(), 1);
        j.handle_key(key(KeyCode::Left)); // cursor-only: no re-rank
        assert_eq!(j.selected(), 1);
        assert_eq!(result_ids(&j), vec![0, 1, 2]);
    }
}
