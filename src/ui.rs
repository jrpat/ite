//! Ratatui rendering for the tree and its modal/auxiliary surfaces. This module
//! turns `App`, `Jump`, and keybinding view-model state into buffer cells and
//! records viewport geometry needed by paging and mouse routing; it owns no
//! event loop or source data.
//!
//! Colors stay inside the terminal's ANSI palette except for the focus
//! background derived from the terminal's own colors (`Palette::focus_bg`). The
//! tree deliberately avoids horizontal scrolling and large flexible-column
//! ideals, which would make the widget render a costly off-screen canvas. The
//! custom vertical scrollbar repaints the widget's reserved gutter until the
//! upstream style API can express the desired quiet, dithered bar.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Scrollbar, ScrollbarState, StatefulWidget, Widget};
use tui_treelistview::{
    ColumnDef, ColumnWidth, TreeColumnSet, TreeExpansionState, TreeGlyphs, TreeLabelPrefix,
    TreeLabelRenderer, TreeListView, TreeListViewStyle, TreeRowContext, tree_label_line,
};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Mode};
use crate::jump::Jump;
use crate::keybindings::{
    GRID_GAP, KeybindingEntry, KeybindingGrid, MAX_PANEL_ROWS, build_sectioned_grid,
    truncate_with_ellipsis,
};
use crate::tree::{NodeId, Tree};

struct Label;

impl TreeLabelRenderer<Tree> for Label {
    fn cell<'a>(
        &'a self,
        model: &'a Tree,
        id: NodeId,
        context: &TreeRowContext<'_>,
        glyphs: &TreeGlyphs<'a>,
    ) -> Cell<'a> {
        let mut label = TreeLabelPrefix {
            name: model.name(id).into(),
            prefix: None,
        };
        if context.level == 0 && context.node.expansion == TreeExpansionState::Leaf {
            label.prefix = Some(glyphs.leaf.into());
        }
        let mut line = tree_label_line(context, label, glyphs);
        let state_glyph = match context.node.expansion {
            TreeExpansionState::Leaf => glyphs.leaf,
            TreeExpansionState::Collapsed => glyphs.collapsed,
            TreeExpansionState::Expanded | TreeExpansionState::ForcedByFilter => glyphs.expanded,
            TreeExpansionState::Unloaded => glyphs.unloaded,
            TreeExpansionState::Loading => glyphs.loading,
        };
        if let Some(state_index) = line
            .spans
            .iter()
            .take(line.spans.len().saturating_sub(1))
            .rposition(|span| span.content == state_glyph)
        {
            line.spans[state_index].style = context.line_style;
        }
        if let Some(detail) = model.detail(id) {
            line.push_span(Span::styled(
                format!(" {detail}"),
                Style::default().fg(Color::DarkGray),
            ));
        }
        Cell::from(line)
    }
}

fn columns() -> TreeColumnSet<'static, Tree> {
    // Note: `flexible(min, ideal)` — the ideal must stay small. A huge ideal
    // makes the widget lay out a virtual canvas of that width and render the
    // whole thing every frame (a ~300ms/frame debug-build regression).
    TreeColumnSet::new([ColumnDef::tree(
        "",
        ColumnWidth::flexible(1, 40).expect("valid width"),
    )])
    .expect("a single tree column is valid")
    .without_header()
}

/// Guides with no horizontal tails: `├ • file`, `├ ▼ dir`.
///
/// The widget draws each row as `<guides><space><state-glyph> <name>`, so the
/// disclosure triangle lands one column past the guides. To keep a child's stem
/// directly beneath its parent's triangle, the ancestor guides (`vert`,
/// `indent`, `empty`) are two columns wide while the branch stems (`branch`,
/// `branch_last`) stay one column — the widget's own separator supplies the
/// branch's second column. That extra column per ancestor is exactly what lines
/// `│`/`├`/`└` up under the triangle they hang from.
const GLYPHS: TreeGlyphs<'static> = TreeGlyphs {
    indent: "  ",
    branch_last: "└",
    branch: "├",
    vert: "│ ",
    empty: "  ",
    leaf: "•",
    expanded: "▼",
    collapsed: "▶",
    // Deliberately the collapsed glyph: unloaded-vs-loaded is bookkeeping,
    // not something the user should see. It also covers walked-empty
    // containers, which report `Unloaded` to stay branches.
    unloaded: "▶",
    loading: "◌",
};

/// The terminal's default foreground and background, queried at startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub fg: (u8, u8, u8),
    pub bg: (u8, u8, u8),
}

impl Palette {
    /// The focus-bar background: the default foreground blended over the
    /// default background at 10% opacity (terminals have no real
    /// translucency, so we premix the color).
    pub fn focus_bg(&self) -> Color {
        let blend = |bg: u8, fg: u8| ((u16::from(bg) * 9 + u16::from(fg) + 5) / 10) as u8;
        Color::Rgb(
            blend(self.bg.0, self.fg.0),
            blend(self.bg.1, self.fg.1),
            blend(self.bg.2, self.fg.2),
        )
    }
}

