// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{Catalog, DuplicateGroup};
use fsguard::{LibraryRoot, ensure_writable};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Setup {
    _tmp: TempDir,
    library: PathBuf,
    root: LibraryRoot,
    cat: Catalog,
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn setup_with(files: &[(&str, &[u8])]) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir_all(&library).unwrap();
    for (rel, bytes) in files {
        write(&library, rel, bytes);
    }
    let root = LibraryRoot::new(&library).unwrap();
    fs::create_dir_all(tmp.path().join("data")).unwrap();
    let db = ensure_writable(
        &tmp.path().join("data").join("catalog.sqlite"),
        std::slice::from_ref(&root),
    )
    .unwrap();
    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&root).unwrap();
    Setup {
        _tmp: tmp,
        library,
        root,
        cat,
    }
}

impl Setup {
    fn rel_paths(&self, group: &DuplicateGroup) -> Vec<String> {
        let base = dunce::canonicalize(&self.library).unwrap();
        group
            .entries
            .iter()
            .map(|e| {
                e.path
                    .strip_prefix(&base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }
}

#[test]
fn library_without_duplicates_returns_empty_groups() {
    let s = setup_with(&[
        ("a.jpg", b"one"),
        ("b.png", b"two"),
        ("sub/c.webp", b"three"),
    ]);
    assert!(s.cat.duplicate_groups().unwrap().is_empty());
}

#[test]
fn duplicate_groups_returns_entries_ordered_by_path_with_metadata() {
    let s = setup_with(&[
        ("z/copy.jpg", b"identical-bytes"),
        ("a/orig.jpg", b"identical-bytes"),
        ("m/unique.png", b"only-once"),
    ]);

    let groups = s.cat.duplicate_groups().unwrap();
    assert_eq!(groups.len(), 1);
    let g = &groups[0];
    let expected_sha = scanner::sha256_file(&s.library.join("a/orig.jpg")).unwrap();
    assert_eq!(g.sha256, expected_sha);
    assert_eq!(g.size, b"identical-bytes".len() as u64);
    assert_eq!(s.rel_paths(g), vec!["a/orig.jpg", "z/copy.jpg"]);
    assert!(g.entries.iter().all(|e| e.sha256 == expected_sha));
    assert!(g.entries.iter().all(|e| e.size == g.size));
    assert_eq!(g.entries[0].asset, g.entries[1].asset);
    assert!(g.entries.iter().all(|e| e.modified_ns.is_some()));
}

#[test]
fn duplicate_groups_orders_by_wasted_bytes_desc_then_sha256_asc() {
    let small_3x = [b'S'; 10]; // 3 copies -> wasted = 20
    let tie_2x = [b'T'; 20]; // 2 copies -> wasted = 20 (ties small_3x)
    let big_2x = [b'B'; 25]; // 2 copies -> wasted = 25 (highest)
    let tiny_4x = [b'X'; 4]; // 4 copies -> wasted = 12 (lowest)

    let s = setup_with(&[
        ("s/1.jpg", &small_3x),
        ("s/2.jpg", &small_3x),
        ("s/3.jpg", &small_3x),
        ("t/1.jpg", &tie_2x),
        ("t/2.jpg", &tie_2x),
        ("b/1.jpg", &big_2x),
        ("b/2.jpg", &big_2x),
        ("x/1.jpg", &tiny_4x),
        ("x/2.jpg", &tiny_4x),
        ("x/3.jpg", &tiny_4x),
        ("x/4.jpg", &tiny_4x),
    ]);

    let groups = s.cat.duplicate_groups().unwrap();
    assert_eq!(groups.len(), 4);

    let sha_small = scanner::sha256_file(&s.library.join("s/1.jpg")).unwrap();
    let sha_tie = scanner::sha256_file(&s.library.join("t/1.jpg")).unwrap();
    let sha_big = scanner::sha256_file(&s.library.join("b/1.jpg")).unwrap();
    let sha_tiny = scanner::sha256_file(&s.library.join("x/1.jpg")).unwrap();

    let mut tied = [sha_small, sha_tie];
    tied.sort();

    let actual_shas: Vec<&str> = groups.iter().map(|g| g.sha256.as_str()).collect();
    assert_eq!(
        actual_shas,
        vec![
            sha_big.as_str(),
            tied[0].as_str(),
            tied[1].as_str(),
            sha_tiny.as_str(),
        ]
    );
}

#[test]
fn missing_locations_are_excluded_and_shrink_or_drop_groups() {
    let big_3x = [b'A'; 10]; // 3 copies -> wasted 20; after removing 1 -> wasted 10
    let mid_2x = [b'B'; 15]; // 2 copies -> wasted 15
    let pair_2x = [b'C'; 30]; // 2 copies -> wasted 30; after removing 1 -> dropped

    let mut s = setup_with(&[
        ("a/1.jpg", &big_3x),
        ("a/2.jpg", &big_3x),
        ("a/3.jpg", &big_3x),
        ("b/1.jpg", &mid_2x),
        ("b/2.jpg", &mid_2x),
        ("c/1.jpg", &pair_2x),
        ("c/2.jpg", &pair_2x),
    ]);

    assert_eq!(s.cat.duplicate_groups().unwrap().len(), 3);

    // Delete one copy from `c` (leaving 1 copy -> no longer a duplicate group)
    // and one copy from `a` (leaving 2 copies -> wasted drops from 20 to 10, below `b`'s 15).
    fs::remove_file(s.library.join("c/2.jpg")).unwrap();
    fs::remove_file(s.library.join("a/2.jpg")).unwrap();
    s.cat.index(&s.root).unwrap();

    let groups = s.cat.duplicate_groups().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(s.rel_paths(&groups[0]), vec!["b/1.jpg", "b/2.jpg"]);
    assert_eq!(s.rel_paths(&groups[1]), vec!["a/1.jpg", "a/3.jpg"]);
}
