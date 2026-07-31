//! Filesystem scanner that transforms a directory into source-neutral tree
//! data, lazily: `scan` walks only the top level, and each directory's
//! contents come from a targeted depth-1 walk when it is expanded or when the
//! app's background sweep reaches it. The targeted walk keeps full ignore
//! semantics — `ignore::WalkBuilder`'s standard filters read ignore files in
//! parent directories, so a lazily walked subdirectory honors the same rules
//! the eager scan did. Unreadable directories are reported through the
//! tree's error list (banner + stderr on exit) instead of being skipped
//! silently.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::tree::{NodeId, Tree};

/// Scan `dir`'s top level, honoring ignore files unless `no_ignore` is set.
///
/// Dotfiles are included. When ignore handling is enabled, repository metadata
/// (`.git` and `.jj`) is excluded too. Deeper directories materialize on
/// demand.
pub fn scan(dir: &Path, no_ignore: bool) -> std::io::Result<Tree> {
    let _span = crate::profile::span("fstree::scan");
    let root_dir = dir.canonicalize()?;
    let mut tree = Tree::new_fs(root_dir.clone(), no_ignore);
    walk_into(&mut tree, None, &root_dir, no_ignore);
    tree.containers_first();
    Ok(tree)
}

/// Materialize one directory's children with a targeted depth-1 walk.
pub(crate) fn materialize(tree: &mut Tree, id: NodeId) {
    let path = PathBuf::from(tree.path(id));
    let no_ignore = tree.fs_no_ignore();
    walk_into(tree, Some(id), &path, no_ignore);
    tree.containers_first_children(id);
    tree.mark_children_loaded(id);
}

/// Push `path`'s immediate entries under `parent`, sorted case-insensitively
/// (directories are reordered first by the callers).
fn walk_into(tree: &mut Tree, parent: Option<NodeId>, path: &Path, no_ignore: bool) {
    let walk = ignore::WalkBuilder::new(path)
        .standard_filters(!no_ignore)
        .hidden(false)
        .max_depth(Some(1))
        .sort_by_file_name(|a, b| a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()))
        .build();
    for entry in walk {
        match entry {
            Ok(entry) => {
                if entry.depth() == 0 {
                    continue; // the walked directory itself
                }
                if !no_ignore && is_repository_metadata(entry.file_name()) {
                    continue;
                }
                let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
                tree.push_fs(parent, entry.file_name(), is_dir);
            }
            Err(error) => tree.record_error(error.to_string()),
        }
    }
}

fn is_repository_metadata(name: &OsStr) -> bool {
    name == OsStr::new(".git") || name == OsStr::new(".jj")
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

    /// Scan and run the sweep to completion, as the app does moments after
    /// startup — structure-shape tests want the whole tree present.
    fn scan_all(dir: &Path) -> Tree {
        let mut tree = scan(dir, false).unwrap();
        tree.index_all();
        tree
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
    fn repository_metadata_directories_follow_the_ignore_setting() {
        let dir = fixture();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::create_dir(dir.path().join(".jj")).unwrap();
        std::fs::create_dir(dir.path().join("b-dir/.git")).unwrap();
        std::fs::create_dir(dir.path().join("b-dir/.jj")).unwrap();

        for (no_ignore, expected) in [(false, false), (true, true)] {
            let mut tree = scan(dir.path(), no_ignore).unwrap();
            let names = root_names(&tree);
            assert_eq!(names.contains(&".git".to_string()), expected, "{names:?}");
            assert_eq!(names.contains(&".jj".to_string()), expected, "{names:?}");

            let b_dir = tree
                .root_ids()
                .iter()
                .copied()
                .find(|&id| tree.name(id) == "b-dir")
                .unwrap();
            tree.ensure_children(b_dir);
            let child_names: Vec<_> = tree
                .children_of(b_dir)
                .iter()
                .map(|&id| tree.name(id))
                .collect();
            assert_eq!(
                child_names.contains(&".git".to_string()),
                expected,
                "{child_names:?}"
            );
            assert_eq!(
                child_names.contains(&".jj".to_string()),
                expected,
                "{child_names:?}"
            );
        }
    }

    #[test]
    fn scan_builds_only_the_top_level_and_dirs_materialize_on_demand() {
        let dir = fixture();
        let mut tree = scan(dir.path(), false).unwrap();
        let b_dir = tree.root_ids()[0];

        assert!(tree.children_of(b_dir).is_empty());
        assert!(!tree.is_leaf(b_dir), "an unwalked dir must stay expandable");
        assert!(!tree.fully_indexed());

        assert!(tree.ensure_children(b_dir));
        assert_eq!(tree.name(tree.children_of(b_dir)[0]), "inner.txt");

        tree.index_all();
        assert!(tree.fully_indexed());
        assert!(tree.errors().is_empty());
    }

    #[test]
    fn ancestor_ignore_rules_apply_to_lazily_walked_subdirectories() {
        let dir = fixture();
        std::fs::write(dir.path().join("b-dir/nested.log"), "").unwrap();
        let mut tree = scan(dir.path(), false).unwrap();
        let b_dir = tree.root_ids()[0];

        tree.ensure_children(b_dir);

        let names: Vec<String> = tree
            .children_of(b_dir)
            .iter()
            .map(|&id| tree.name(id))
            .collect();
        assert!(!names.contains(&"nested.log".to_string()), "{names:?}");
        assert!(names.contains(&"inner.txt".to_string()), "{names:?}");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directories_record_an_error_instead_of_vanishing() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let mut tree = scan(dir.path(), false).unwrap();
        tree.index_all();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(tree.fully_indexed());
        assert!(!tree.errors().is_empty());
    }

    #[test]
    fn children_and_depth() {
        let dir = fixture();
        let tree = scan_all(dir.path());
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
        let tree = scan_all(dir.path());
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
        let tree = scan_all(dir.path());
        let by_name = |name: &str| {
            tree.root_ids()
                .iter()
                .copied()
                .find(|&id| tree.name(id) == name)
                .unwrap()
        };
        assert!(!tree.is_leaf(by_name("b-dir")));
        // An empty directory is still a directory: a branch that opens to
        // nothing, never a leaf.
        assert!(!tree.is_leaf(by_name("empty-dir")));
        assert!(tree.is_leaf(by_name("a-file.txt")));
    }

    #[test]
    fn tree_model_children_match_nodes() {
        let dir = fixture();
        let tree = scan_all(dir.path());
        let b_dir = tree.root_ids()[0];
        match tree.children(b_dir) {
            TreeChildren::Loaded(kids) => assert_eq!(kids, tree.children_of(b_dir)),
            other => panic!("expected Loaded, got {other:?}"),
        }
        // A walked-empty directory stays a branch; `Unloaded` (not an empty
        // slice) is how the model keeps that fact visible to the widget.
        let empty = tree.root_ids()[1];
        assert_eq!(tree.children(empty), TreeChildren::Unloaded);
    }

    #[test]
    fn unwalked_directories_report_unloaded_children() {
        let dir = fixture();
        let tree = scan(dir.path(), false).unwrap();
        let b_dir = tree.root_ids()[0];
        assert_eq!(tree.children(b_dir), TreeChildren::Unloaded);
    }

    #[test]
    fn branches_lists_expandable_dirs() {
        let dir = fixture();
        let tree = scan_all(dir.path());
        let names: Vec<String> = tree.branches().map(|(id, _)| tree.name(id)).collect();
        assert_eq!(names, ["b-dir", "empty-dir"]);
    }
}
