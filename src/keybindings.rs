//! Keybinding-panel entries, formatting, layout, and scroll state.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Position, Rect};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::config::{Binding, BindingAction};
use crate::keys::Key;

/// The reserved key that toggles the panel; user configs cannot rebind it
/// (see `App::new`). Already in `Key`'s normalized form.
pub const TOGGLE_KEY: Key = Key {
    code: KeyCode::Char('?'),
    mods: KeyModifiers::NONE,
};
/// Closes the open panel before its normal binding runs (see `App::handle_key`).
pub const CLOSE_KEY: Key = Key {
    code: KeyCode::Esc,
    mods: KeyModifiers::NONE,
};

pub const GRID_GAP: usize = 2;
const MIN_COLUMN_WIDTH: usize = 24;
const MAX_KEY_WIDTH: usize = 12;
pub const MAX_PANEL_ROWS: usize = 20;
const SHELL_DESCRIPTION_WIDTH: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyLabel {
    pub full: String,
    pub base: String,
    pub modified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeybindingEntry {
    pub key: Key,
    pub label: KeyLabel,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeybindingGrid {
    /// Indices into the entries slice, one inner vec per on-screen row,
    /// filled down columns before moving right.
    pub rows: Vec<Vec<usize>>,
    pub column_width: usize,
    pub key_width: usize,
}

#[derive(Clone, Debug, Default)]
pub struct KeybindingPanelState {
    open: bool,
    scroll: usize,
    area: Option<Rect>,
    total_rows: usize,
    viewport_rows: usize,
}

impl KeybindingPanelState {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        self.open = true;
        self.scroll = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.clear_layout();
    }

    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.max_scroll());
    }

    pub fn area(&self) -> Option<Rect> {
        self.area
    }

    pub fn contains(&self, column: u16, row: u16) -> bool {
        self.area
            .is_some_and(|area| area.contains(Position::new(column, row)))
    }

    pub fn record_layout(&mut self, area: Rect, total_rows: usize, viewport_rows: usize) {
        self.area = Some(area);
        self.total_rows = total_rows;
        self.viewport_rows = viewport_rows;
        self.scroll = self.scroll.min(self.max_scroll());
    }

    pub fn clear_layout(&mut self) {
        self.area = None;
        self.total_rows = 0;
        self.viewport_rows = 0;
    }

    fn max_scroll(&self) -> usize {
        self.total_rows.saturating_sub(self.viewport_rows)
    }
}

fn format_key(key: Key) -> KeyLabel {
    let base = match key.code {
        KeyCode::Backspace => "bksp".to_owned(),
        KeyCode::Enter => "ret".to_owned(),
        KeyCode::Left => "←".to_owned(),
        KeyCode::Right => "→".to_owned(),
        KeyCode::Up => "↑".to_owned(),
        KeyCode::Down => "↓".to_owned(),
        KeyCode::Home => "home".to_owned(),
        KeyCode::End => "end".to_owned(),
        KeyCode::PageUp => "pgup".to_owned(),
        KeyCode::PageDown => "pgdn".to_owned(),
        KeyCode::Tab | KeyCode::BackTab => "tab".to_owned(),
        KeyCode::Delete => "del".to_owned(),
        KeyCode::Insert => "ins".to_owned(),
        KeyCode::F(number) => format!("f{number}"),
        KeyCode::Char(' ') => "⎵".to_owned(),
        KeyCode::Char(chr) => chr.to_string(),
        KeyCode::Null => "null".to_owned(),
        KeyCode::Esc => "␛".to_owned(),
        KeyCode::CapsLock => "caps".to_owned(),
        KeyCode::ScrollLock => "scroll".to_owned(),
        KeyCode::NumLock => "num".to_owned(),
        KeyCode::PrintScreen => "prtsc".to_owned(),
        KeyCode::Pause => "pause".to_owned(),
        KeyCode::Menu => "menu".to_owned(),
        KeyCode::KeypadBegin => "begin".to_owned(),
        KeyCode::Media(code) => format!("{code:?}").to_ascii_lowercase(),
        KeyCode::Modifier(code) => format!("{code:?}").to_ascii_lowercase(),
    };
    let mut modifiers = String::new();
    if key.mods.contains(KeyModifiers::CONTROL) {
        modifiers.push('⌃');
    }
    if key.mods.contains(KeyModifiers::ALT) {
        modifiers.push('⌥');
    }
    if key.mods.contains(KeyModifiers::SHIFT) {
        modifiers.push('⇧');
    }
    if key
        .mods
        .intersects(KeyModifiers::SUPER | KeyModifiers::META)
    {
        modifiers.push('⌘');
    }
    if key.mods.contains(KeyModifiers::HYPER) {
        modifiers.push('◆');
    }

    KeyLabel {
        full: format!("{modifiers}{base}"),
        base,
        modified: !modifiers.is_empty(),
    }
}

