//! Rendering: a single-column tree list using only the terminal's ANSI palette.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, StatefulWidget, Widget};
use tui_treelistview::{
    ColumnDef, ColumnWidth, TreeColumnSet, TreeExpansionState, TreeGlyphs, TreeLabelPrefix,
    TreeLabelRenderer, TreeListView, TreeListViewStyle, TreeRowContext, tree_label_line,
};

use crate::app::{App, Mode};
use crate::jump::Jump;
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
        let node = model.node(id);
        let mut label = TreeLabelPrefix::borrowed(&node.name);
        if context.level == 0 && context.node.expansion == TreeExpansionState::Leaf {
            label.prefix = Some(glyphs.leaf.into());
        }
        let mut line = tree_label_line(context, label, glyphs);
        if let Some(detail) = &node.detail {
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
    unloaded: "◇",
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
/// known, and falls back to reverse video. Guide lines keep the normal text
/// color. No border, no header.
fn style(palette: Option<Palette>) -> TreeListViewStyle<'static> {
    let highlight_style = match palette {
        Some(palette) => Style::default().bg(palette.focus_bg()),
        None => Style::default().add_modifier(Modifier::REVERSED),
    };
    TreeListViewStyle {
        highlight_style,
        line_style: Style::default(),
        highlight_symbol: "",
        // Long names truncate at the viewport edge instead of paying for the
        // widget's off-screen virtual canvas.
        horizontal_scroll: tui_treelistview::TreeHorizontalScroll::Disabled,
        ..TreeListViewStyle::borderless()
    }
}

/// Render the current mode into `area`. In a modal picker the tree is hidden;
/// otherwise the tree list is drawn and the viewport height recorded for paging.
pub fn draw(app: &mut App, area: Rect, buf: &mut Buffer) {
    if let Mode::Jump(_) = app.mode {
        let palette = app.palette;
        let target = jump_area(area);
        if let Mode::Jump(jump) = &mut app.mode {
            render_jump(jump, target, buf, palette);
        }
        return;
    }
    app.page_height = area.height as usize;
    {
        let _span = crate::profile::span("ui::ensure_projection");
        app.state.ensure_projection(&app.tree, &app.query);
    }
    let _span = crate::profile::span("ui::widget_render");
    let columns = columns();
    let widget =
        TreeListView::new(&app.tree, &app.query, &Label, &columns, style(app.palette)).glyphs(GLYPHS);
    widget.render(area, buf, &mut app.state);
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

    let match_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let start = jump.scroll();
    let selected = jump.selected();
    let results = jump.results();
    let end = (start + rows).min(results.len());
    for (row, res) in results[start..end].iter().enumerate() {
        let y = area.y + 2 + row as u16;
        let line = Line::from(highlight_spans(jump.path(res.id), &res.indices, match_style));
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
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
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
    fn tree_guides_render_in_normal_text_color() {
        let (_d, mut app) = fixture_app();
        let (buf, text) = drawn(&mut app, 40, 10);
        // Row 1 is `├ • inner.txt`; its guide glyph must not be recolored.
        assert!(text.lines().nth(1).unwrap().starts_with('├'), "{text}");
        assert_eq!(buf[(0, 1)].fg, Color::Reset, "guides use the default fg");
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
            text.starts_with(r#"▶ project {4} name: "ite" · status: "experimental""#),
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
        let got: String = text.lines().take(6).map(|l| format!("{}\n", l.trim_end())).collect();
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
        assert!(lines[0].starts_with("/ inner"), "prompt row: {:?}", lines[0]);
        assert_eq!(buf[(0, 0)].symbol(), "/");
        assert_eq!(buf[(0, 0)].fg, Color::DarkGray);
        // A block cursor (reverse video) sits at the caret, just past the query.
        assert!(
            buf[(7, 0)].modifier.contains(Modifier::REVERSED),
            "expected a block cursor after `/ inner`"
        );
        // The counter shows one match out of the four candidate nodes.
        assert!(lines[0].trim_end().ends_with("1/4"), "counter row: {:?}", lines[0]);
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
