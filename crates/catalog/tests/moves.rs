// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{
    AssetId, Catalog, Filter, LabelKind, MoveMethod, MoveRow, MoveState, NewMove, Sort, Source,
};
use fsguard::{DestPath, LibraryRoot, ensure_writable};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

struct Setup {
    _tmp: TempDir,
    library: PathBuf,
    outside: PathBuf,
    root: LibraryRoot,
    db: DestPath,
    cat: Catalog,
}

/// A library with two tagged photos, already indexed, and an empty folder outside it.
fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let library = base.join("library");
    let outside = base.join("outside");
    for dir in [&library.join("sorted"), &outside, &base.join("data")] {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(library.join("a.jpg"), b"photo a").unwrap();
    fs::write(library.join("b.jpg"), b"photo bb").unwrap();
    let root = LibraryRoot::new(&library).unwrap();
    let db = ensure_writable(
        &base.join("data").join("catalog.sqlite"),
        std::slice::from_ref(&root),
    )
    .unwrap();
    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&root).unwrap();
    let tag = cat.label(LabelKind::Tag, "viaje").unwrap();
    for name in ["a.jpg", "b.jpg"] {
        let asset = cat.asset_of(&library.join(name)).unwrap().unwrap();
        cat.suggest(asset, tag, Source::User, 1.0).unwrap();
        cat.accept(asset, tag).unwrap();
    }
    Setup {
        _tmp: tmp,
        library,
        outside,
        root,
        db,
        cat,
    }
}

impl Setup {
    fn asset(&self, name: &str) -> AssetId {
        self.cat
            .asset_of(&self.library.join(name))
            .unwrap()
            .expect("indexed")
    }

    /// The pending row for moving `library/<name>` to `dst`.
    fn begin(&self, batch: i64, name: &str, dst: &PathBuf) -> i64 {
        let src = self.library.join(name);
        let entry = self
            .entries()
            .into_iter()
            .find(|(path, _)| *path == src)
            .expect("in the gallery");
        let bytes_path = if src.exists() { &src } else { dst };
        let sha = scanner::sha256_file(bytes_path).unwrap();
        self.cat
            .begin_move(&NewMove {
                batch,
                src_path: &src,
                src_root: self.root.as_path(),
                dst_path: dst,
                dst_root: dst.starts_with(&self.library).then(|| self.root.as_path()),
                sha256: &sha,
                size: fs::metadata(bytes_path).unwrap().len(),
                mtime_ns: entry.1,
                method: MoveMethod::Rename,
            })
            .unwrap()
    }

    fn entries(&self) -> Vec<(PathBuf, Option<i64>)> {
        self.cat
            .entries(&Filter::default(), Sort::PathAsc)
            .unwrap()
            .into_iter()
            .map(|e| (e.path, e.modified_ns))
            .collect()
    }

    /// Every row of the log, oldest first.
    fn log(&self) -> Vec<(i64, String)> {
        let db = rusqlite::Connection::open(self.db.as_path()).unwrap();
        let mut stmt = db
            .prepare("SELECT id, state FROM move_log ORDER BY id")
            .unwrap();
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        rows.map(Result::unwrap).collect()
    }

    fn paths(&self) -> Vec<PathBuf> {
        self.entries().into_iter().map(|(p, _)| p).collect()
    }
}

#[test]
fn a_finished_move_inside_a_library_keeps_the_labels_on_the_new_path() {
    let s = setup();
    let asset = s.asset("a.jpg");
    let labels = s.cat.labels_of(asset).unwrap();
    let dst = s.library.join("sorted").join("a.jpg");
    let id = s.begin(1, "a.jpg", &dst);
    fs::rename(s.library.join("a.jpg"), &dst).unwrap();

    s.cat
        .finish_move(id, MoveState::Done, Some(s.root.as_path()))
        .unwrap();

    assert_eq!(s.paths(), vec![s.library.join("b.jpg"), dst.clone()]);
    assert_eq!(s.cat.asset_of(&dst).unwrap(), Some(asset));
    assert_eq!(s.cat.labels_of(asset).unwrap(), labels);
    assert_eq!(
        s.cat.root_of_path(&dst).unwrap(),
        Some(s.root.as_path().to_path_buf())
    );
}

#[test]
fn a_move_out_of_the_libraries_hides_the_photo_but_keeps_its_labels() {
    let s = setup();
    let asset = s.asset("a.jpg");
    let labels = s.cat.labels_of(asset).unwrap();
    assert!(!labels.is_empty());
    let dst = s.outside.join("a.jpg");
    let id = s.begin(1, "a.jpg", &dst);
    fs::rename(s.library.join("a.jpg"), &dst).unwrap();

    s.cat.finish_move(id, MoveState::Done, None).unwrap();

    assert_eq!(s.paths(), vec![s.library.join("b.jpg")]);
    assert_eq!(s.cat.labels_of(asset).unwrap(), labels);
}

