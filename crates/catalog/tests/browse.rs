// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{
    AssetId, Catalog, Control, Entry, Filter, LabelCount, LabelKind, Progress, Sort, Source,
};
use fsguard::{DestPath, LibraryRoot, ensure_writable};
use std::fs::{self, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

struct Setup {
    _tmp: TempDir,
    library: PathBuf,
    root: LibraryRoot,
    db: DestPath,
    cat: Catalog,
}

fn write(root: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, bytes).unwrap();
    p
}

fn set_mtime(path: &Path, secs: u64) {
    let f = fs::File::options().write(true).open(path).unwrap();
    let times = FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(secs));
    f.set_times(times).unwrap();
}

/// A library with three images of distinct sizes and modified times, already indexed.
fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let p1 = write(&library, "memes/funny.jpg", b"one"); // 3 bytes
    let p2 = write(&library, "shots/chat.png", b"two-long"); // 8 bytes
    let p3 = write(&library, "manga/page.webp", b"three"); // 5 bytes
    set_mtime(&p1, 1_700_000_200);
    set_mtime(&p2, 1_700_000_100);
    set_mtime(&p3, 1_700_000_300);

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
        db,
        cat,
    }
}

impl Setup {
    fn asset(&self, rel: &str) -> AssetId {
        let path = dunce::canonicalize(self.library.join(rel)).unwrap();
        self.cat.asset_of(&path).unwrap().expect("indexed")
    }

