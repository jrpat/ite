//! Filesystem tree scanned with the `ignore` crate, exposed as a `TreeModel`.

use std::path::{Path, PathBuf};

use tui_treelistview::{TreeChildren, TreeModel, TreeRevision};

pub type NodeId = usize;

#[derive(Debug)]
pub struct Node {
    pub name: String,
    /// Absolute path.
    pub path: PathBuf,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub is_dir: bool,
    /// 0 for top-level entries of the scanned directory.
    pub depth: usize,
}

#[derive(Debug)]
pub struct FsTree {
    nodes: Vec<Node>,
    roots: Vec<NodeId>,
    root_dir: PathBuf,
}

impl FsTree {
    /// Scan `dir`, honoring ignore files unless `no_ignore` is set.
    ///
    /// Dotfiles are always included.
    pub fn scan(dir: &Path, no_ignore: bool) -> std::io::Result<Self> {
        let _span = crate::profile::span("fstree::scan");
        let root_dir = dir.canonicalize()?;
        let mut tree = Self {
            nodes: Vec::new(),
            roots: Vec::new(),
            root_dir: root_dir.clone(),
        };
        let mut ids_by_path: std::collections::HashMap<PathBuf, NodeId> =
            std::collections::HashMap::new();

        let walk = ignore::WalkBuilder::new(&root_dir)
            .standard_filters(!no_ignore)
            .hidden(false)
            .sort_by_file_name(|a, b| a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()))
            .build();
        for entry in walk {
            let Ok(entry) = entry else { continue };
            if entry.depth() == 0 {
                continue; // the scanned directory itself
            }
            let path = entry.path().to_path_buf();
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            let parent = path.parent().and_then(|p| ids_by_path.get(p)).copied();
            let id = tree.nodes.len();
            let depth = entry.depth() - 1;
            tree.nodes.push(Node {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: path.clone(),
                parent,
                children: Vec::new(),
                is_dir,
                depth,
            });
            match parent {
                Some(parent) => tree.nodes[parent].children.push(id),
                None => tree.roots.push(id),
            }
            if is_dir {
                ids_by_path.insert(path, id);
            }
        }

        // The walker sorts alphabetically; reorder each sibling list to put
        // directories first.
        let dirs_first = |nodes: &[Node], ids: &mut Vec<NodeId>| {
            ids.sort_by_key(|&id| !nodes[id].is_dir);
        };
        let mut roots = std::mem::take(&mut tree.roots);
        dirs_first(&tree.nodes, &mut roots);
        tree.roots = roots;
        for id in 0..tree.nodes.len() {
            let mut children = std::mem::take(&mut tree.nodes[id].children);
            dirs_first(&tree.nodes, &mut children);
            tree.nodes[id].children = children;
        }
        Ok(tree)
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

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub fn root_ids(&self) -> &[NodeId] {
        &self.roots
    }

    /// Path relative to the scanned directory.
    pub fn rel_path(&self, id: NodeId) -> &Path {
        self.nodes[id]
            .path
            .strip_prefix(&self.root_dir)
            .unwrap_or(&self.nodes[id].path)
    }

    /// True when the node cannot be expanded (a file or an empty directory).
    pub fn is_leaf(&self, id: NodeId) -> bool {
        self.nodes[id].children.is_empty()
    }

    /// All expandable (non-leaf) nodes as `(id, parent)` pairs, in tree order.
    pub fn branches(&self) -> impl Iterator<Item = (NodeId, Option<NodeId>)> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(id, node)| node.is_dir && !self.is_leaf(*id))
            .map(|(id, node)| (id, node.parent))
    }
}

impl TreeModel for FsTree {
    type Id = NodeId;

    fn roots(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.roots.iter().copied()
    }

