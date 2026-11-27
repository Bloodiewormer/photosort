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

#[test]
fn indexing_recognizes_video_media_type_and_facets() {
    let s = setup();
    write(&s.library, "clip.mp4", b"synthetic video bytes");
    let mut cat = Catalog::open(&db_path(&s)).unwrap();
    cat.index(&s.root).unwrap();

    let roots = [s.library.clone()];
    let facets = cat.metadata_facets_in_roots(&roots).unwrap();
    assert_eq!(facets.videos_count(), 1);
    assert_eq!(facets.photos_count(), 2);

    let entries = cat
        .entries(&catalog::Filter::default(), catalog::Sort::PathAsc)
        .unwrap();
    let video_entry = entries
        .iter()
        .find(|e| e.media_type == catalog::MediaType::Video)
        .expect("video entry");
    assert!(video_entry.path.ends_with("clip.mp4"));
}

#[test]
fn migration_preserves_existing_assets_labels_and_decisions() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("legacy_meta.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE asset (id INTEGER PRIMARY KEY, sha256 TEXT NOT NULL UNIQUE, size INTEGER NOT NULL);
             CREATE TABLE location (id INTEGER PRIMARY KEY, asset_id INTEGER NOT NULL, root TEXT NOT NULL, path TEXT NOT NULL UNIQUE, size INTEGER NOT NULL, mtime_ns INTEGER, seen_run INTEGER NOT NULL, missing INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE label (id INTEGER PRIMARY KEY, kind TEXT NOT NULL CHECK (kind IN ('category', 'tag', 'entity')), name TEXT NOT NULL COLLATE NOCASE, UNIQUE (kind, name));
             CREATE TABLE asset_label (asset_id INTEGER NOT NULL, label_id INTEGER NOT NULL, source TEXT NOT NULL, confidence REAL NOT NULL, status TEXT NOT NULL, PRIMARY KEY (asset_id, label_id));
             INSERT INTO asset (id, sha256, size) VALUES (1, 'aaa', 100);
             INSERT INTO location (asset_id, root, path, size, mtime_ns, seen_run, missing)
                 VALUES (1, 'R', 'R/a.jpg', 100, 1000, 1, 0);
             INSERT INTO label (id, kind, name) VALUES (1, 'tag', 'favorito');
             INSERT INTO asset_label (asset_id, label_id, source, confidence, status)
                 VALUES (1, 1, 'user', 1.0, 'accepted');",
        )
        .unwrap();
    }
    let db = ensure_writable(&db_path, &[]).unwrap();
    let migrated = Catalog::open(&db).unwrap();
    let assigns = migrated.labels_of(catalog::AssetId(1)).unwrap();
    assert_eq!(assigns.len(), 1);
    assert_eq!(assigns[0].name, "favorito");
    assert_eq!(assigns[0].status, catalog::Status::Accepted);
    let entry = migrated.entry_for(catalog::AssetId(1)).unwrap().unwrap();
    assert_eq!(entry.media_type, catalog::MediaType::Image);
    assert_eq!(entry.duration_ms, None);
}
