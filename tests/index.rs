mod common;

use herdr_file_viewer::index;
use std::fs;

// (a) AC-12: a file nested >= 2 levels deep appears (recursive walk)
#[test]
fn nested_file_appears() {
    let tmp = common::TempDir::new();
    let root = tmp.path();
    fs::create_dir_all(root.join("a/b")).unwrap();
    fs::write(root.join("a/b/deep.rs"), "").unwrap();

    let paths = index::build(root);
    assert!(
        paths.iter().any(|p| p == "a/b/deep.rs"),
        "expected a/b/deep.rs in index, got: {paths:?}"
    );
}

// (b) AC-13: a .gitignored file is absent
#[test]
fn gitignored_file_is_absent() {
    let tmp = common::TempDir::new();
    let root = tmp.path();
    fs::write(root.join(".gitignore"), "secret.txt\n").unwrap();
    fs::write(root.join("secret.txt"), "hidden").unwrap();
    fs::write(root.join("visible.txt"), "shown").unwrap();

    let paths = index::build(root);
    assert!(
        !paths.iter().any(|p| p == "secret.txt"),
        "secret.txt must be absent (gitignored)"
    );
    assert!(
        paths.iter().any(|p| p == "visible.txt"),
        "visible.txt must be present"
    );
}

// (c) AC-14: nothing under .git/ appears
#[test]
fn git_subtree_is_excluded() {
    let tmp = common::TempDir::new();
    let root = tmp.path();
    common::init_repo_with_commit(root);

    let paths = index::build(root);
    assert!(
        !paths.iter().any(|p| p.starts_with(".git")),
        "no path may start with .git, got: {paths:?}"
    );
}

// (d) AC-15: directories are NOT in the list, only files
#[test]
fn directories_not_in_index() {
    let tmp = common::TempDir::new();
    let root = tmp.path();
    fs::create_dir_all(root.join("subdir")).unwrap();
    fs::write(root.join("subdir/file.txt"), "").unwrap();

    let paths = index::build(root);
    assert!(
        !paths.iter().any(|p| p == "subdir"),
        "bare directory 'subdir' must not appear in index"
    );
    assert!(
        paths.iter().any(|p| p == "subdir/file.txt"),
        "subdir/file.txt must be in the index"
    );
}

// (e) AC-N5: every returned path is root-relative — no absolute paths, no ".."
#[test]
fn all_paths_are_root_relative() {
    let tmp = common::TempDir::new();
    let root = tmp.path();
    fs::create_dir_all(root.join("a/b")).unwrap();
    fs::write(root.join("a/b/deep.rs"), "").unwrap();
    fs::write(root.join("top.txt"), "").unwrap();

    let paths = index::build(root);
    assert!(!paths.is_empty(), "index must not be empty");
    for p in &paths {
        assert!(
            !std::path::Path::new(p).is_absolute(),
            "path must not be absolute: {p}"
        );
        assert!(!p.contains(".."), "path must not contain '..': {p}");
    }
}

// (f) AC-18: build is fresh each call — a new file added between calls appears
#[test]
fn rebuild_includes_new_file() {
    let tmp = common::TempDir::new();
    let root = tmp.path();
    fs::write(root.join("first.txt"), "").unwrap();

    let before = index::build(root);
    assert!(before.iter().any(|p| p == "first.txt"));
    assert!(!before.iter().any(|p| p == "second.txt"));

    fs::write(root.join("second.txt"), "").unwrap();
    let after = index::build(root);
    assert!(
        after.iter().any(|p| p == "second.txt"),
        "second.txt must appear after it is created"
    );
}

// (g) AC-19: works in a non-git directory without error
#[test]
fn works_in_non_git_dir() {
    let tmp = common::TempDir::new();
    let root = tmp.path();
    // No git init — plain directory
    fs::write(root.join("plain.txt"), "hello").unwrap();

    let paths = index::build(root);
    assert!(
        paths.iter().any(|p| p == "plain.txt"),
        "plain.txt must appear in a non-git dir"
    );
}

// (i) Regression: `build_scoped(root, true)` bounds the ancestor `.gitignore` search at root's
// own repo boundary instead of letting it climb into an unrelated enclosing directory. See the
// matching `tree_filters.rs` test for the Tree Model side of the same fix.
#[test]
fn build_scoped_bounds_ancestor_gitignore_at_the_repo_boundary() {
    let outer = common::TempDir::new();
    fs::write(outer.path().join(".gitignore"), "vendor/\n").unwrap();
    let inner = outer.path().join("inner");
    fs::create_dir_all(inner.join("vendor")).unwrap();
    common::init_repo_with_commit(&inner);
    fs::write(inner.join("vendor/keep.txt"), "k").unwrap();

    // `build` (is_git_repo = false, today's default) is unbounded and still picks up the outer
    // ancestor's unrelated `vendor/` rule.
    let unbounded = index::build(&inner);
    assert!(
        !unbounded.iter().any(|p| p.starts_with("vendor/")),
        "sanity check: the outer ancestor .gitignore reaches in when not told this is a repo"
    );

    // `build_scoped(&inner, true)` bounds the search at `inner`'s own `.git`.
    let bounded = index::build_scoped(&inner, true);
    assert!(
        bounded.iter().any(|p| p == "vendor/keep.txt"),
        "an unrelated ancestor .gitignore outside the repo must not hide files inside it, got: {bounded:?}"
    );
}

// (h) AC-N1: the filesystem is unchanged after build
#[test]
fn filesystem_unchanged_after_build() {
    let tmp = common::TempDir::new();
    let root = tmp.path();
    fs::write(root.join("file.txt"), "content").unwrap();

    let before: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();

    let _ = index::build(root);

    let after: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();

    assert_eq!(
        before.len(),
        after.len(),
        "build must not add/remove entries in root"
    );
}
