// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use export::{ExportItem, export, preview};
use fsguard::{LibraryRoot, ensure_export_dir};
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

#[test]
fn previewing_and_exporting_leave_the_originals_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let mut items = vec![];
    for (rel, bytes) in [
        ("a.jpg", &b"jpeg bytes"[..]),
        ("sub/b.png", b"png bytes"),
        ("sub/deep/c.webp", b"webp bytes"),
    ] {
        let p = library.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, bytes).unwrap();
        items.push(ExportItem {
            source: p,
            labels: vec![],
        });
    }
    let root = LibraryRoot::new(&library).unwrap();
    let dest = ensure_export_dir(&tmp.path().join("export"), &[root]).unwrap();

    // NTFS refreshes a directory's modified time lazily the first time it is listed from its parent,
    // so one throwaway read settles it. Without this the test compares a stale time with a fresh one.
    snapshot(&library);
    let before = snapshot(&library);

    preview(&items, &dest).unwrap();
    export(&items, &dest).unwrap();
    export(&items, &dest).unwrap();

    assert_eq!(before, snapshot(&library));
}