    fn children(&self, id: NodeId) -> TreeChildren<'_, NodeId> {
        TreeChildren::loaded(&self.nodes[id].children)
    }

    fn revision(&self) -> TreeRevision {
        TreeRevision::INITIAL
    }

    fn size_hint(&self) -> usize {
        self.nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds:
    ///   root/
    ///     .hidden-file
    ///     b-dir/
    ///       inner.txt
    ///     empty-dir/
    ///     a-file.txt
    ///     z-file.txt
    ///     ignored.log     (matched by .ignore)
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(p.join(".hidden-file"), "").unwrap();
        std::fs::create_dir(p.join("b-dir")).unwrap();
        std::fs::write(p.join("b-dir/inner.txt"), "").unwrap();
        std::fs::create_dir(p.join("empty-dir")).unwrap();
        std::fs::write(p.join("a-file.txt"), "").unwrap();
        std::fs::write(p.join("z-file.txt"), "").unwrap();
        std::fs::write(p.join("ignored.log"), "").unwrap();
        std::fs::write(p.join(".ignore"), "*.log\n").unwrap();
        dir
    }

    fn root_names(tree: &FsTree) -> Vec<String> {
        tree.root_ids()
            .iter()
            .map(|&id| tree.node(id).name.clone())
            .collect()
    }

    #[test]
    fn default_scan_shows_dotfiles_but_honors_ignore_files() {
        let dir = fixture();
        let tree = FsTree::scan(dir.path(), false).unwrap();
        // Dirs come first, then files, each sorted case-insensitively.
        assert_eq!(
            root_names(&tree),
            [
                "b-dir",
                "empty-dir",
                ".hidden-file",
                ".ignore",
                "a-file.txt",
                "z-file.txt",
            ]
        );
        assert!(!root_names(&tree).contains(&"ignored.log".to_string()));
    }

    #[test]
    fn no_ignore_reveals_ignored_files() {
        let dir = fixture();
        let tree = FsTree::scan(dir.path(), true).unwrap();
        let names = root_names(&tree);
        assert!(names.contains(&".hidden-file".to_string()));
        assert!(names.contains(&"ignored.log".to_string()));
    }

    #[test]
    fn children_and_depth() {
        let dir = fixture();
        let tree = FsTree::scan(dir.path(), false).unwrap();
        let b_dir = tree.root_ids()[0];
        assert_eq!(tree.node(b_dir).name, "b-dir");
        assert_eq!(tree.node(b_dir).depth, 0);
        let kids = tree.node(b_dir).children.clone();
        assert_eq!(kids.len(), 1);
        assert_eq!(tree.node(kids[0]).name, "inner.txt");
        assert_eq!(tree.node(kids[0]).depth, 1);
        assert_eq!(tree.node(kids[0]).parent, Some(b_dir));
    }

    #[test]
    fn paths_are_absolute_and_relative() {
        let dir = fixture();
        let tree = FsTree::scan(dir.path(), false).unwrap();
        let b_dir = tree.root_ids()[0];
        let inner = tree.node(b_dir).children[0];
        assert!(tree.node(inner).path.is_absolute());
        assert!(tree.node(inner).path.ends_with("b-dir/inner.txt"));
        assert_eq!(tree.rel_path(inner), Path::new("b-dir/inner.txt"));
    }

    #[test]
    fn leaf_classification() {
        let dir = fixture();
        let tree = FsTree::scan(dir.path(), false).unwrap();
        let by_name = |name: &str| {
            tree.root_ids()
                .iter()
                .copied()
                .find(|&id| tree.node(id).name == name)
                .unwrap()
        };
        assert!(!tree.is_leaf(by_name("b-dir")));
        // An empty directory cannot be expanded, so it is a leaf.
        assert!(tree.is_leaf(by_name("empty-dir")));
        assert!(tree.is_leaf(by_name("a-file.txt")));
    }

    #[test]
    fn tree_model_children_match_nodes() {
        let dir = fixture();
        let tree = FsTree::scan(dir.path(), false).unwrap();
        let b_dir = tree.root_ids()[0];
        match tree.children(b_dir) {
            TreeChildren::Loaded(kids) => assert_eq!(kids, tree.node(b_dir).children.as_slice()),
            other => panic!("expected Loaded, got {other:?}"),
        }
        let empty = tree.root_ids()[1];
        assert_eq!(tree.children(empty), TreeChildren::Leaf);
    }

    #[test]
    fn branches_lists_expandable_dirs() {
        let dir = fixture();
        let tree = FsTree::scan(dir.path(), false).unwrap();
        let names: Vec<&str> = tree
            .branches()
            .map(|(id, _)| tree.node(id).name.as_str())
            .collect();
        assert_eq!(names, ["b-dir"]);
    }
}
