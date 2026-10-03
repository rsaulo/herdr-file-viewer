//! Media must use the same regular-file / viewed-root boundary as text (AC-N5).
//! Real PNG fixtures exercise decoding; no PDF renderer, browser, or terminal is required.

mod common;

use common::TempDir;
use herdr_file_viewer::{media, render};
use std::fs;
use std::path::{Path, PathBuf};

fn image_fixture(root: &Path) -> PathBuf {
    let path = root.join("sample.png");
    image::RgbImage::from_pixel(2, 2, image::Rgb([255, 0, 0]))
        .save(&path)
        .unwrap();
    path
}

#[test]
fn raster_decodes_a_regular_image_inside_the_root() {
    let dir = TempDir::new();
    let path = image_fixture(dir.path());
    let png = media::rasterize_png(dir.path(), &path).expect("in-root PNG renders");
    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (2, 2));
}

#[test]
fn raster_and_browser_refuse_direct_and_parent_traversal_outside_the_root() {
    let dir = TempDir::new();
    let outside = image_fixture(dir.path());
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    for path in [outside, root.join("../sample.png")] {
        assert!(media::rasterize_png(&root, &path).is_none());
        assert!(media::browser_url(&root, &path).is_none());
        assert_eq!(
            render::classify(&root, &path, render::Caps::default()),
            render::Prepared::Unavailable {
                reason: render::UnavailableReason::OutsideViewedRoot,
            }
        );
    }
}

#[test]
fn browser_refuses_every_supported_kind_outside_the_root() {
    let dir = TempDir::new();
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    for extension in ["md", "html", "pdf", "png"] {
        let path = dir.path().join(format!("outside.{extension}"));
        fs::write(&path, b"outside content").unwrap();
        // Markdown must be refused BEFORE reading or writing temporary HTML.
        assert!(media::browser_url(&root, &path).is_none(), "{extension}");
    }
}

#[test]
fn media_refuses_missing_files_and_non_regular_paths() {
    let dir = TempDir::new();
    let directory = dir.path().join("directory.png");
    fs::create_dir(&directory).unwrap();
    for path in [directory, dir.path().join("missing.png")] {
        assert!(media::rasterize_png(dir.path(), &path).is_none());
        assert!(media::browser_url(dir.path(), &path).is_none());
    }
}

#[test]
fn browser_accepts_an_existing_html_file_inside_the_root() {
    let dir = TempDir::new();
    let path = dir.path().join("page.html");
    fs::write(&path, "<h1>Local</h1>").unwrap();
    let url = media::browser_url(dir.path(), &path).expect("regular in-root HTML");
    assert!(url.starts_with("file:///"), "{url}");
    assert!(url.ends_with("/page.html"), "{url}");
}

#[cfg(unix)]
#[test]
fn out_of_root_symlinks_are_blocked_for_raster_and_every_browser_kind() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new();
    let outside = image_fixture(dir.path());
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    for extension in ["png", "pdf", "html", "md"] {
        let link = root.join(format!("external.{extension}"));
        symlink(&outside, &link).unwrap();
        assert!(media::browser_url(&root, &link).is_none(), "{extension}");
        if extension == "png" {
            // Before the fix this decoded the external PNG despite the text guard refusing it.
            assert!(media::rasterize_png(&root, &link).is_none());
        }
    }
}

#[cfg(unix)]
#[test]
fn in_root_symlinks_still_render_and_broken_symlinks_are_refused() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new();
    let path = image_fixture(dir.path());
    let link = dir.path().join("internal.png");
    symlink(&path, &link).unwrap();
    assert!(media::rasterize_png(dir.path(), &link).is_some());
    assert!(
        media::browser_url(dir.path(), &link)
            .unwrap()
            .ends_with("/sample.png")
    );

    let broken = dir.path().join("broken.png");
    symlink("does-not-exist", &broken).unwrap();
    assert!(media::rasterize_png(dir.path(), &broken).is_none());
    assert!(media::browser_url(dir.path(), &broken).is_none());
}