    fn rel_entries(&self, entries: &[Entry]) -> Vec<String> {
        let base = dunce::canonicalize(&self.library).unwrap();
        entries
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
fn entries_returns_full_metadata_and_matches_filter_semantics() {
    let mut s = setup();
    // Duplicate of funny.jpg: entries must return one row per non-missing location.
    write(&s.library, "memes/funny_copy.jpg", b"one");
    s.cat.index(&s.root).unwrap();

    let meme = s.cat.label(LabelKind::Tag, "meme").unwrap();
    let shot = s.cat.label(LabelKind::Tag, "screenshot").unwrap();
    let anime = s.cat.label(LabelKind::Category, "anime").unwrap();

    let funny = s.asset("memes/funny.jpg");
    let chat = s.asset("shots/chat.png");
    let page = s.asset("manga/page.webp");

    s.cat.suggest(funny, meme, Source::Ai, 0.9).unwrap();
    s.cat.suggest(chat, shot, Source::Ai, 0.8).unwrap();
    s.cat.reject(chat, meme).unwrap();
    s.cat.accept(page, anime).unwrap();

    let all = s.cat.entries(&Filter::default(), Sort::PathAsc).unwrap();
    assert_eq!(
        s.rel_entries(&all),
        [
            "manga/page.webp",
            "memes/funny.jpg",
            "memes/funny_copy.jpg",
            "shots/chat.png",
        ]
    );
    let funny_entry = &all[1];
    assert_eq!(funny_entry.asset, funny);
    assert_eq!(funny_entry.size, 3);
    assert_eq!(
        funny_entry.sha256,
        scanner::sha256_file(&s.library.join("memes/funny.jpg")).unwrap()
    );
    assert_eq!(funny_entry.modified_ns, Some(1_700_000_200_000_000_000));

    // Rejected labels never match; duplicates share asset labels so both locations match.
    let memes = s
        .cat
        .entries(
            &Filter {
                require: vec![meme],
                ..Default::default()
            },
            Sort::PathAsc,
        )
        .unwrap();
    assert_eq!(
        s.rel_entries(&memes),
        ["memes/funny.jpg", "memes/funny_copy.jpg"]
    );

    // Combining any + exclude + path_contains.
    let filtered = s
        .cat
        .entries(
            &Filter {
                any: vec![meme, shot, anime],
                exclude: vec![meme],
                path_contains: Some("CHAT".into()),
                ..Default::default()
            },
            Sort::PathAsc,
        )
        .unwrap();
    assert_eq!(s.rel_entries(&filtered), ["shots/chat.png"]);

    // Missing locations are never returned.
    fs::remove_file(s.library.join("memes/funny_copy.jpg")).unwrap();
    s.cat.index(&s.root).unwrap();
    let memes_after = s
        .cat
        .entries(
            &Filter {
                require: vec![meme],
                ..Default::default()
            },
            Sort::PathAsc,
        )
        .unwrap();
    assert_eq!(s.rel_entries(&memes_after), ["memes/funny.jpg"]);
}

#[test]
fn entries_sorts_in_every_order_and_breaks_ties_by_path() {
    let mut s = setup();
    // Same size (3 bytes) and same mtime (1_700_000_200) as memes/funny.jpg to test tie-breaking.
    let tie = write(&s.library, "aaa/tie.jpg", b"xyz");
    set_mtime(&tie, 1_700_000_200);
    s.cat.index(&s.root).unwrap();

    let order = |sort: Sort| s.rel_entries(&s.cat.entries(&Filter::default(), sort).unwrap());

    assert_eq!(
        order(Sort::PathAsc),
        [
            "aaa/tie.jpg",
            "manga/page.webp",
            "memes/funny.jpg",
            "shots/chat.png",
        ]
    );
    assert_eq!(
        order(Sort::PathDesc),
        [
            "shots/chat.png",
            "memes/funny.jpg",
            "manga/page.webp",
            "aaa/tie.jpg",
        ]
    );
    // Sizes: tie=3, funny=3, page=5, chat=8. Ties broken by path ascending.
    assert_eq!(
        order(Sort::SizeAsc),
        [
            "aaa/tie.jpg",
            "memes/funny.jpg",
            "manga/page.webp",
            "shots/chat.png",
        ]
    );
    assert_eq!(
        order(Sort::SizeDesc),
        [
            "shots/chat.png",
            "manga/page.webp",
            "aaa/tie.jpg",
            "memes/funny.jpg",
        ]
    );
    // Mtimes: chat=100, tie=200, funny=200, page=300. Ties broken by path ascending.
    assert_eq!(
        order(Sort::ModifiedAsc),
        [
            "shots/chat.png",
            "aaa/tie.jpg",
            "memes/funny.jpg",
            "manga/page.webp",
        ]
    );
    assert_eq!(
        order(Sort::ModifiedDesc),
        [
            "manga/page.webp",
            "aaa/tie.jpg",
            "memes/funny.jpg",
            "shots/chat.png",
        ]
    );
}

#[test]
fn labels_in_use_counts_distinct_non_missing_assets_ordered_by_kind_and_name() {
    let mut s = setup();
    // Duplicate of funny.jpg: must still count as 1 distinct asset.
    write(&s.library, "memes/funny_dup.jpg", b"one");
    s.cat.index(&s.root).unwrap();

    let funny = s.asset("memes/funny.jpg");
    let chat = s.asset("shots/chat.png");
    let page = s.asset("manga/page.webp");

    let cat_manga = s.cat.label(LabelKind::Category, "Manga").unwrap();
    let cat_anime = s.cat.label(LabelKind::Category, "anime").unwrap();
    let ent_work = s.cat.label(LabelKind::Entity, "Obra A").unwrap();
    let tag_meme = s.cat.label(LabelKind::Tag, "meme").unwrap();
    let tag_unused = s.cat.label(LabelKind::Tag, "unused").unwrap();
    let tag_rejected_only = s.cat.label(LabelKind::Tag, "only-rejected").unwrap();
    let tag_missing_only = s.cat.label(LabelKind::Tag, "only-missing").unwrap();
    let _ = tag_unused;

    s.cat.accept(funny, cat_anime).unwrap();
    s.cat.suggest(page, cat_manga, Source::Ai, 0.8).unwrap();
    s.cat.accept(page, ent_work).unwrap();
    s.cat.suggest(funny, tag_meme, Source::Ai, 0.9).unwrap();
    s.cat.accept(chat, tag_meme).unwrap();
    s.cat.reject(page, tag_meme).unwrap(); // rejected does not count
    s.cat.reject(funny, tag_rejected_only).unwrap();
    s.cat.accept(chat, tag_missing_only).unwrap();

    // Remove chat.png and reindex so it becomes missing.
    fs::remove_file(s.library.join("shots/chat.png")).unwrap();
    s.cat.index(&s.root).unwrap();

    let in_use = s.cat.labels_in_use().unwrap();
    assert_eq!(
        in_use,
        vec![
            LabelCount {
                label: cat_anime,
                kind: LabelKind::Category,
                name: "Anime".into(),
                images: 1,
            },
            LabelCount {
                label: cat_manga,
                kind: LabelKind::Category,
                name: "Manga".into(),
                images: 1,
            },
            LabelCount {
                label: ent_work,
                kind: LabelKind::Entity,
                name: "Obra A".into(),
                images: 1,
            },
            LabelCount {
                label: tag_meme,
                kind: LabelKind::Tag,
                name: "meme".into(),
                images: 1, // funny (with 2 locations) counts once; chat is missing; page is rejected
            },
        ]
    );
}

#[test]
fn entry_for_returns_entry_for_known_asset_and_none_for_unknown_or_missing() {
    let mut s = setup();
    let chat = s.asset("shots/chat.png");
    let entry = s.cat.entry_for(chat).unwrap().expect("present");
    assert_eq!(entry.asset, chat);
    assert_eq!(entry.size, 8);
    assert!(entry.path.ends_with("chat.png"));
    assert_eq!(
        entry.sha256,
        scanner::sha256_file(&s.library.join("shots/chat.png")).unwrap()
    );

    assert_eq!(s.cat.entry_for(AssetId(999_999)).unwrap(), None);

    fs::remove_file(s.library.join("shots/chat.png")).unwrap();
    s.cat.index(&s.root).unwrap();
    assert_eq!(s.cat.entry_for(chat).unwrap(), None);
}

#[test]
fn index_with_reports_monotonic_progress_ending_at_total() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    write(&library, "a.jpg", b"a");
    write(&library, "b.png", b"bb");
    write(&library, "c.webp", b"ccc");
    let root = LibraryRoot::new(&library).unwrap();
    fs::create_dir_all(tmp.path().join("data")).unwrap();
    let db = ensure_writable(
        &tmp.path().join("data").join("catalog.sqlite"),
        std::slice::from_ref(&root),
    )
    .unwrap();
    let mut cat = Catalog::open(&db).unwrap();

    let mut seen: Vec<(usize, usize)> = Vec::new();
    let report = cat
        .index_with(&root, &mut |p: &Progress| {
            seen.push((p.done, p.total));
            Control::Continue
        })
        .unwrap();
    assert!(!report.stopped);
    assert_eq!(report.new, 3);
    assert!(!seen.is_empty());
    assert!(seen.iter().all(|&(_, total)| total == 3));
    assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0));
    assert_eq!(seen.last().unwrap().0, 3);

    // Empty root also ends cleanly at total == 0.
    let empty_dir = tmp.path().join("empty");
    fs::create_dir_all(&empty_dir).unwrap();
    let empty_root = LibraryRoot::new(&empty_dir).unwrap();
    let mut empty_seen = Vec::new();
    let empty_report = cat
        .index_with(&empty_root, &mut |p| {
            empty_seen.push((p.done, p.total));
            Control::Continue
        })
        .unwrap();
    assert!(!empty_report.stopped);
    assert_eq!(empty_seen.last(), Some(&(0, 0)));
}

