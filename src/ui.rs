//! Rendering: a single-column tree list using only the terminal's ANSI palette.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Cell, StatefulWidget};
use tui_treelistview::{
    ColumnDef, ColumnWidth, TreeColumnSet, TreeGlyphs, TreeLabelPrefix, TreeLabelRenderer,
    TreeListView, TreeListViewStyle, TreeRowContext, tree_name_cell,
};

use crate::app::App;
use crate::fstree::{FsTree, NodeId};

struct Label;

impl TreeLabelRenderer<FsTree> for Label {
    fn cell<'a>(
        &'a self,
        model: &'a FsTree,
        id: NodeId,
        context: &TreeRowContext<'_>,
        glyphs: &TreeGlyphs<'a>,
    ) -> Cell<'a> {
        let node = model.node(id);
        let cell = tree_name_cell(context, TreeLabelPrefix::borrowed(&node.name), glyphs);
        if node.is_dir {
            cell.style(
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            cell
        }
    }
}

fn columns() -> TreeColumnSet<'static, FsTree> {
    // Note: `flexible(min, ideal)` — the ideal must stay small. A huge ideal
    // makes the widget lay out a virtual canvas of that width and render the
    // whole thing every frame (a ~300ms/frame debug-build regression).
    TreeColumnSet::new([ColumnDef::tree(
        "",
        ColumnWidth::flexible(1, 40).expect("valid width"),
    )])
    .expect("a single tree column is valid")
}

/// Compact guides with no horizontal tails: `├ • cli.rs`.
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

/// Style restricted to the default terminal palette: reversed-video focus,
/// dark-gray guide lines, no border.
fn style() -> TreeListViewStyle<'static> {
    TreeListViewStyle {
        highlight_style: Style::default().add_modifier(Modifier::REVERSED),
        line_style: Style::default().fg(Color::DarkGray),
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
        TreeListView::new(&app.tree, &app.query, &Label, &columns, style()).glyphs(GLYPHS);
    widget.render(area, buf, &mut app.state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ExpandSpec;
    use crate::config::Config;
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
        let tree = FsTree::scan(dir.path(), false).unwrap();
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
    fn renders_expanded_tree_rows() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        std::fs::write(dir.path().join("subdir/inner.txt"), "").unwrap();
        std::fs::write(dir.path().join("file.txt"), "").unwrap();
        let tree = FsTree::scan(dir.path(), false).unwrap();
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
        let tree = FsTree::scan(dir.path(), false).unwrap();
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
