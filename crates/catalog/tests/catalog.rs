// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{Catalog, Counts};
use fsguard::{DestPath, LibraryRoot, ensure_writable};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Setup {
    tmp: TempDir,
    library: PathBuf,
    root: LibraryRoot,
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    write(&library, "a.jpg", b"aaaa");
    write(&library, "sub/b.png", b"bb");
    write(&library, "notes.txt", b"not an image");
    let root = LibraryRoot::new(&library).unwrap();
    Setup { tmp, library, root }
}

fn db_path(s: &Setup) -> DestPath {
    let dir = s.tmp.path().join("data");
    fs::create_dir_all(&dir).unwrap();
    ensure_writable(&dir.join("catalog.sqlite"), std::slice::from_ref(&s.root)).unwrap()
}

fn counts(assets: i64, locations: i64, missing: i64) -> Counts {
    Counts {
        assets,
        locations,
        missing,
    }
}

#[test]
fn the_first_index_adds_every_image() {
    let s = setup();
    let mut cat = Catalog::open(&db_path(&s)).unwrap();
    let r = cat.index(&s.root).unwrap();
    assert_eq!((r.new, r.changed, r.unchanged, r.hashed), (2, 0, 0, 2));
    assert_eq!(cat.counts().unwrap(), counts(2, 2, 0));
}

#[test]
fn a_second_index_reads_no_bytes_for_unchanged_files() {
    let s = setup();
    let mut cat = Catalog::open(&db_path(&s)).unwrap();
    cat.index(&s.root).unwrap();
    let r = cat.index(&s.root).unwrap();
    assert_eq!((r.new, r.changed, r.unchanged, r.hashed), (0, 0, 2, 0));
}

#[test]
fn a_modified_file_is_hashed_again() {
    let s = setup();
    let mut cat = Catalog::open(&db_path(&s)).unwrap();
    cat.index(&s.root).unwrap();
    write(&s.library, "a.jpg", b"a different and longer content");
    let r = cat.index(&s.root).unwrap();
    assert_eq!((r.new, r.changed, r.unchanged, r.hashed), (0, 1, 1, 1));
    assert_eq!(cat.counts().unwrap(), counts(2, 2, 0));
}

#[test]
fn a_deleted_file_is_flagged_missing_and_kept_in_the_catalog() {
    let s = setup();
    let mut cat = Catalog::open(&db_path(&s)).unwrap();
    cat.index(&s.root).unwrap();
    fs::remove_file(s.library.join("sub/b.png")).unwrap();
    let r = cat.index(&s.root).unwrap();
    assert_eq!(r.missing, 1);
    assert_eq!(cat.counts().unwrap(), counts(2, 2, 1));
    // when the file comes back it is found again, with nothing to hash if it is the same file
    write(&s.library, "sub/b.png", b"bb");
    let r = cat.index(&s.root).unwrap();
    assert_eq!(r.missing, 0);
    assert_eq!(cat.counts().unwrap().missing, 0);
}

#[test]
fn identical_bytes_in_two_paths_are_one_asset_with_two_locations() {
    let s = setup();
    write(&s.library, "sub/copy.jpg", b"aaaa");
    let mut cat = Catalog::open(&db_path(&s)).unwrap();
    cat.index(&s.root).unwrap();
    assert_eq!(cat.counts().unwrap(), counts(2, 3, 0));
    let groups = cat.duplicates().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].len(), 2);
}

#[test]
fn the_catalog_survives_being_reopened() {
    let s = setup();
    let db = db_path(&s);
    Catalog::open(&db).unwrap().index(&s.root).unwrap();
    let mut again = Catalog::open(&db).unwrap();
    assert_eq!(again.counts().unwrap(), counts(2, 2, 0));
    assert_eq!(again.index(&s.root).unwrap().hashed, 0);
}

#[test]
fn indexing_one_root_never_flags_another_roots_files_as_missing() {
    let s = setup();
    let other = s.tmp.path().join("scans");
    write(&other, "x.png", b"x");
    let other_root = LibraryRoot::new(&other).unwrap();
    let mut cat = Catalog::open(&db_path(&s)).unwrap();
    cat.index(&s.root).unwrap();
    let r = cat.index(&other_root).unwrap();
    assert_eq!(r.missing, 0);
    assert_eq!(cat.counts().unwrap(), counts(3, 3, 0));
}

#[cfg(windows)]
#[test]
fn an_unreadable_file_is_reported_and_the_rest_is_still_indexed() {
    use std::os::windows::fs::OpenOptionsExt;
    let s = setup();
    write(&s.library, "sub/locked.jpg", b"locked");
    // an exclusive lock makes opening the file for reading fail
    let _lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(s.library.join("sub/locked.jpg"))
        .unwrap();
    let mut cat = Catalog::open(&db_path(&s)).unwrap();
    let r = cat.index(&s.root).unwrap();
    assert_eq!(r.errors.len(), 1);
    assert_eq!((r.new, r.hashed), (2, 2));
}