#[test]
fn finishing_a_move_survives_an_index_that_already_saw_the_destination() {
    let mut s = setup();
    let asset = s.asset("a.jpg");
    let dst = s.library.join("sorted").join("a.jpg");
    let id = s.begin(1, "a.jpg", &dst);
    fs::rename(s.library.join("a.jpg"), &dst).unwrap();
    // The folder review runs between the move and its log: the destination row already exists.
    s.cat.index(&s.root).unwrap();

    s.cat
        .finish_move(id, MoveState::Done, Some(s.root.as_path()))
        .unwrap();

    assert_eq!(s.paths(), vec![s.library.join("b.jpg"), dst.clone()]);
    assert_eq!(s.cat.asset_of(&dst).unwrap(), Some(asset));
    assert_eq!(s.cat.asset_of(&s.library.join("a.jpg")).unwrap(), None);
}

/// A row left behind by a file that used to be at the destination must not take the moved photo's place.
#[test]
fn finishing_a_move_replaces_a_stale_row_of_other_bytes_at_the_destination() {
    let mut s = setup();
    let asset = s.asset("a.jpg");
    let dst = s.library.join("sorted").join("a.jpg");
    fs::write(&dst, b"something else that was here").unwrap();
    s.cat.index(&s.root).unwrap();
    fs::remove_file(&dst).unwrap();
    s.cat.index(&s.root).unwrap();
    let id = s.begin(1, "a.jpg", &dst);
    fs::rename(s.library.join("a.jpg"), &dst).unwrap();

    s.cat
        .finish_move(id, MoveState::Done, Some(s.root.as_path()))
        .unwrap();

    assert_eq!(s.cat.asset_of(&dst).unwrap(), Some(asset));
    assert_eq!(s.paths(), vec![s.library.join("b.jpg"), dst]);
}

#[test]
fn undo_brings_the_photo_back_to_its_original_path() {
    let s = setup();
    let asset = s.asset("a.jpg");
    let original = s.paths();
    for (batch, dst) in [
        (1, s.library.join("sorted").join("a.jpg")),
        (2, s.outside.join("a.jpg")),
    ] {
        let inside = dst.starts_with(&s.library);
        let id = s.begin(batch, "a.jpg", &dst);
        fs::rename(s.library.join("a.jpg"), &dst).unwrap();
        s.cat
            .finish_move(id, MoveState::Done, inside.then(|| s.root.as_path()))
            .unwrap();

        fs::rename(&dst, s.library.join("a.jpg")).unwrap();
        s.cat.undo_done(id).unwrap();

        assert_eq!(s.paths(), original);
        assert_eq!(s.asset("a.jpg"), asset);
        assert!(s.cat.last_undoable_batch().unwrap().is_none());
    }
}

#[test]
fn pending_moves_are_resolved_when_the_catalog_opens() {
    let s = setup();
    let batch = s.cat.next_move_batch().unwrap();
    // Only the destination is there: the move happened.
    let moved = s.outside.join("a.jpg");
    let moved_id = s.begin(batch, "a.jpg", &moved);
    fs::rename(s.library.join("a.jpg"), &moved).unwrap();
    // Only the origin is there: nothing happened.
    s.begin(batch, "b.jpg", &s.outside.join("b.jpg"));
    // Both are there: cut between the copy and the Recycle Bin.
    let copied = s.outside.join("b copia.jpg");
    let kept_id = s.begin(batch, "b.jpg", &copied);
    fs::copy(s.library.join("b.jpg"), &copied).unwrap();

    let reopened = Catalog::open(&s.db).unwrap();

    let done: Vec<i64> = reopened
        .last_undoable_batch()
        .unwrap()
        .unwrap()
        .iter()
        .map(|row| row.id)
        .collect();
    assert_eq!(done, vec![moved_id]);
    assert_eq!(s.paths(), vec![s.library.join("b.jpg")]);
    assert!(s.library.join("b.jpg").exists() && copied.exists());
    // The discarded row is gone, the other two are settled: a second open changes nothing.
    let settled = vec![
        (moved_id, "done".to_string()),
        (kept_id, "kept_original".to_string()),
    ];
    assert_eq!(s.log(), settled);
    Catalog::open(&s.db).unwrap();
    assert_eq!(s.log(), settled);
}

