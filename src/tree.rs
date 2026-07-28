//! Source-neutral tree data consumed by the application and renderer.

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
pub struct Node {
    pub name: String,
    /// Optional secondary text rendered after the name.
    pub detail: Option<String>,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    /// Whether this node represents a container, including an empty one.
    pub is_container: bool,
    /// 0 for roots.
    pub depth: usize,
    pub action: ActionValues,
}

#[derive(Debug, Default)]
pub struct Tree {
    pub(crate) nodes: Vec<Node>,
    pub(crate) roots: Vec<NodeId>,
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

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id]
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
        self.nodes[id].children.is_empty()
    }

    /// All expandable nodes as `(id, parent)` pairs, in tree order.
    pub fn branches(&self) -> impl Iterator<Item = (NodeId, Option<NodeId>)> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(id, _)| !self.is_leaf(*id))
            .map(|(id, node)| (id, node.parent))
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
