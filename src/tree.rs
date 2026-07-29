//! Source-neutral tree data consumed by the application and renderer.
//!
//! Consumers read nodes only through the accessor methods on [`Tree`]; the
//! stored representation is private so it can be swapped for spans into
//! retained input without touching the app, renderer, or picker. Accessors
//! return owned values for the same reason: a future tree computes them from
//! the source on demand instead of holding them.

use std::ffi::OsString;

use tui_treelistview::{TreeChildren, TreeModel, TreeRevision};

pub type NodeId = usize;

/// Values used when the focused node is accepted or passed to a shell binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionValues {
    /// Text written to stdout when the node is accepted.
    pub output: OsString,
    /// Text written to stdout by the alternate accept action.
    pub alternate_output: OsString,
    /// Value exported to shell bindings as `$path`.
    pub path: OsString,
    /// Value exported to shell bindings as `$relpath`.
    pub relpath: OsString,
}

impl ActionValues {
    pub fn new(
        output: impl Into<OsString>,
        path: impl Into<OsString>,
        relpath: impl Into<OsString>,
    ) -> Self {
        let output = output.into();
        Self {
            alternate_output: output.clone(),
            output,
            path: path.into(),
            relpath: relpath.into(),
        }
    }

    pub fn with_alternate_output(mut self, output: impl Into<OsString>) -> Self {
        self.alternate_output = output.into();
        self
    }
}

#[derive(Debug)]
struct Node {
    name: String,
    detail: Option<String>,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    is_container: bool,
    depth: usize,
    action: ActionValues,
}

#[derive(Debug, Default)]
pub struct Tree {
    nodes: Vec<Node>,
    roots: Vec<NodeId>,
    view_root: Option<NodeId>,
    revision: TreeRevision,
}

impl Tree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(
        &mut self,
        parent: Option<NodeId>,
        name: impl Into<String>,
        is_container: bool,
        action: ActionValues,
    ) -> NodeId {
        self.push_with_detail(parent, name, None, is_container, action)
    }

    pub fn push_with_detail(
        &mut self,
        parent: Option<NodeId>,
        name: impl Into<String>,
        detail: Option<String>,
        is_container: bool,
        action: ActionValues,
    ) -> NodeId {
        let id = self.nodes.len();
        let depth = parent.map_or(0, |id| self.nodes[id].depth + 1);
        self.nodes.push(Node {
            name: name.into(),
            detail,
            parent,
            children: Vec::new(),
            is_container,
            depth,
            action,
        });
        match parent {
            Some(parent) => self.nodes[parent].children.push(id),
            None => self.roots.push(id),
        }
        id
    }

    fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
    }

    /// The node's display name.
    pub fn name(&self, id: NodeId) -> String {
        self.node(id).name.clone()
    }

    /// Optional secondary text rendered after the name.
    pub fn detail(&self, id: NodeId) -> Option<String> {
        self.node(id).detail.clone()
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).parent
    }

    /// 0 for roots.
    pub fn depth(&self, id: NodeId) -> usize {
        self.node(id).depth
    }

    /// Whether the node represents a container, including an empty one.
    pub fn is_container(&self, id: NodeId) -> bool {
        self.node(id).is_container
    }

    /// The node's children, in display order.
    pub fn children_of(&self, id: NodeId) -> &[NodeId] {
        &self.node(id).children
    }

    /// Text written to stdout when the node is accepted.
    pub fn output(&self, id: NodeId) -> OsString {
        self.node(id).action.output.clone()
    }

    /// Text written to stdout by the alternate accept action.
    pub fn alternate_output(&self, id: NodeId) -> OsString {
        self.node(id).action.alternate_output.clone()
    }

    /// Value exported to shell bindings as `$path`.
    pub fn path(&self, id: NodeId) -> OsString {
        self.node(id).action.path.clone()
    }

    /// Value exported to shell bindings as `$relpath`.
    pub fn relpath(&self, id: NodeId) -> OsString {
        self.node(id).action.relpath.clone()
    }

    /// The text the jump picker matches and displays: the node's
    /// document-global address. Coincides with `relpath` for directory scans
    /// and JSON documents; JSONL diverges, since a record-relative `relpath`
    /// repeats across records.
    pub fn jump_key(&self, id: NodeId) -> String {
        self.node(id).action.relpath.to_string_lossy().into_owned()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn root_ids(&self) -> &[NodeId] {
        &self.roots
    }

    /// The temporary root used by the tree view, if it has been narrowed.
    pub(crate) fn view_root(&self) -> Option<NodeId> {
        self.view_root
    }

    /// Narrow the tree model to one subtree, or restore the original forest.
    pub(crate) fn set_view_root(&mut self, root: Option<NodeId>) {
        if self.view_root != root {
            self.view_root = root;
            self.revision.advance();
        }
    }

    /// The node's parent in the current view. A temporary root has no parent.
    pub(crate) fn view_parent(&self, id: NodeId) -> Option<NodeId> {
        if self.view_root == Some(id) {
            None
        } else {
            self.nodes[id].parent
        }
    }

    /// Whether a node belongs to the subtree exposed by the current view.
    pub(crate) fn is_in_view(&self, id: NodeId) -> bool {
        let Some(root) = self.view_root else {
            return true;
        };
        let mut cursor = Some(id);
        while let Some(current) = cursor {
            if current == root {
                return true;
            }
            cursor = self.nodes[current].parent;
        }
        false
    }

    /// True when the node cannot be expanded.
    pub fn is_leaf(&self, id: NodeId) -> bool {
        self.node(id).children.is_empty()
    }

    /// All expandable nodes as `(id, parent)` pairs, in tree order.
    pub fn branches(&self) -> impl Iterator<Item = (NodeId, Option<NodeId>)> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(id, _)| !self.is_leaf(*id))
            .map(|(id, node)| (id, node.parent))
    }

    /// Reorder every sibling list (roots included) to put containers first,
    /// keeping the existing relative order within each group.
    pub(crate) fn containers_first(&mut self) {
        let containers_first = |nodes: &[Node], ids: &mut Vec<NodeId>| {
            ids.sort_by_key(|&id| !nodes[id].is_container);
        };
        let mut roots = std::mem::take(&mut self.roots);
        containers_first(&self.nodes, &mut roots);
        self.roots = roots;
        for id in 0..self.nodes.len() {
            let mut children = std::mem::take(&mut self.nodes[id].children);
            containers_first(&self.nodes, &mut children);
            self.nodes[id].children = children;
        }
    }
}