/// Without both places in sight, or without the very bytes at the destination, nothing is decided.
#[test]
fn a_pending_move_that_cannot_be_told_for_sure_stays_pending() {
    let s = setup();
    let dst = s.outside.join("a.jpg");
    let id = s.begin(1, "a.jpg", &dst);
    fs::rename(s.library.join("a.jpg"), &dst).unwrap();
    let pending = vec![(id, "pending".to_string())];

    // The library is unplugged: everything in it looks gone.
    let unplugged = s.library.with_file_name("unplugged");
    fs::rename(&s.library, &unplugged).unwrap();
    Catalog::open(&s.db).unwrap();
    assert_eq!(s.log(), pending);
    fs::rename(&unplugged, &s.library).unwrap();

    // So is the destination folder, with the original apparently still in its place.
    let away = s.outside.with_file_name("away");
    fs::rename(&s.outside, &away).unwrap();
    fs::copy(away.join("a.jpg"), s.library.join("a.jpg")).unwrap();
    Catalog::open(&s.db).unwrap();
    assert_eq!(s.log(), pending);
    fs::remove_file(s.library.join("a.jpg")).unwrap();
    fs::rename(&away, &s.outside).unwrap();

    // Something else is at the destination.
    fs::write(&dst, b"photo z").unwrap();
    Catalog::open(&s.db).unwrap();
    assert_eq!(s.log(), pending);
    assert_eq!(s.paths().len(), 2);

    fs::write(&dst, b"photo a").unwrap();
    Catalog::open(&s.db).unwrap();
    assert_eq!(s.log(), vec![(id, "done".to_string())]);
}

#[test]
fn a_pending_move_inside_a_library_keeps_the_photo_in_the_gallery() {
    let s = setup();
    let asset = s.asset("a.jpg");
    let dst = s.library.join("sorted").join("a.jpg");
    s.begin(1, "a.jpg", &dst);
    fs::rename(s.library.join("a.jpg"), &dst).unwrap();

    let reopened = Catalog::open(&s.db).unwrap();

    assert_eq!(s.paths(), vec![s.library.join("b.jpg"), dst.clone()]);
    assert_eq!(reopened.asset_of(&dst).unwrap(), Some(asset));
}

/// A log written before the destination's library was kept.
#[test]
fn an_older_move_log_is_upgraded_without_losing_rows() {
    let s = setup();
    let id = s.begin(1, "a.jpg", &s.outside.join("a.jpg"));
    s.cat.finish_move(id, MoveState::Done, None).unwrap();
    rusqlite::Connection::open(s.db.as_path())
        .unwrap()
        .execute_batch("ALTER TABLE move_log DROP COLUMN dst_root;")
        .unwrap();

    let rows = Catalog::open(&s.db)
        .unwrap()
        .last_undoable_batch()
        .unwrap()
        .unwrap();

    assert_eq!((rows[0].id, &rows[0].dst_root), (id, &None));
}

/// Other threads open the catalog while a move runs: its pending rows are not theirs to resolve.
#[test]
fn a_move_in_progress_keeps_its_pending_rows_from_other_connections() {
    let s = setup();
    let lock = s.cat.lock_moves().unwrap();
    let dst = s.outside.join("a.jpg");
    // Mid copy: the origin is there and the destination is not, which would discard the row.
    let id = s.begin(1, "a.jpg", &dst);

    let other = Catalog::open(&s.db).unwrap();
    assert!(other.lock_moves().is_err(), "only one move at a time");
    fs::rename(s.library.join("a.jpg"), &dst).unwrap();
    s.cat.finish_move(id, MoveState::Done, None).unwrap();
    drop(lock);

    let rows: Vec<MoveRow> = other.last_undoable_batch().unwrap().unwrap();
    assert_eq!(rows.len(), 1);
    assert!(other.lock_moves().is_ok());
}

#[test]
fn the_last_undoable_batch_steps_back_one_batch_at_a_time() {
    let s = setup();
    assert!(s.cat.last_undoable_batch().unwrap().is_none());
    assert_eq!(s.cat.next_move_batch().unwrap(), 1);
    let first = s.begin(1, "a.jpg", &s.outside.join("a.jpg"));
    let second = s.begin(2, "b.jpg", &s.outside.join("b.jpg"));
    let third = s.begin(2, "a.jpg", &s.outside.join("a (1).jpg"));
    let kept = s.begin(3, "b.jpg", &s.outside.join("b (1).jpg"));
    for id in [first, second, third] {
        s.cat.finish_move(id, MoveState::Done, None).unwrap();
    }
    s.cat
        .finish_move(kept, MoveState::KeptOriginal, None)
        .unwrap();
    assert_eq!(s.cat.next_move_batch().unwrap(), 4);

    let ids = |cat: &Catalog| -> Vec<i64> {
        cat.last_undoable_batch()
            .unwrap()
            .map(|rows| rows.iter().map(|r| r.id).collect())
            .unwrap_or_default()
    };
    assert_eq!(ids(&s.cat), vec![third, second]);
    s.cat.undo_done(third).unwrap();
    assert_eq!(ids(&s.cat), vec![second]);
    s.cat.undo_done(second).unwrap();
    assert_eq!(ids(&s.cat), vec![first]);
    s.cat.undo_done(first).unwrap();
    assert!(s.cat.last_undoable_batch().unwrap().is_none());
}