#[test]
fn index_with_stop_keeps_committed_work_flags_nothing_missing_and_resumes() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    write(&library, "a.jpg", b"a");
    write(&library, "b.jpg", b"bb");
    write(&library, "c.jpg", b"ccc");
    write(&library, "d.jpg", b"dddd");
    let root = LibraryRoot::new(&library).unwrap();
    fs::create_dir_all(tmp.path().join("data")).unwrap();
    let db = ensure_writable(
        &tmp.path().join("data").join("catalog.sqlite"),
        std::slice::from_ref(&root),
    )
    .unwrap();
    let mut cat = Catalog::open(&db).unwrap();

    // Stop after 2 files: those 2 stay committed, nothing flagged missing.
    let stopped = cat
        .index_with(&root, &mut |p| {
            if p.done >= 2 {
                Control::Stop
            } else {
                Control::Continue
            }
        })
        .unwrap();
    assert!(stopped.stopped);
    assert_eq!(stopped.new, 2);
    assert_eq!(stopped.missing, 0);
    assert_eq!(cat.counts().unwrap().locations, 2);

    // A following full index finishes the remaining 2 without rehashing the first 2.
    let resumed = cat.index(&root).unwrap();
    assert!(!resumed.stopped);
    assert_eq!((resumed.new, resumed.unchanged, resumed.hashed), (2, 2, 2));
    assert_eq!(cat.counts().unwrap().locations, 4);

    // Now delete one file on disk, but stop partway through re-indexing: nothing may be flagged missing.
    fs::remove_file(library.join("d.jpg")).unwrap();
    let partial = cat
        .index_with(&root, &mut |p| {
            if p.done >= 1 {
                Control::Stop
            } else {
                Control::Continue
            }
        })
        .unwrap();
    assert!(partial.stopped);
    assert_eq!(partial.missing, 0);
    assert_eq!(cat.counts().unwrap().missing, 0);

    // A full index completes the walk and flags the deleted file missing.
    let full = cat.index(&root).unwrap();
    assert!(!full.stopped);
    assert_eq!(full.missing, 1);
    assert_eq!(cat.counts().unwrap().missing, 1);
}

