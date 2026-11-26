// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::Catalog;
use fsguard::{LibraryRoot, ensure_writable};
use image::codecs::gif::GifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ColorType, Frame, ImageEncoder, RgbaImage};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

type Snapshot = BTreeMap<
    PathBuf,
    (
        bool,
        u64,
        Option<SystemTime>,
        Option<SystemTime>,
        Option<String>,
    ),
>;

/// Everything observable about a tree: kind, size, modified and created times and, for files, the SHA-256.
fn snapshot(root: &Path) -> Snapshot {
    WalkDir::new(root)
        .into_iter()
        .map(|e| {
            let e = e.unwrap();
            let md = e.metadata().unwrap();
            let hash = md
                .is_file()
                .then(|| scanner::sha256_file(e.path()).unwrap());
            (
                e.path().to_path_buf(),
                (
                    md.is_dir(),
                    md.len(),
                    md.modified().ok(),
                    md.created().ok(),
                    hash,
                ),
            )
        })
        .collect()
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

#[test]
fn running_rules_leaves_the_originals_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");

    let mut jpg = Vec::new();
    JpegEncoder::new(&mut jpg)
        .encode(&vec![64u8; 64 * 64 * 3], 64, 64, ColorType::Rgb8.into())
        .unwrap();
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(&vec![128u8; 100 * 50 * 3], 100, 50, ColorType::Rgb8.into())
        .unwrap();
    let mut gif = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut gif);
        enc.encode_frame(Frame::new(RgbaImage::new(32, 96)))
            .unwrap();
    }

    write(&library, "IMG-20240101-WA0001.jpg", &jpg);
    write(&library, "sub/Screenshot_1.png", &png);
    write(&library, "sub/deep/strip.gif", &gif);
    write(&library, "sub/corrupt.jpg", b"not a valid jpeg");

    let root = LibraryRoot::new(&library).unwrap();
    fs::create_dir_all(tmp.path().join("data")).unwrap();
    let db = ensure_writable(
        &tmp.path().join("data").join("catalog.sqlite"),
        std::slice::from_ref(&root),
    )
    .unwrap();
    let mut catalog = Catalog::open(&db).unwrap();
    catalog.index(&root).unwrap();

    // NTFS refreshes a directory's modified time lazily the first time it is listed from its parent,
    // so one throwaway read settles it. Without this the test compares a stale time with a fresh one.
    snapshot(&library);
    let before = snapshot(&library);

    rules::run(&catalog).unwrap();
    rules::run(&catalog).unwrap();

    assert_eq!(before, snapshot(&library));
}