/// Focus uses a translucent-looking blend of the terminal's own colors when
/// known, and falls back to reverse video. Tree chrome uses ANSI foreground
/// color 8. No border, no header.
fn style(palette: Option<Palette>) -> TreeListViewStyle<'static> {
    TreeListViewStyle {
        highlight_style: focus_style(palette),
        line_style: Style::default().fg(Color::DarkGray),
        highlight_symbol: "",
        // Long names truncate at the viewport edge instead of paying for the
        // widget's off-screen virtual canvas.
        horizontal_scroll: tui_treelistview::TreeHorizontalScroll::Disabled,
        ..TreeListViewStyle::borderless()
    }
}

fn focus_style(palette: Option<Palette>) -> Style {
    match palette {
        Some(palette) => Style::default().bg(palette.focus_bg()),
        None => Style::default().add_modifier(Modifier::REVERSED),
    }
}

/// Repaint the vertical scrollbar over the one `TreeListView` just drew.
///
/// `TreeListViewStyle` exposes no scrollbar knobs, so the widget always paints
/// `Scrollbar::default()` — a `║` track and `█` thumb capped with `▲`/`▼`, all
/// at the terminal's full default foreground, shouting over the tree beside it.
/// It does reserve the gutter, though, so we paint our own bar into that column
/// afterwards rather than trying to restyle its glyphs in place.
///
/// The geometry is the widget's own: it reserves the gutter exactly when the
/// projection overflows the viewport (there is no header and no horizontal bar
/// to shorten it), and drives the bar from `offset` over `len - viewport + 1`
/// positions. Both are read back off the state it just clamped, so the two stay
/// in agreement. A patch making this configurable upstream is on the
/// `scrollbar-style` branch of the fork; when it lands, delete this and set
/// `vertical_scrollbar: Some(scrollbar())` in [`style`] instead.
fn render_scrollbar(app: &App, area: Rect, buf: &mut Buffer) {
    let total = app.state.projection().len();
    let viewport = area.height as usize;
    if area.width == 0 || total <= viewport {
        return;
    }
    let gutter = Rect {
        x: area.x + area.width - 1,
        width: 1,
        ..area
    };
    render_gutter_scrollbar(gutter, total - viewport, app.state.offset(), viewport, buf);
}

/// Paint the quiet bar into a one-cell gutter: `scrollable + 1` thumb
/// positions (one per reachable offset) over a `viewport`-row window.
fn render_gutter_scrollbar(
    gutter: Rect,
    scrollable: usize,
    position: usize,
    viewport: usize,
    buf: &mut Buffer,
) {
    let mut state = ScrollbarState::new(scrollable + 1)
        .position(position)
        .viewport_content_length(viewport);
    scrollbar().render(gutter, buf, &mut state);
}

/// The scrollbar is chrome, so it stays quiet: ANSI color 8 at half opacity,
/// and no arrow caps.
///
/// The opacity is dithered rather than blended: `▒` covers half of its cell and
/// `░` a quarter, so the terminal's own background shows through at a fixed
/// ratio. That keeps the bar inside the ANSI palette and makes it identical in
/// terminals that never answered the OSC 10/11 query.
fn scrollbar() -> Scrollbar<'static> {
    Scrollbar::default()
        .thumb_symbol("▒")
        .track_symbol(Some("░"))
        .begin_symbol(None)
        .end_symbol(None)
        .style(Style::default().fg(Color::DarkGray))
}

struct PanelLayout {
    area: Rect,
    /// Where entry text goes: inside the border, padding, and scrollbar chrome.
    content: Rect,
    grid: KeybindingGrid,
    overflow: bool,
}

fn keybinding_panel_layout(
    entries: &[KeybindingEntry],
    user_entry_count: usize,
    screen: Rect,
) -> PanelLayout {
    let width_without_chrome = |scrollbar: bool| {
        usize::from(screen.width)
            .saturating_sub(2) // the Block border
            .saturating_sub(2) // one cell of horizontal padding
            .saturating_sub(usize::from(scrollbar))
    };
    let max_body_rows = usize::from(screen.height / 2).min(MAX_PANEL_ROWS);
    let mut grid = build_sectioned_grid(entries, user_entry_count, width_without_chrome(false));
    let mut viewport_rows = grid.rows.len().min(max_body_rows);
    let mut overflow = grid.rows.len() > viewport_rows;
    if overflow {
        grid = build_sectioned_grid(entries, user_entry_count, width_without_chrome(true));
        viewport_rows = grid.rows.len().min(max_body_rows);
        overflow = grid.rows.len() > viewport_rows;
    }

    // `viewport_rows` is capped at MAX_PANEL_ROWS, so the height fits u16.
    let height = ((viewport_rows + 2) as u16).min(screen.height);
    let area = Rect::new(
        screen.x,
        screen.y + screen.height.saturating_sub(height),
        screen.width,
        height,
    );
    let content = Rect::new(
        area.x + 2, // the Block border plus one cell of horizontal padding
        area.y + 1,
        width_without_chrome(overflow) as u16,
        height.saturating_sub(2),
    );
    PanelLayout {
        area,
        content,
        grid,
        overflow,
    }
}