#[test]
fn two_connections_can_share_the_wal_database() {
    let s = setup();
    let reader = Catalog::open(&s.db).unwrap();
    assert_eq!(reader.counts().unwrap().locations, 3);

    // Concurrent indexing on a second connection while reading on the first succeeds cleanly.
    let lib_path = s.library.clone();
    write(&lib_path, "extra/e.png", b"extra");
    let db2 = ensure_writable(s.db.as_path(), std::slice::from_ref(&s.root)).unwrap();
    let root2 = LibraryRoot::new(&lib_path).unwrap();
    let handle = std::thread::spawn(move || {
        let mut writer = Catalog::open(&db2).unwrap();
        writer.index(&root2).unwrap()
    });
    for _ in 0..10 {
        let _ = reader.entries(&Filter::default(), Sort::PathAsc).unwrap();
    }
    let report = handle.join().unwrap();
    assert_eq!(report.new, 1);
    assert_eq!(reader.counts().unwrap().locations, 4);
}

#[test]
fn metadata_sort_filters_and_legacy_migration_preserve_decisions() {
    use catalog::{LabelKind, Orientation, Status};

    // 1. Legacy catalog without asset metadata columns migrates without losing labels/decisions.
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
    assert_eq!(assigns[0].status, Status::Accepted);
    migrated
        .set_asset_metadata(
            catalog::AssetId(1),
            Some(1920),
            Some(1080),
            Some("JPEG"),
            Some(1_700_000_000_000_000_000),
            Some(2023),
        )
        .unwrap();

    // 2. Capture date sort (with mtime fallback), pixel sort, and format/orientation/year filters.
    let s = setup();
    let a_manga = s.asset("manga/page.webp"); // mtime = 100
    let a_funny = s.asset("memes/funny.jpg"); // mtime = 200
    let a_chat = s.asset("shots/chat.png"); // mtime = 300

    // page.webp: portrait 600x1000 (600k px), WEBP, captured_at = 250s (between funny=200 and chat=300)
    s.cat
        .set_asset_metadata(
            a_manga,
            Some(600),
            Some(1000),
            Some("WEBP"),
            Some(1_700_000_250_000_000_000),
            Some(2023),
        )
        .unwrap();
    // funny.jpg: square 500x500 (250k px), JPEG, no EXIF capture date -> falls back to mtime (200s)
    s.cat
        .set_asset_metadata(a_funny, Some(500), Some(500), Some("JPEG"), None, None)
        .unwrap();
    // chat.png: landscape 1920x1080 (2.07M px), PNG, captured_at = 50s (earliest!)
    s.cat
        .set_asset_metadata(
            a_chat,
            Some(1920),
            Some(1080),
            Some("PNG"),
            Some(1_700_000_050_000_000_000),
            Some(2021),
        )
        .unwrap();

    let order = |sort: Sort| s.rel_entries(&s.cat.entries(&Filter::default(), sort).unwrap());
    assert_eq!(
        order(Sort::CapturedAsc),
        ["shots/chat.png", "memes/funny.jpg", "manga/page.webp"]
    );
    assert_eq!(
        order(Sort::CapturedDesc),
        ["manga/page.webp", "memes/funny.jpg", "shots/chat.png"]
    );
    assert_eq!(
        order(Sort::PixelsDesc),
        ["shots/chat.png", "manga/page.webp", "memes/funny.jpg"]
    );
    assert_eq!(
        order(Sort::PixelsAsc),
        ["memes/funny.jpg", "manga/page.webp", "shots/chat.png"]
    );

    let by_fmt = s
        .cat
        .entries(
            &Filter {
                formats: vec!["JPEG".into(), "WEBP".into()],
                ..Default::default()
            },
            Sort::PathAsc,
        )
        .unwrap();
    assert_eq!(
        s.rel_entries(&by_fmt),
        ["manga/page.webp", "memes/funny.jpg"]
    );

    let by_orient = s
        .cat
        .entries(
            &Filter {
                orientations: vec![Orientation::Landscape],
                ..Default::default()
            },
            Sort::PathAsc,
        )
        .unwrap();
    assert_eq!(s.rel_entries(&by_orient), ["shots/chat.png"]);

    let by_year = s
        .cat
        .entries(
            &Filter {
                capture_years: vec![2023],
                ..Default::default()
            },
            Sort::PathAsc,
        )
        .unwrap();
    assert_eq!(s.rel_entries(&by_year), ["manga/page.webp"]);

    let facets = s
        .cat
        .metadata_facets_in_roots(&[s.root.as_path().to_path_buf()])
        .unwrap();
    assert_eq!(facets.formats.len(), 3);
    assert_eq!(facets.orientations.len(), 3);
    assert_eq!(facets.years.len(), 2);
    let _ = LabelKind::Tag;
}