impl TreeModel for Tree {
    type Id = NodeId;

    fn roots(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.roots
            .iter()
            .copied()
            .filter(|_| self.view_root.is_none())
            .chain(self.view_root)
    }

    fn children(&self, id: NodeId) -> TreeChildren<'_, NodeId> {
        TreeChildren::loaded(&self.nodes[id].children)
    }

    fn revision(&self) -> TreeRevision {
        self.revision
    }

    fn size_hint(&self) -> usize {
        self.nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn sample() -> (Tree, NodeId, NodeId, NodeId) {
        let mut tree = Tree::new();
        let dir = tree.push(None, "dir", true, ActionValues::new("dir", "/abs/dir", "dir"));
        let file = tree.push_with_detail(
            Some(dir),
            "file",
            Some("hint".to_owned()),
            false,
            ActionValues::new("dir/file", "/abs/dir/file", "dir/file")
                .with_alternate_output("file"),
        );
        let leaf = tree.push(None, "leaf", false, ActionValues::new("leaf", "/abs/leaf", "leaf"));
        (tree, dir, file, leaf)
    }

    #[test]
    fn accessors_expose_hierarchy_and_display_data() {
        let (tree, dir, file, leaf) = sample();
        assert_eq!(tree.name(dir), "dir");
        assert_eq!(tree.detail(dir), None);
        assert_eq!(tree.detail(file).as_deref(), Some("hint"));
        assert_eq!(tree.parent(dir), None);
        assert_eq!(tree.parent(file), Some(dir));
        assert_eq!(tree.depth(dir), 0);
        assert_eq!(tree.depth(file), 1);
        assert!(tree.is_container(dir));
        assert!(!tree.is_container(leaf));
        assert_eq!(tree.children_of(dir), [file]);
        assert!(tree.children_of(leaf).is_empty());
    }

    #[test]
    fn accessors_expose_action_values() {
        let (tree, _, file, _) = sample();
        assert_eq!(tree.output(file), OsStr::new("dir/file"));
        assert_eq!(tree.alternate_output(file), OsStr::new("file"));
        assert_eq!(tree.path(file), OsStr::new("/abs/dir/file"));
        assert_eq!(tree.relpath(file), OsStr::new("dir/file"));
    }

    #[test]
    fn jump_key_is_the_relpath_text_today() {
        let (tree, dir, file, _) = sample();
        assert_eq!(tree.jump_key(dir), "dir");
        assert_eq!(tree.jump_key(file), "dir/file");
    }

    #[test]
    fn containers_first_reorders_every_sibling_list_stably() {
        let mut tree = Tree::new();
        let a = tree.push(None, "a-file", false, ActionValues::new("", "", ""));
        let b = tree.push(None, "b-dir", true, ActionValues::new("", "", ""));
        let c = tree.push(None, "c-file", false, ActionValues::new("", "", ""));
        let d = tree.push(None, "d-dir", true, ActionValues::new("", "", ""));
        let inner_file = tree.push(Some(b), "x-file", false, ActionValues::new("", "", ""));
        let inner_dir = tree.push(Some(b), "y-dir", true, ActionValues::new("", "", ""));

        tree.containers_first();

        assert_eq!(tree.root_ids(), [b, d, a, c]);
        assert_eq!(tree.children_of(b), [inner_dir, inner_file]);
    }
}