fn render_keybinding_panel(app: &mut App, layout: &PanelLayout, buf: &mut Buffer) {
    let entries = &app.panel_entries;
    let block_style = Style::default()
        .bg(Color::Reset)
        .remove_modifier(Modifier::REVERSED);
    let block = Block::bordered()
        .style(block_style)
        .border_style(Style::default().fg(Color::Reset));
    let inner = block.inner(layout.area);
    block.render(layout.area, buf);
    let viewport_rows = usize::from(layout.content.height);
    app.keybinding_panel
        .record_layout(layout.area, layout.grid.rows.len(), viewport_rows);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let content = layout.content;
    let key_style = Style::default()
        .fg(Color::Reset)
        .add_modifier(Modifier::BOLD);
    let description_style = Style::default().fg(Color::DarkGray);
    let separator_style = Style::default().fg(Color::DarkGray);
    let start = app.keybinding_panel.scroll();
    let end = (start + viewport_rows).min(layout.grid.rows.len());
    for (visible_row, row) in layout.grid.rows[start..end].iter().enumerate() {
        let y = content.y + visible_row as u16;
        if row.is_empty() {
            let separator_width = inner.width.saturating_sub(u16::from(layout.overflow));
            buf.set_stringn(
                inner.x,
                y,
                "─".repeat(usize::from(separator_width)),
                usize::from(separator_width),
                separator_style,
            );
            continue;
        }
        for (column, &entry_index) in row.iter().enumerate() {
            let entry = &entries[entry_index];
            // Grid widths derive from the u16 screen width, so they fit u16.
            let x = content.x + (column * (layout.grid.column_width + GRID_GAP)) as u16;
            let column_width = layout
                .grid
                .column_width
                .min(usize::from(content.right().saturating_sub(x)));
            if column_width == 0 {
                continue;
            }

            // Right-align the key label within the key column.
            let key_width = layout.grid.key_width.min(column_width);
            let label_width = UnicodeWidthStr::width(entry.label.full.as_str()).min(key_width);
            buf.set_stringn(
                x + (key_width - label_width) as u16,
                y,
                &entry.label.full,
                label_width,
                key_style,
            );

            let description_width = column_width.saturating_sub(key_width + 1);
            if description_width > 0 {
                buf.set_stringn(
                    x + (key_width + 1) as u16,
                    y,
                    truncate_with_ellipsis(&entry.description, description_width),
                    description_width,
                    description_style,
                );
            }
        }
    }

    if layout.overflow {
        let gutter = Rect::new(inner.right() - 1, inner.y, 1, inner.height);
        render_gutter_scrollbar(
            gutter,
            layout.grid.rows.len().saturating_sub(viewport_rows),
            app.keybinding_panel.scroll(),
            viewport_rows,
            buf,
        );
    }
}

/// Render the current mode into `area`. In a modal picker the tree is hidden;
/// otherwise the tree list is drawn and the viewport height recorded for paging.
pub fn draw(app: &mut App, area: Rect, buf: &mut Buffer) {
    // The recorded layout resets every frame and only the panel-painting path
    // below records a fresh one, so a mode that never draws the panel cannot
    // leave a stale rectangle capturing mouse events.
    app.keybinding_panel.clear_layout();
    if let Mode::Jump(_) = app.mode {
        let palette = app.palette;
        let target = jump_area(area);
        if let Mode::Jump(jump) = &mut app.mode {
            render_jump(jump, target, buf, palette);
        }
        return;
    }
    if let Mode::Indexing = app.mode {
        render_indexing(&app.tree, area, buf);
        return;
    }

    let panel = app
        .keybinding_panel
        .is_open()
        .then(|| keybinding_panel_layout(&app.panel_entries, app.panel_user_entry_count, area));
    let mut tree_area = match &panel {
        Some(layout) => Rect::new(
            area.x,
            area.y,
            area.width,
            layout.area.y.saturating_sub(area.y),
        ),
        None => area,
    };

    // Corrupt spans surface as a persistent banner; the tree shifts down
    // within the viewport left above the keybinding panel.
    if !app.tree.errors().is_empty() && tree_area.height > 0 {
        let banner = format!(
            "⚠ {} invalid record(s) — details on stderr at exit",
            app.tree.errors().len()
        );
        buf.set_stringn(
            tree_area.x,
            tree_area.y,
            &banner,
            tree_area.width as usize,
            Style::default().fg(Color::Red),
        );
        tree_area = Rect {
            y: tree_area.y + 1,
            height: tree_area.height - 1,
            ..tree_area
        };
    }
    app.page_height = tree_area.height as usize;
    {
        let _span = crate::profile::span("ui::ensure_projection");
        app.state.ensure_projection(&app.tree, &app.query);
    }
    if tree_area.width > 0 && tree_area.height > 0 {
        let _span = crate::profile::span("ui::widget_render");
        let columns = columns();
        let widget = TreeListView::new(&app.tree, &app.query, &Label, &columns, style(app.palette))
            .glyphs(GLYPHS);
        widget.render(tree_area, buf, &mut app.state);
        render_scrollbar(app, tree_area, buf);
    }
    if let Some(layout) = panel {
        render_keybinding_panel(app, &layout, buf);
    }
}

/// The blocking progress screen shown when `/` is pressed before the index
/// completes: one dim status line, esc to cancel back to the tree.
fn render_indexing(tree: &crate::tree::Tree, area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let status = format!(
        "indexing… {} nodes · {} pending · esc cancels",
        tree.len(),
        tree.pending()
    );
    buf.set_stringn(
        area.x,
        area.y,
        &status,
        area.width as usize,
        Style::default().fg(Color::DarkGray),
    );
}

