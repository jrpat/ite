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
    TreeColumnSet::new([ColumnDef::tree(
        "",
        ColumnWidth::flexible(1, u16::MAX).expect("valid width"),
    )])
    .expect("a single tree column is valid")
}

/// Style restricted to the default terminal palette: reversed-video focus,
/// dark-gray guide lines, no border.
fn style() -> TreeListViewStyle<'static> {
    TreeListViewStyle {
        highlight_style: Style::default().add_modifier(Modifier::REVERSED),
        line_style: Style::default().fg(Color::DarkGray),
        highlight_symbol: "",
        ..TreeListViewStyle::borderless()
    }
}

/// Render the tree into `area` and record the viewport height for paging.
pub fn draw(app: &mut App, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    app.page_height = area.height as usize;
    let columns = columns();
    let widget = TreeListView::new(&app.tree, &app.query, &Label, &columns, style());
    widget.render(area, buf, &mut app.state);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ExpandSpec;
    use crate::config::Config;
    use ratatui::buffer::Buffer;

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
}
