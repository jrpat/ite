//! Filesystem scanner that transforms a directory into source-neutral tree data.

use std::path::{Path, PathBuf};

use crate::tree::{NodeId, Tree};

/// Scan `dir`, honoring ignore files unless `no_ignore` is set.
///
/// Dotfiles are always included.
pub fn scan(dir: &Path, no_ignore: bool) -> std::io::Result<Tree> {
    let _span = crate::profile::span("fstree::scan");
    let root_dir = dir.canonicalize()?;
    let mut tree = Tree::new_fs(root_dir.clone());
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
        let id = tree.push_fs(parent, entry.file_name(), is_dir);
        if is_dir {
            ids_by_path.insert(path, id);
        }
    }

    // The walker sorts alphabetically; reorder each sibling list to put
    // directories first.
    tree.containers_first();
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_treelistview::{TreeChildren, TreeModel};

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

    fn root_names(tree: &Tree) -> Vec<String> {
        tree.root_ids().iter().map(|&id| tree.name(id)).collect()
    }

    #[test]
    fn default_scan_shows_dotfiles_but_honors_ignore_files() {
        let dir = fixture();
        let tree = scan(dir.path(), false).unwrap();
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
        let tree = scan(dir.path(), true).unwrap();
        let names = root_names(&tree);
        assert!(names.contains(&".hidden-file".to_string()));
        assert!(names.contains(&"ignored.log".to_string()));
    }

    #[test]
    fn children_and_depth() {
        let dir = fixture();
        let tree = scan(dir.path(), false).unwrap();
        let b_dir = tree.root_ids()[0];
        assert_eq!(tree.name(b_dir), "b-dir");
        assert_eq!(tree.depth(b_dir), 0);
        let kids = tree.children_of(b_dir).to_vec();
        assert_eq!(kids.len(), 1);
        assert_eq!(tree.name(kids[0]), "inner.txt");
        assert_eq!(tree.depth(kids[0]), 1);
        assert_eq!(tree.parent(kids[0]), Some(b_dir));
    }

    #[test]
    fn paths_are_absolute_and_relative() {
        let dir = fixture();
        let tree = scan(dir.path(), false).unwrap();
        let b_dir = tree.root_ids()[0];
        let inner = tree.children_of(b_dir)[0];
        assert!(Path::new(&tree.path(inner)).is_absolute());
        assert!(Path::new(&tree.path(inner)).ends_with("b-dir/inner.txt"));
        assert_eq!(
            tree.relpath(inner),
            Path::new("b-dir/inner.txt").as_os_str()
        );
        assert_eq!(
            tree.alternate_output(inner),
            std::ffi::OsStr::new("inner.txt")
        );
    }

    #[test]
    fn leaf_classification() {
        let dir = fixture();
        let tree = scan(dir.path(), false).unwrap();
        let by_name = |name: &str| {
            tree.root_ids()
                .iter()
                .copied()
                .find(|&id| tree.name(id) == name)
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
        let tree = scan(dir.path(), false).unwrap();
        let b_dir = tree.root_ids()[0];
        match tree.children(b_dir) {
            TreeChildren::Loaded(kids) => assert_eq!(kids, tree.children_of(b_dir)),
            other => panic!("expected Loaded, got {other:?}"),
        }
        let empty = tree.root_ids()[1];
        assert_eq!(tree.children(empty), TreeChildren::Leaf);
    }

    #[test]
    fn branches_lists_expandable_dirs() {
        let dir = fixture();
        let tree = scan(dir.path(), false).unwrap();
        let names: Vec<String> = tree.branches().map(|(id, _)| tree.name(id)).collect();
        assert_eq!(names, ["b-dir"]);
    }
}
