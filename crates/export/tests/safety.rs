// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{Assignment, LabelId, LabelKind, Source, Status};
use export::{Control, ExportItem, Layout, export, export_with, preview, preview_with};
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

fn assign(kind: LabelKind, name: &str) -> Assignment {
    Assignment {
        label: LabelId(1),
        kind,
        name: name.into(),
        source: Source::User,
        confidence: 1.0,
        status: Status::Accepted,
    }
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

#[test]
fn previewing_and_exporting_in_all_four_layouts_leave_the_originals_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let specs = [
        (
            "a.jpg",
            &b"jpeg bytes"[..],
            vec![
                assign(LabelKind::Category, "Manga"),
                assign(LabelKind::Entity, "Obra A"),
            ],
        ),
        (
            "sub/b.png",
            &b"png bytes"[..],
            vec![assign(LabelKind::Category, "Fotos")],
        ),
        ("sub/deep/c.webp", &b"webp bytes"[..], vec![]),
    ];
    let mut items = vec![];
    for (rel, bytes, labels) in specs {
        let p = library.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, bytes).unwrap();
        items.push(ExportItem { source: p, labels });
    }
    let root = LibraryRoot::new(&library).unwrap();

    // Throwaway read so NTFS settles directory modified times before taking the baseline snapshot.
    snapshot(&library);
    let before = snapshot(&library);

    for (i, layout) in [
        Layout::Flat,
        Layout::ByCategory,
        Layout::ByEntity,
        Layout::ByYear,
    ]
    .into_iter()
    .enumerate()
    {
        let dest = ensure_export_dir(
            &tmp.path().join(format!("export_{i}")),
            std::slice::from_ref(&root),
        )
        .unwrap();
        preview_with(&items, &dest, layout).unwrap();
        export_with(&items, &dest, layout, &mut |_| Control::Continue).unwrap();
        export_with(&items, &dest, layout, &mut |_| Control::Continue).unwrap();
    }

    assert_eq!(before, snapshot(&library));
}