pub fn build_entries(keymap: &HashMap<Key, Binding>) -> Vec<KeybindingEntry> {
    let mut entries: Vec<_> = keymap
        .iter()
        .map(|(&key, binding)| {
            // The panel is only on screen while it is open, and then `esc`
            // closes it instead of running its binding (see `App::handle_key`).
            let description = if key == CLOSE_KEY {
                "Close".to_owned()
            } else {
                binding_description(binding)
            };
            KeybindingEntry {
                key,
                label: format_key(key),
                description,
            }
        })
        .collect();
    entries.sort_by(compare_entries);
    entries
}

fn clean_first_line(text: &str) -> Option<String> {
    let line = text.split('\n').next().unwrap_or_default();
    let clean: String = line
        .chars()
        .map(|chr| if chr.is_control() { ' ' } else { chr })
        .collect();
    let clean = clean.trim();
    (!clean.is_empty()).then(|| clean.to_owned())
}

pub fn truncate_with_ellipsis(text: &str, max_width: usize) -> Cow<'_, str> {
    if UnicodeWidthStr::width(text) <= max_width {
        return Cow::Borrowed(text);
    }
    if max_width == 0 {
        return Cow::Borrowed("");
    }
    if max_width == 1 {
        return Cow::Borrowed("…");
    }

    let target = max_width - 1;
    let mut width = 0;
    let mut truncated = String::new();
    for chr in text.chars() {
        let chr_width = UnicodeWidthChar::width(chr).unwrap_or(0);
        if width + chr_width > target {
            break;
        }
        truncated.push(chr);
        width += chr_width;
    }
    truncated.push('…');
    Cow::Owned(truncated)
}

pub fn build_grid(entries: &[KeybindingEntry], available_width: usize) -> KeybindingGrid {
    if entries.is_empty() {
        return KeybindingGrid::default();
    }

    let max_columns = ((available_width + GRID_GAP) / (MIN_COLUMN_WIDTH + GRID_GAP)).max(1);
    let column_count = entries.len().min(max_columns);
    let gaps = GRID_GAP * (column_count - 1);
    let column_width = available_width.saturating_sub(gaps) / column_count;
    let key_width = entries
        .iter()
        .map(|entry| UnicodeWidthStr::width(entry.label.full.as_str()))
        .max()
        .unwrap_or(0)
        .min(MAX_KEY_WIDTH)
        .min(column_width);
    let row_count = entries.len().div_ceil(column_count);
    let mut rows = vec![Vec::new(); row_count];
    for index in 0..entries.len() {
        rows[index % row_count].push(index);
    }

    KeybindingGrid {
        rows,
        column_width,
        key_width,
    }
}

fn binding_description(binding: &Binding) -> String {
    if let Some(help) = binding.help.as_deref().and_then(clean_first_line) {
        return help;
    }
    match &binding.action {
        BindingAction::Cmd(command) => command.description().to_owned(),
        BindingAction::Sh(command) => {
            let command = clean_first_line(command).unwrap_or_default();
            format!(
                "`{}`",
                truncate_with_ellipsis(&command, SHELL_DESCRIPTION_WIDTH)
            )
        }
    }
}

fn compare_entries(a: &KeybindingEntry, b: &KeybindingEntry) -> Ordering {
    let a_alphanumeric = a.label.base.chars().all(char::is_alphanumeric);
    let b_alphanumeric = b.label.base.chars().all(char::is_alphanumeric);
    a_alphanumeric
        .cmp(&b_alphanumeric)
        .then_with(|| {
            a.label
                .base
                .to_lowercase()
                .cmp(&b.label.base.to_lowercase())
        })
        .then_with(|| a.label.modified.cmp(&b.label.modified))
        .then_with(|| uppercase_weight(a).cmp(&uppercase_weight(b)))
        .then_with(|| modifier_weight(a.key).cmp(&modifier_weight(b.key)))
        .then_with(|| a.label.full.cmp(&b.label.full))
        .then_with(|| a.description.cmp(&b.description))
}

fn uppercase_weight(entry: &KeybindingEntry) -> u8 {
    match entry.key.code {
        KeyCode::Char(chr) if chr.is_uppercase() => 1,
        _ => 0,
    }
}

