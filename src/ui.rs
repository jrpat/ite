//! Rendering: a single-column tree list using only the terminal's ANSI palette.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Cell, StatefulWidget};
use tui_treelistview::{
    ColumnDef, ColumnWidth, TreeColumnSet, TreeGlyphs, TreeLabelPrefix, TreeLabelRenderer,
    TreeListView, TreeListViewStyle, TreeRowContext, tree_label_line,
};

use crate::app::App;
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
        let mut line = tree_label_line(context, TreeLabelPrefix::borrowed(&node.name), glyphs);
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

/// One-column guides with no horizontal tails: `├ • file`, `├ ▼ dir`.
const GLYPHS: TreeGlyphs<'static> = TreeGlyphs {
    indent: " ",
    branch_last: "└",
    branch: "├",
    vert: "│",
    empty: " ",
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

/// Render the tree into `area` and record the viewport height for paging.
pub fn draw(app: &mut App, area: Rect, buf: &mut ratatui::buffer::Buffer) {
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
}