/// The jump picker's placement. Today it is the whole screen; moving it to a
/// floating window or a split pane later changes only this function (plus any
/// border/clear chrome) — `render_jump` is agnostic to the `Rect` it receives.
/// See docs/adr/0002-surface-agnostic-jump-picker.md.
fn jump_area(screen: Rect) -> Rect {
    screen
}

/// Render the jump picker into `area`: a `/query` prompt with a right-aligned
/// `matched/total` counter, a divider line, then the ranked results with matched
/// characters highlighted and the selected row barred. Knows nothing about where
/// `area` is; full-screen is just the identity placement above.
pub fn render_jump(jump: &mut Jump, area: Rect, buf: &mut Buffer, palette: Option<Palette>) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let width = area.width;
    // Row 0 is the prompt, row 1 a divider, rows 2.. the results.
    let rows = area.height.saturating_sub(2) as usize;
    jump.set_viewport(rows);

    // Prompt: a dim `/ ` (matching the counter), then the editable query field,
    // then a right-aligned counter.
    let dim = Style::default().fg(Color::DarkGray);
    let (query_x, _) = buf.set_stringn(area.x, area.y, "/ ", width as usize, dim);
    let counter = format!("{}/{}", jump.matched(), jump.total());
    let counter_w = counter.chars().count() as u16;
    // The query field runs from `query_x` up to a gap before the counter.
    let right = area.x + width;
    let field_end = right.saturating_sub(counter_w + 1).max(query_x);
    let field_w = field_end - query_x;
    if field_w > 0 {
        let scroll = jump.visual_scroll(field_w as usize);
        Paragraph::new(jump.query())
            .scroll((0, scroll as u16))
            .render(Rect::new(query_x, area.y, field_w, 1), buf);
        // A block cursor (reverse video) at the caret — there is no real cursor
        // in the alt screen.
        let caret = query_x + (jump.visual_cursor().saturating_sub(scroll)) as u16;
        buf[(caret.min(field_end - 1), area.y)]
            .set_style(Style::default().add_modifier(Modifier::REVERSED));
    }
    if counter_w < width {
        buf.set_stringn(right - counter_w, area.y, &counter, counter_w as usize, dim);
    }

    // A single divider line under the input, separating it from the results.
    if area.height >= 2 {
        let divider = "─".repeat(width as usize);
        buf.set_stringn(
            area.x,
            area.y + 1,
            &divider,
            width as usize,
            Style::default().fg(Color::DarkGray),
        );
    }

    let match_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let start = jump.scroll();
    let selected = jump.selected();
    let results = jump.results();
    let end = (start + rows).min(results.len());
    for (row, res) in results[start..end].iter().enumerate() {
        let y = area.y + 2 + row as u16;
        let line = Line::from(highlight_spans(
            jump.path(res.id),
            &res.indices,
            match_style,
        ));
        buf.set_line(area.x, y, &line, width);
        if start + row == selected {
            highlight_row(buf, area.x, y, width, palette);
        }
    }
}

/// Bar the selected row: the terminal-derived focus blend when known, else
/// reverse video (mirrors the tree's focus styling and the palette-0–16 rule).
fn highlight_row(buf: &mut Buffer, x0: u16, y: u16, width: u16, palette: Option<Palette>) {
    for x in x0..x0 + width {
        let cell = &mut buf[(x, y)];
        match palette {
            Some(p) => {
                cell.set_bg(p.focus_bg());
            }
            None => {
                cell.set_style(Style::default().add_modifier(Modifier::REVERSED));
            }
        }
    }
}

/// Split `path` into spans, styling the matched character positions. `indices`
/// are char offsets, sorted and deduplicated by the picker.
fn highlight_spans(path: &str, indices: &[u32], match_style: Style) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_matched = false;
    for (i, chr) in path.chars().enumerate() {
        let matched = indices.binary_search(&(i as u32)).is_ok();
        if !run.is_empty() && matched != run_matched {
            spans.push(span(std::mem::take(&mut run), run_matched, match_style));
        }
        run.push(chr);
        run_matched = matched;
    }
    if !run.is_empty() {
        spans.push(span(run, run_matched, match_style));
    }
    spans
}

