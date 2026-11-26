// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::worker::{IndexEvent, IndexWorker, PreviewWorker};
use fsguard::{LibraryRoot, ensure_writable};
use image::{ImageBuffer, Rgba};
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

fn write_png(path: &Path, color: [u8; 4]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = ImageBuffer::from_pixel(16, 12, Rgba(color));
    img.save(path).unwrap();
}

#[test]
fn worker_indexing_and_preview_leave_the_originals_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let a = library.join("a.png");
    let b = library.join("sub/b.png");
    let c = library.join("sub/deep/c.png");
    write_png(&a, [255, 0, 0, 255]);
    write_png(&b, [0, 255, 0, 255]);
    write_png(&c, [0, 0, 255, 255]);
    fs::write(library.join("sub/notes.txt"), b"text").unwrap();

    let root = LibraryRoot::new(&library).unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db = ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();

    // NTFS refreshes a directory's modified time lazily the first time it is listed from its parent,
    // so one throwaway read settles it. Without this the test compares a stale time with a fresh one.
    snapshot(&library);
    let before = snapshot(&library);

    let worker = IndexWorker::start(db, vec![root], None);
    while let Ok(ev) = worker.recv() {
        match ev {
            IndexEvent::Finished { stopped, new, .. } => {
                assert!(!stopped);
                assert_eq!(new, 3);
                break;
            }
            IndexEvent::Failed(e) => panic!("indexing failed: {e}"),
            _ => {}
        }
    }

    // Also exercise read-only original decoding via the inspector preview worker.
    let preview = PreviewWorker::new(8, None);
    preview.request(a.clone());
    let loaded = preview.recv().unwrap();
    assert!(loaded.image.is_ok());

    assert_eq!(before, snapshot(&library));
}
