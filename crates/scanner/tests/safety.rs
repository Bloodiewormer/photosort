// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use scanner::{scan, sha256_file};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

type Snapshot = BTreeMap<PathBuf, (bool, u64, Option<SystemTime>, Option<String>)>;

/// Everything observable about a tree: kind, size, modified time and, for files, the SHA-256.
fn snapshot(root: &Path) -> Snapshot {
    WalkDir::new(root)
        .into_iter()
        .map(|e| {
            let e = e.unwrap();
            let md = e.metadata().unwrap();
            let hash = md.is_file().then(|| sha256_file(e.path()).unwrap());
            (
                e.path().to_path_buf(),
                (md.is_dir(), md.len(), md.modified().ok(), hash),
            )
        })
        .collect()
}

#[test]
fn scanning_and_hashing_leave_the_originals_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    for (rel, bytes) in [
        ("a.jpg", &b"jpeg bytes"[..]),
        ("sub/b.png", b"png bytes"),
        ("sub/deep/c.webp", b"webp bytes"),
        ("sub/notes.txt", b"text"),
    ] {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    // NTFS refreshes a directory's modified time lazily the first time it is listed from its parent,
    // so one throwaway read settles it. Without this the test compares a stale time with a fresh one.
    snapshot(&root);
    let before = snapshot(&root);
    let report = scan(&root).unwrap();
    for f in &report.files {
        sha256_file(&f.path).unwrap();
    }
    let after = snapshot(&root);

    assert_eq!(report.files.len(), 3);
    assert_eq!(before, after);
}