fn span(text: String, matched: bool, match_style: Style) -> Span<'static> {
    if matched {
        Span::styled(text, match_style)
    } else {
        Span::raw(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ExpandSpec;
    use crate::config::Config;
    use crate::fstree;
    use crate::tree::{ActionValues, Tree};
    use ratatui::buffer::Buffer;

    fn drawn(app: &mut App, width: u16, height: u16) -> (Buffer, String) {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        draw(app, area, &mut buf);
        let text: String = (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
            .collect();
        (buf, text)
    }

    fn fixture_app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        std::fs::write(dir.path().join("subdir/inner.txt"), "").unwrap();
        std::fs::write(dir.path().join("subdir/last.txt"), "").unwrap();
        std::fs::write(dir.path().join("file.txt"), "").unwrap();
        let tree = fstree::scan(dir.path(), false).unwrap();
        let app = App::new(tree, &Config::default(), Some(ExpandSpec::All));
        (dir, app)
    }

    #[test]
    fn tree_guides_have_no_horizontal_tails() {
        let (_d, mut app) = fixture_app();
        let (_buf, text) = drawn(&mut app, 40, 10);
        assert!(
            text.contains("├ • inner.txt"),
            "expected `├ • inner.txt` in:\n{text}"
        );
        assert!(
            text.contains("└ • last.txt"),
            "expected `└ • last.txt` in:\n{text}"
        );
        assert!(!text.contains('─'), "no horizontal tails in:\n{text}");
    }

    #[test]
    fn node_type_glyphs_follow_parent_stems_with_one_space() {
        let mut tree = Tree::new();
        let root = tree.push(None, "root", true, ActionValues::new("", "", ""));
        let open = tree.push(Some(root), "open", true, ActionValues::new("", "", ""));
        tree.push(Some(open), "nested", false, ActionValues::new("", "", ""));
        let closed = tree.push(Some(root), "closed", true, ActionValues::new("", "", ""));
        tree.push(Some(closed), "hidden", false, ActionValues::new("", "", ""));
        tree.push(Some(root), "leaf", false, ActionValues::new("", "", ""));
        let mut app = App::new(tree, &Config::default(), Some(ExpandSpec::All));
        app.state.set_expanded(closed, Some(root), false);

        let (_buf, text) = drawn(&mut app, 40, 10);
        let got: Vec<_> = text.lines().take(5).map(str::trim_end).collect();
        assert_eq!(
            got,
            [
                "▼ root",
                "├ ▼ open",
                "│ └ • nested",
                "├ ▶ closed",
                "└ • leaf",
            ]
        );
    }

    #[test]
    fn top_level_leaves_use_the_leaf_glyph() {
        let (_d, mut app) = fixture_app();
        let (_buf, text) = drawn(&mut app, 40, 10);
        let got: Vec<_> = text.lines().take(4).map(str::trim_end).collect();

        assert_eq!(
            got,
            ["▼ subdir", "├ • inner.txt", "└ • last.txt", "• file.txt"]
        );
    }

    #[test]
    fn unwalked_directories_render_the_collapsed_glyph() {
        let (_d, mut app) = {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir(dir.path().join("subdir")).unwrap();
            std::fs::write(dir.path().join("subdir/inner.txt"), "").unwrap();
            std::fs::write(dir.path().join("file.txt"), "").unwrap();
            let tree = fstree::scan(dir.path(), false).unwrap();
            (dir, App::new(tree, &Config::default(), None))
        };
        assert!(app.has_work(), "subdir must still be unwalked");
        let (_buf, text) = drawn(&mut app, 40, 10);
        assert!(text.contains("▶ subdir"), "in:\n{text}");
        assert!(!text.contains('◇'), "no unloaded glyph in:\n{text}");
    }

    #[test]
    fn empty_directories_render_as_collapsed_branches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("hollow")).unwrap();
        std::fs::write(dir.path().join("file.txt"), "").unwrap();
        let tree = fstree::scan(dir.path(), false).unwrap();
        let mut app = App::new(tree, &Config::default(), Some(ExpandSpec::All));
        while app.do_work() {}

        let (_buf, text) = drawn(&mut app, 40, 10);
        assert!(text.contains("▶ hollow"), "in:\n{text}");
        assert!(!text.contains("• hollow"), "not a leaf in:\n{text}");
    }

    #[test]
    fn focus_bg_blends_foreground_at_ten_percent() {
        let white_on_black = Palette {
            fg: (255, 255, 255),
            bg: (0, 0, 0),
        };
        assert_eq!(white_on_black.focus_bg(), Color::Rgb(26, 26, 26));
        let mixed = Palette {
            fg: (0, 0, 0),
            bg: (200, 100, 50),
        };
        assert_eq!(mixed.focus_bg(), Color::Rgb(180, 90, 45));
    }

    #[test]
    fn focused_row_uses_blended_bg_when_palette_known() {
        let (_d, mut app) = fixture_app();
        app.palette = Some(Palette {
            fg: (255, 255, 255),
            bg: (0, 0, 0),
        });
        let (buf, text) = drawn(&mut app, 40, 10);
        // Focus starts on the first row ("subdir").
        assert!(text.starts_with("▼ subdir"), "{text}");
        let cell = &buf[(0, 0)];
        assert_eq!(cell.bg, Color::Rgb(26, 26, 26), "focused bg is the blend");
        assert!(
            !cell.modifier.contains(Modifier::REVERSED),
            "no reverse video when the palette is known"
        );
    }

    #[test]
    fn focused_row_falls_back_to_reverse_video_without_palette() {
        let (_d, mut app) = fixture_app();
        assert_eq!(app.palette, None);
        let (buf, text) = drawn(&mut app, 40, 10);
        assert!(text.starts_with("▼ subdir"), "{text}");
        assert!(
            buf[(0, 0)].modifier.contains(Modifier::REVERSED),
            "reverse video fallback"
        );
    }

    #[test]
    fn tree_chrome_uses_ansi_color_8() {
        let mut tree = Tree::new();
        let outer = tree.push(None, "outer", true, ActionValues::new("", "", ""));
        let inner = tree.push(Some(outer), "inner", true, ActionValues::new("", "", ""));
        tree.push(Some(inner), "first", false, ActionValues::new("", "", ""));
        tree.push(Some(inner), "last", false, ActionValues::new("", "", ""));
        tree.push(Some(outer), "sibling", false, ActionValues::new("", "", ""));
        let closed = tree.push(None, "closed", true, ActionValues::new("", "", ""));
        tree.push(Some(closed), "hidden", false, ActionValues::new("", "", ""));
        tree.push(None, "root-leaf", false, ActionValues::new("", "", ""));
        let mut app = App::new(tree, &Config::default(), Some(ExpandSpec::All));
        app.state.set_expanded(closed, None, false);

        let (buf, text) = drawn(&mut app, 40, 10);
        for (x, y, symbol) in [
            (0, 0, "▼"),
            (0, 1, "├"),
            (2, 1, "▼"),
            (0, 2, "│"),
            (2, 2, "├"),
            (4, 2, "•"),
            (0, 3, "│"),
            (2, 3, "└"),
            (4, 3, "•"),
            (0, 4, "└"),
            (2, 4, "•"),
            (0, 5, "▶"),
            (0, 6, "•"),
        ] {
            let cell = &buf[(x, y)];
            assert_eq!(
                cell.symbol(),
                symbol,
                "unexpected tree at ({x}, {y}):\n{text}"
            );
            assert_eq!(
                cell.fg,
                Color::DarkGray,
                "tree glyph at ({x}, {y}) should use ANSI foreground color 8"
            );
        }
    }

    #[test]
    fn node_detail_uses_ansi_color_8_while_primary_text_stays_normal() {
        let mut tree = Tree::new();
        let root = tree.push_with_detail(
            None,
            "project {4}",
            Some(r#"name: "ite" · status: "experimental""#.to_owned()),
            true,
            ActionValues::new("", "", ""),
        );
        tree.push(
            Some(root),
            r#"name: "ite""#,
            false,
            ActionValues::new("", "", ""),
        );
        let mut app = App::new(tree, &Config::default(), None);

        let (buf, text) = drawn(&mut app, 60, 1);

        assert!(
            // Single-rooted trees open expanded, so the root shows ▼.
            text.starts_with(r#"▼ project {4} name: "ite" · status: "experimental""#),
            "{text}"
        );
        let primary = &buf[(2, 0)];
        assert_eq!(primary.fg, Color::Reset);
        assert!(!primary.modifier.contains(Modifier::BOLD));

        let detail = &buf[(14, 0)];
        assert_eq!(detail.symbol(), "n");
        assert_eq!(detail.fg, Color::DarkGray);
        assert!(!detail.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn renders_expanded_tree_rows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        std::fs::write(dir.path().join("subdir/inner.txt"), "").unwrap();
        std::fs::write(dir.path().join("file.txt"), "").unwrap();
        let tree = fstree::scan(dir.path(), false).unwrap();
        let mut app = App::new(tree, &Config::default(), Some(ExpandSpec::All));

        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        draw(&mut app, area, &mut buf);

        let text: String = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        assert!(text.contains("subdir"), "missing subdir in:\n{text}");
        assert!(text.contains("inner.txt"), "missing inner.txt in:\n{text}");
        assert!(text.contains("file.txt"), "missing file.txt in:\n{text}");
        assert_eq!(app.page_height, 10);
    }

    /// A child's stem (`├`/`└`/`│`) must sit in the same column as its
    /// parent container's triangle. With single-column guides the widget's
    /// disclosure glyph lands one column past the guides, so deeper stems
    /// used to drift left of the triangle they hang from.
    #[test]
    fn stems_align_with_parent_triangle() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("outer/inner")).unwrap();
        std::fs::write(dir.path().join("outer/inner/deep.txt"), "").unwrap();
        std::fs::write(dir.path().join("outer/inner/deep2.txt"), "").unwrap();
        std::fs::write(dir.path().join("outer/sibling.txt"), "").unwrap();
        std::fs::write(dir.path().join("zroot.txt"), "").unwrap();
        let tree = fstree::scan(dir.path(), false).unwrap();
        let mut app = App::new(tree, &Config::default(), Some(ExpandSpec::All));
        let (_buf, text) = drawn(&mut app, 40, 12);
        let got: String = text
            .lines()
            .take(6)
            .map(|l| format!("{}\n", l.trim_end()))
            .collect();
        let want = "\
▼ outer
├ ▼ inner
│ ├ • deep.txt
│ └ • deep2.txt
└ • sibling.txt
• zroot.txt
";
        assert_eq!(got, want, "\ngot:\n{got}\nwant:\n{want}");
    }

    /// The scrollbar is chrome, so it follows the same rules as the tree
    /// guides: ANSI color 8, dithered to a half-tone, and no `▲`/`▼` caps —
    /// rather than ratatui's default `║`/`█`/`▲`/`▼` at full brightness.
    #[test]
    fn scrollbar_is_dim_and_uncapped() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..30 {
            std::fs::write(dir.path().join(format!("file-{i:02}.txt")), "").unwrap();
        }
        let tree = fstree::scan(dir.path(), false).unwrap();
        let mut app = App::new(tree, &Config::default(), Some(ExpandSpec::All));

        // 30 rows in a 10-row viewport, so the bar is drawn in the last column.
        let (buf, text) = drawn(&mut app, 40, 10);
        let column: Vec<&str> = (0..10).map(|y| buf[(39, y)].symbol()).collect();
        assert!(
            column.iter().all(|s| *s == "░" || *s == "▒"),
            "unexpected scrollbar column {column:?} in:\n{text}"
        );
        assert!(column.contains(&"▒"), "no thumb drawn: {column:?}");
        assert!(column.contains(&"░"), "no track drawn: {column:?}");
        for y in 0..10 {
            assert_eq!(
                buf[(39, y)].fg,
                Color::DarkGray,
                "scrollbar row {y} should use ANSI foreground color 8"
            );
        }
    }

    /// We paint the bar into the gutter `TreeListView` reserves, deriving its
    /// geometry from the same state the widget does. If those ever drift — a
    /// header appearing, the reservation rule changing — the thumb stops
    /// tracking the viewport, so pin it at both ends of the scroll range.
    #[test]
    fn scrollbar_thumb_tracks_the_viewport() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..30 {
            std::fs::write(dir.path().join(format!("file-{i:02}.txt")), "").unwrap();
        }
        let tree = fstree::scan(dir.path(), false).unwrap();
        let mut app = App::new(tree, &Config::default(), Some(ExpandSpec::All));

        let (top, _) = drawn(&mut app, 40, 10);
        assert_eq!(top[(39, 0)].symbol(), "▒", "thumb should start at the top");
        assert_eq!(top[(39, 9)].symbol(), "░", "track should fill the bottom");

        app.state.set_offset(usize::MAX);
        let (bottom, _) = drawn(&mut app, 40, 10);
        assert_eq!(
            bottom[(39, 9)].symbol(),
            "▒",
            "thumb should reach the bottom at the last viewport"
        );
        assert_eq!(bottom[(39, 0)].symbol(), "░", "track should fill the top");
    }

    /// The bar appears only when the tree actually overflows its viewport.
    #[test]
    fn no_scrollbar_when_everything_fits() {
        let (_d, mut app) = fixture_app();
        let (buf, text) = drawn(&mut app, 40, 10);
        for y in 0..10 {
            let symbol = buf[(39, y)].symbol();
            assert!(
                symbol == " " || symbol.is_empty(),
                "unexpected scrollbar cell {symbol:?} at row {y} in:\n{text}"
            );
        }
    }

    /// Guards against the virtual-canvas regression: a mis-sized column made
    /// the widget allocate and render a 65k-cell-wide buffer per frame
    /// (~10ms). 100 draws must stay far under that regime's ~1s.
    #[test]
    fn repeated_draws_are_fast() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..30 {
            std::fs::write(dir.path().join(format!("file-{i:02}.txt")), "").unwrap();
        }
        let tree = fstree::scan(dir.path(), false).unwrap();
        let mut app = App::new(tree, &Config::default(), Some(ExpandSpec::All));
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        draw(&mut app, area, &mut buf); // warm-up
        let start = std::time::Instant::now();
        for _ in 0..100 {
            draw(&mut app, area, &mut buf);
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "100 draws took {elapsed:?}"
        );
    }

    #[test]
    fn keybinding_panel_is_bottom_docked_styled_and_reduces_the_tree_viewport() {
        use crate::keys::Key;
        let (_d, mut app) = fixture_app();
        app.palette = Some(Palette {
            fg: (255, 255, 255),
            bg: (0, 0, 0),
        });
        app.handle_key(Key::parse("?").unwrap());

        let (buf, text) = drawn(&mut app, 80, 24);
        let panel = app.keybinding_panel.area().expect("panel area");

        assert_eq!(panel.y + panel.height, 24);
        assert_eq!(app.page_height, panel.y as usize);
        assert_eq!(buf[(panel.x, panel.y)].symbol(), "┌");
        assert_eq!(buf[(panel.x, panel.y)].fg, Color::Reset);
        assert_eq!(buf[(panel.x + 1, panel.y + 1)].bg, Color::Reset);
        assert!(text.contains("Shortcuts"), "{text}");
        assert!(text.contains("Close"), "{text}");
        assert!(text.contains("First"), "{text}");
        assert!(
            app.panel_entries
                .iter()
                .all(|entry| entry.label.full != "gg"),
            "{text}"
        );

        let layout = keybinding_panel_layout(
            &app.panel_entries,
            app.panel_user_entry_count,
            Rect::new(0, 0, 80, 24),
        );
        let entry = &app.panel_entries[layout.grid.rows[0][0]];
        let key_width = layout.grid.key_width;
        let label_width = UnicodeWidthStr::width(entry.label.full.as_str());
        let key = &buf[(
            layout.content.x + (key_width - label_width) as u16,
            layout.content.y,
        )];
        assert_eq!(
            key.fg,
            Color::Reset,
            "key should use the default foreground"
        );
        assert!(key.modifier.contains(Modifier::BOLD), "key should be bold");

        let description = &buf[(layout.content.x + key_width as u16 + 1, layout.content.y)];
        assert_eq!(
            description.fg,
            Color::DarkGray,
            "description should use ANSI 8"
        );
        assert!(
            !description.modifier.contains(Modifier::BOLD),
            "description should not be bold"
        );
    }

    #[test]
    fn user_keybindings_are_above_builtin_bindings_with_a_full_width_gray_rule() {
        use crate::keys::Key;

        let mut tree = Tree::new();
        tree.push(None, "leaf", false, ActionValues::new("", "", ""));
        let config = Config::parse(
            r#"
[j]
cmd = "quit"
help = "Custom quit"

[x]
sh = "printf custom"
help = "Custom action"
"#,
        )
        .unwrap();
        let mut app = App::new(tree, &config, None);
        let configured_j: Vec<_> = app
            .panel_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.key == Key::parse("j").unwrap())
            .collect();
        assert_eq!(
            configured_j.len(),
            1,
            "an override should not also be built in"
        );
        assert!(configured_j[0].0 < app.panel_user_entry_count);
        app.palette = Some(Palette {
            fg: (255, 255, 255),
            bg: (0, 0, 0),
        });
        app.handle_key(Key::parse("?").unwrap());

        let (buf, text) = drawn(&mut app, 80, 30);
        let panel = app.keybinding_panel.area().expect("panel area");
        let separator_y = (panel.y + 1..panel.bottom() - 1)
            .find(|&y| {
                (panel.x + 1..panel.right() - 1).all(|x| {
                    let cell = &buf[(x, y)];
                    cell.symbol() == "─" && cell.fg == Color::DarkGray
                })
            })
            .unwrap_or_else(|| panic!("missing full-width ANSI-8 separator:\n{text}"));
        let row_containing = |needle: &str| {
            text.lines()
                .position(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("missing {needle:?} in:\n{text}")) as u16
        };

        assert!(row_containing("Custom quit") < separator_y, "{text}");
        assert!(row_containing("Custom action") < separator_y, "{text}");
        assert!(row_containing("Shortcuts") > separator_y, "{text}");
    }

    #[test]
    fn overflowing_keybinding_panel_reuses_the_app_scrollbar_style() {
        use crate::keys::Key;
        let (_d, mut app) = fixture_app();
        app.handle_key(Key::parse("?").unwrap());

        let (buf, text) = drawn(&mut app, 30, 12);
        let panel = app.keybinding_panel.area().expect("panel area");
        assert_eq!(panel.height, 8, "six body rows plus the border");

        let x = panel.x + panel.width - 2;
        let symbols: Vec<_> = (panel.y + 1..panel.y + panel.height - 1)
            .map(|y| buf[(x, y)].symbol())
            .collect();
        assert!(
            symbols
                .iter()
                .all(|symbol| *symbol == "▒" || *symbol == "░"),
            "unexpected scrollbar {symbols:?}:\n{text}"
        );
        assert!(symbols.contains(&"▒"));
        assert!(symbols.contains(&"░"));
        assert_eq!(buf[(x, panel.y + 1)].fg, Color::DarkGray);
    }

    #[test]
    fn keybinding_panel_uses_default_background_when_the_palette_is_unknown() {
        use crate::keys::Key;
        let (_d, mut app) = fixture_app();
        app.handle_key(Key::parse("?").unwrap());

        let (buf, _text) = drawn(&mut app, 80, 24);
        let panel = app.keybinding_panel.area().expect("panel area");

        let body = &buf[(panel.x + 1, panel.y + 1)];
        assert_eq!(body.bg, Color::Reset);
        assert!(!body.modifier.contains(Modifier::REVERSED));
        let border = &buf[(panel.x, panel.y)];
        assert_eq!(border.fg, Color::Reset);
        assert!(
            !border.modifier.contains(Modifier::REVERSED),
            "the fallback must not reverse the default-foreground border"
        );
    }

    #[test]
    fn jump_picker_renders_prompt_results_and_highlights() {
        use crate::keys::Key;
        let (_d, mut app) = fixture_app();
        app.handle_key(Key::parse("/").unwrap());
        for k in ["i", "n", "n", "e", "r"] {
            app.handle_key(Key::parse(k).unwrap());
        }
        let (buf, text) = drawn(&mut app, 40, 10);
        let lines: Vec<&str> = text.lines().collect();
        // A dim `/ ` prefix (same color as the counter), then the query.
        assert!(
            lines[0].starts_with("/ inner"),
            "prompt row: {:?}",
            lines[0]
        );
        assert_eq!(buf[(0, 0)].symbol(), "/");
        assert_eq!(buf[(0, 0)].fg, Color::DarkGray);
        // A block cursor (reverse video) sits at the caret, just past the query.
        assert!(
            buf[(7, 0)].modifier.contains(Modifier::REVERSED),
            "expected a block cursor after `/ inner`"
        );
        // The counter shows one match out of the four candidate nodes.
        assert!(
            lines[0].trim_end().ends_with("1/4"),
            "counter row: {:?}",
            lines[0]
        );
        // Row 1 is a full-width divider under the input.
        assert_eq!(lines[1], "─".repeat(40), "divider row: {:?}", lines[1]);
        // Results begin on row 2.
        assert!(lines[2].contains("inner.txt"), "result row: {:?}", lines[2]);
        // At least one matched character in the results renders cyan + bold.
        let highlighted = (0..40).any(|x| {
            (2..10).any(|y| {
                let cell = &buf[(x, y)];
                cell.fg == Color::Cyan && cell.modifier.contains(Modifier::BOLD)
            })
        });
        assert!(highlighted, "expected a highlighted match cell:\n{text}");
    }
}
