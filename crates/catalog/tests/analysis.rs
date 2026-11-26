// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{AssetId, Catalog, Pending};
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

/// A library with three different images, already indexed.
fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    write(&library, "memes/funny.jpg", b"one");
    write(&library, "shots/chat.png", b"two");
    write(&library, "manga/page.webp", b"three");
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
    fn asset(&self, rel: &str) -> AssetId {
        let path = dunce::canonicalize(self.library.join(rel)).unwrap();
        self.cat.asset_of(&path).unwrap().expect("indexed")
    }

    fn canonical(&self, rel: &str) -> PathBuf {
        dunce::canonicalize(self.library.join(rel)).unwrap()
    }
}

#[test]
fn everything_is_pending_at_first_ordered_by_asset_id() {
    let s = setup();
    let pending: Vec<Pending> = s.cat.pending("rules", 1, 10).unwrap();
    assert_eq!(pending.len(), 3);
    assert!(pending.windows(2).all(|w| w[0].asset.0 < w[1].asset.0));
    for item in &pending {
        assert_eq!(s.cat.asset_of(&item.path).unwrap(), Some(item.asset));
    }
}

#[test]
fn after_mark_done_nothing_is_pending() {
    let s = setup();
    for item in s.cat.pending("rules", 1, 10).unwrap() {
        s.cat.mark_done(item.asset, "rules", 1).unwrap();
    }
    assert!(s.cat.pending("rules", 1, 10).unwrap().is_empty());
}

#[test]
fn bumping_the_version_makes_everything_pending_again() {
    let s = setup();
    for item in s.cat.pending("rules", 1, 10).unwrap() {
        s.cat.mark_done(item.asset, "rules", 1).unwrap();
    }
    assert!(s.cat.pending("rules", 1, 10).unwrap().is_empty());
    assert_eq!(s.cat.pending("rules", 2, 10).unwrap().len(), 3);
}

#[test]
fn mark_done_never_lowers_a_recorded_version() {
    let s = setup();
    let funny = s.asset("memes/funny.jpg");
    s.cat.mark_done(funny, "rules", 3).unwrap();
    s.cat.mark_done(funny, "rules", 1).unwrap();
    let pending_v2 = s.cat.pending("rules", 2, 10).unwrap();
    assert!(!pending_v2.iter().any(|p| p.asset == funny));
    let pending_v3 = s.cat.pending("rules", 3, 10).unwrap();
    assert!(!pending_v3.iter().any(|p| p.asset == funny));
    let pending_v4 = s.cat.pending("rules", 4, 10).unwrap();
    assert!(pending_v4.iter().any(|p| p.asset == funny));
}

#[test]
fn a_new_file_appears_as_pending_while_done_ones_stay_done() {
    let mut s = setup();
    for item in s.cat.pending("rules", 1, 10).unwrap() {
        s.cat.mark_done(item.asset, "rules", 1).unwrap();
    }
    write(&s.library, "new/extra.jpg", b"four");
    s.cat.index(&s.root).unwrap();

    let pending = s.cat.pending("rules", 1, 10).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].asset, s.asset("new/extra.jpg"));
    assert_eq!(pending[0].path, s.canonical("new/extra.jpg"));
}

#[test]
fn missing_files_are_not_pending() {
    let mut s = setup();
    let funny = s.asset("memes/funny.jpg");
    fs::remove_file(s.library.join("memes/funny.jpg")).unwrap();
    s.cat.index(&s.root).unwrap();

    let pending = s.cat.pending("rules", 1, 10).unwrap();
    assert_eq!(pending.len(), 2);
    assert!(!pending.iter().any(|p| p.asset == funny));
}

#[test]
fn duplicate_locations_yield_the_smallest_non_missing_path() {
    let mut s = setup();
    write(&s.library, "aaa/copy.jpg", b"one");
    write(&s.library, "zzz/copy.jpg", b"one");
    s.cat.index(&s.root).unwrap();
    let asset = s.asset("memes/funny.jpg");

    let pending = s.cat.pending("rules", 1, 10).unwrap();
    let item = pending.iter().find(|p| p.asset == asset).unwrap();
    assert_eq!(item.path, s.canonical("aaa/copy.jpg"));

    fs::remove_file(s.library.join("aaa/copy.jpg")).unwrap();
    s.cat.index(&s.root).unwrap();

    let pending = s.cat.pending("rules", 1, 10).unwrap();
    let item = pending.iter().find(|p| p.asset == asset).unwrap();
    assert_eq!(item.path, s.canonical("memes/funny.jpg"));
}

#[test]
fn limit_is_respected() {
    let s = setup();
    assert!(s.cat.pending("rules", 1, 0).unwrap().is_empty());

    let first_two = s.cat.pending("rules", 1, 2).unwrap();
    assert_eq!(first_two.len(), 2);

    for item in &first_two {
        s.cat.mark_done(item.asset, "rules", 1).unwrap();
    }
    let remaining = s.cat.pending("rules", 1, 2).unwrap();
    assert_eq!(remaining.len(), 1);
}

#[test]
fn stages_are_independent() {
    let s = setup();
    for item in s.cat.pending("rules", 1, 10).unwrap() {
        s.cat.mark_done(item.asset, "rules", 1).unwrap();
    }
    assert!(s.cat.pending("rules", 1, 10).unwrap().is_empty());
    assert_eq!(s.cat.pending("ocr", 1, 10).unwrap().len(), 3);
}