fn modifier_weight(key: Key) -> u8 {
    u8::from(key.mods.contains(KeyModifiers::CONTROL))
        | (u8::from(key.mods.contains(KeyModifiers::ALT)) << 1)
        | (u8::from(key.mods.contains(KeyModifiers::SHIFT)) << 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::keys::Key;
    use ratatui::layout::Rect;

    #[test]
    fn key_labels_use_control_pictures_arrows_and_modifier_sigils() {
        assert_eq!(format_key(Key::parse("esc").unwrap()).full, "␛");
        assert_eq!(format_key(Key::parse("enter").unwrap()).full, "ret");
        assert_eq!(format_key(Key::parse("space").unwrap()).full, "⎵");
        assert_eq!(format_key(Key::parse("left").unwrap()).full, "←");
        assert_eq!(
            format_key(Key::parse("ctrl+alt+space").unwrap()).full,
            "⌃⌥⎵"
        );
        assert_eq!(format_key(Key::parse("shift+right").unwrap()).full, "⇧→");
        assert_eq!(format_key(Key::parse("shift+j").unwrap()).full, "J");
        assert_eq!(format_key(Key::parse("pageup").unwrap()).full, "pgup");
        assert_eq!(format_key(Key::parse("backspace").unwrap()).full, "bksp");
    }

    #[test]
    fn entries_sort_non_alphanumeric_first_then_by_displayed_base_key() {
        let config = Config::parse(
            r#"
[/]
cmd = "jump"

[?]
cmd = "quit"

[f]
cmd = "first"

[F]
cmd = "last"

[ctrl+f]
cmd = "page-down"
"#,
        )
        .unwrap();

        let entries = build_entries(&config.bindings);
        let labels: Vec<_> = entries
            .iter()
            .map(|entry| entry.label.full.as_str())
            .collect();

        assert_eq!(labels, ["/", "?", "f", "F", "⌃f"]);
    }

    #[test]
    fn entries_use_help_fallbacks_and_the_close_override() {
        let config = Config::parse(
            r#"
[esc]
cmd = "back"

[x]
sh = "attach-to-review\nignored"

[y]
cmd = "expand-recursively"
help = "  Custom\tCOPY\nignored"

[z]
sh = "printf z"
help = " 	  "
"#,
        )
        .unwrap();

        let entries = build_entries(&config.bindings);
        let description = |key: &str| {
            entries
                .iter()
                .find(|entry| entry.key == Key::parse(key).unwrap())
                .unwrap()
                .description
                .as_str()
        };

        assert_eq!(description("esc"), "Close");
        assert_eq!(description("x"), "`attach-to-r…`");
        // `help` is cleaned at display time: trimmed first line, controls
        // become spaces.
        assert_eq!(description("y"), "Custom COPY");
        // Blank `help` falls back to the action-derived description.
        assert_eq!(description("z"), "`printf z`");
    }

    #[test]
    fn text_cleanup_uses_trimmed_first_line_and_replaces_controls() {
        assert_eq!(
            clean_first_line("  Keep\tthis\u{7}\nignore this  "),
            Some("Keep this".to_owned())
        );
        assert_eq!(clean_first_line(" \t \nignored"), None);
    }

    #[test]
    fn truncation_counts_terminal_cells_and_includes_the_ellipsis() {
        assert_eq!(
            truncate_with_ellipsis("attach-to-review", 12),
            "attach-to-r…"
        );
        assert_eq!(truncate_with_ellipsis("界界界", 5), "界界…");
        assert_eq!(truncate_with_ellipsis("abc", 3), "abc");
        assert_eq!(truncate_with_ellipsis("abc", 1), "…");
        assert_eq!(truncate_with_ellipsis("abc", 0), "");
    }

    #[test]
    fn grid_fills_down_columns_before_moving_right() {
        let entries: Vec<_> = ["a", "b", "c", "d", "e"]
            .into_iter()
            .map(|name| {
                let key = Key::parse(name).unwrap();
                KeybindingEntry {
                    key,
                    label: format_key(key),
                    description: name.to_ascii_uppercase(),
                }
            })
            .collect();

        let grid = build_grid(&entries, 50);

        assert_eq!(grid.column_width, 24);
        assert_eq!(grid.rows, [vec![0, 3], vec![1, 4], vec![2]]);
    }

    #[test]
    fn panel_scroll_resets_when_reopened_and_clamps_to_layout() {
        let mut panel = KeybindingPanelState::default();
        panel.open();
        panel.record_layout(Rect::new(0, 10, 40, 6), 12, 4);
        panel.scroll_by(20);
        assert_eq!(panel.scroll(), 8);
        assert!(panel.contains(20, 12));
        assert!(!panel.contains(20, 9));

        panel.close();
        panel.open();

        assert_eq!(panel.scroll(), 0);
    }
}
