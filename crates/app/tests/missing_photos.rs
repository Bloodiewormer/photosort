// Photos that disappear from their folder. Synthetic fixtures in temp dirs only.
#![allow(clippy::disallowed_methods)]

use app::export_flow::{ExportPhase, ExportWizard, PreviewState};
use app::review_engine::{
    FOLDER_RECHECK_INTERVAL, FolderRecheck, ReviewConfig, ReviewEngine, ReviewEngineEvent,
};
use app::settings::{self, Settings};
use app::thumb_provider::open_thumbnails;
use app::ui::PhotosortApp;
use app::worker::{ThumbPool, ThumbRequest};
use catalog::{Catalog, Entry, Filter, Sort};
use eframe::App as _;
use eframe::egui::{self, Pos2, RawInput, Rect, Vec2};
use export::ExportItem;
use fsguard::{DestPath, LibraryRoot, ensure_writable};
use image::{ImageBuffer, Rgba};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct Library {
    tmp: TempDir,
    root: LibraryRoot,
    data: PathBuf,
}

impl Library {
    /// Two indexed photos, `a.png` and `b.png`.
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        fs::create_dir_all(&lib).unwrap();
        for (name, red) in [("a.png", 10u8), ("b.png", 200)] {
            ImageBuffer::from_pixel(24, 18, Rgba([red, 0, 0, 255]))
                .save(lib.join(name))
                .unwrap();
        }
        let data = tmp.path().join("data");
        fs::create_dir_all(&data).unwrap();
        let root = LibraryRoot::new(&lib).unwrap();
        let library = Self { tmp, root, data };
        library.catalog().index(&library.root).unwrap();
        library
    }

    fn db(&self) -> DestPath {
        ensure_writable(
            &self.data.join("catalog.sqlite"),
            std::slice::from_ref(&self.root),
        )
        .unwrap()
    }

    fn catalog(&self) -> Catalog {
        Catalog::open(&self.db()).unwrap()
    }

    fn entries(&self) -> Vec<Entry> {
        self.catalog()
            .entries(&Filter::default(), Sort::PathAsc)
            .unwrap()
    }
}

fn names(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn a_cached_thumbnail_of_a_deleted_file_still_reports_it_gone() {
    let library = Library::new();
    let (provider, warning) = open_thumbnails(&library.data, std::slice::from_ref(&library.root));
    assert!(warning.is_none());
    let entry = library.entries().remove(0);
    let request = ThumbRequest {
        source: entry.path.clone(),
        sha256: entry.sha256.clone(),
    };
    let pool = ThumbPool::new(1, provider, None);
    pool.set_visible(&HashSet::from([entry.sha256.clone()]));

    pool.request(request.clone());
    let first = pool.recv().unwrap();
    assert!(first.image.is_ok() && first.gone.is_none());

    fs::remove_file(&entry.path).unwrap();
    pool.request(request);
    let second = pool.recv().unwrap();
    assert!(second.image.is_ok(), "the cache still answers");
    let gone = second.gone.expect("gone");

    assert!(library.catalog().mark_location_missing(&gone).unwrap());
    assert_eq!(names(&library.entries()), ["b.png"]);
}

/// The whole path inside the application: gallery, thumbnail thread, catalog.
#[test]
fn the_application_flags_a_photo_deleted_while_it_is_open() {
    let library = Library::new();
    let settings = Settings {
        library_roots: vec![library.root.as_path().to_path_buf()],
        ..Settings::default()
    };
    settings::save(&library.data, &settings).unwrap();
    fs::remove_file(library.root.as_path().join("a.png")).unwrap();

    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = PhotosortApp::new_with_mode(&cc, library.data.clone(), true, open_thumbnails);
    let mut frame = eframe::Frame::_new_kittest();
    let deadline = Instant::now() + Duration::from_secs(20);
    while library.entries().len() != 1 {
        assert!(Instant::now() < deadline, "the photo was never flagged");
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 820.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| app.ui(ui, &mut frame));
        out.textures_delta.clear();
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(names(&library.entries()), ["b.png"]);
    assert_eq!(library.catalog().counts().unwrap().missing, 1);
}

#[cfg(windows)]
#[test]
fn a_locked_file_is_not_flagged_and_neither_is_one_whose_library_is_gone() {
    use std::os::windows::fs::OpenOptionsExt;
    let library = Library::new();
    let path = library.entries()[0].path.clone();
    let cat = library.catalog();
    {
        let _lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(fs::File::open(&path).is_err());
        assert!(!cat.mark_location_missing(&path).unwrap());
    }
    // An unplugged drive: the whole library is out of reach.
    fs::rename(library.root.as_path(), library.tmp.path().join("away")).unwrap();
    assert!(!cat.mark_location_missing(&path).unwrap());
    assert_eq!(cat.counts().unwrap().missing, 0);
}

#[test]
fn a_missing_photo_gets_everything_back_when_it_returns_or_turns_up_moved() {
    let library = Library::new();
    let mut cat = library.catalog();
    let entry = library.entries().remove(0);
    let tag = cat.create_tag("favorita").unwrap();
    let other = cat.create_tag("descartada").unwrap();
    cat.accept(entry.asset, tag).unwrap();
    cat.reject(entry.asset, other).unwrap();
    let labels = cat.labels_of(entry.asset).unwrap();
    let away = library.tmp.path().join("a.png");

    // Out of the library and back to the same path.
    fs::rename(&entry.path, &away).unwrap();
    assert!(cat.mark_location_missing(&entry.path).unwrap());
    fs::rename(&away, &entry.path).unwrap();
    assert_eq!(cat.index(&library.root).unwrap().restored, 1);
    assert_eq!(names(&library.entries()), ["a.png", "b.png"]);
    assert_eq!(cat.labels_of(entry.asset).unwrap(), labels);

    // Moved to another folder inside the library.
    let folder = library.root.as_path().join("sub");
    fs::create_dir(&folder).unwrap();
    fs::rename(&entry.path, folder.join("moved.png")).unwrap();
    assert_eq!(cat.index(&library.root).unwrap().missing, 1);
    let moved = library
        .entries()
        .into_iter()
        .find(|e| e.path.ends_with("moved.png"))
        .expect("listed at its new path");
    assert_eq!(moved.asset, entry.asset);
    assert_eq!(library.entries().len(), 2);
    assert_eq!(cat.labels_of(moved.asset).unwrap(), labels);
}

#[test]
fn an_automatic_check_finds_a_deleted_file_with_background_review_off() {
    let library = Library::new();
    let engine = ReviewEngine::start(
        library.db(),
        ReviewConfig {
            roots: vec![library.root.as_path().to_path_buf()],
            ..ReviewConfig::default()
        },
        None,
    );
    fs::remove_file(library.root.as_path().join("a.png")).unwrap();
    engine.request_index();
    engine.request_index();

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < deadline, "no index finished");
        if let Ok(ReviewEngineEvent::IndexFinished { missing, .. }) =
            engine.recv_timeout(Duration::from_millis(50))
        {
            assert_eq!(missing, 1);
            break;
        }
    }
    assert_eq!(names(&library.entries()), ["b.png"]);
}

#[test]
fn the_review_engine_flags_a_deleted_file_instead_of_counting_a_failure() {
    let library = Library::new();
    let asset = library.entries()[0].asset;
    fs::remove_file(library.root.as_path().join("a.png")).unwrap();
    let engine = ReviewEngine::start(
        library.db(),
        ReviewConfig {
            roots: vec![library.root.as_path().to_path_buf()],
            ..ReviewConfig::default()
        },
        None,
    );
    engine.prioritize(vec![asset]);

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < deadline, "the chunk never finished");
        match engine.recv_timeout(Duration::from_millis(50)) {
            Ok(ReviewEngineEvent::Status(status)) => assert_eq!(status.failed_count, 0),
            Ok(ReviewEngineEvent::ChunkFinished) => break,
            _ => {}
        }
    }
    assert_eq!(names(&library.entries()), ["b.png"]);
}

#[test]
fn the_export_preview_reports_the_sources_that_are_gone() {
    let library = Library::new();
    let items: Vec<ExportItem> = library
        .entries()
        .into_iter()
        .map(|e| ExportItem::new(e.path))
        .collect();
    let deleted = items[0].source.clone();
    fs::remove_file(&deleted).unwrap();
    let mut wizard = ExportWizard::from_items(items, false, Some(library.tmp.path().join("out")));
    wizard
        .request_preview(std::slice::from_ref(&library.root), None)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !matches!(wizard.phase(), ExportPhase::Preview(PreviewState::Ready(_))) {
        assert!(Instant::now() < deadline, "no preview");
        wizard.poll();
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(wizard.take_gone(), std::slice::from_ref(&deleted));

    // The export itself reports it too.
    wizard.start_export(None).unwrap();
    while !matches!(wizard.phase(), ExportPhase::Done(_)) {
        assert!(Instant::now() < deadline, "no export");
        wizard.poll();
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(wizard.take_gone(), [deleted]);
    assert!(!Path::new(&library.root.as_path().join("a.png")).exists());
}

#[test]
fn folders_are_checked_on_regaining_focus_and_every_interval() {
    let start = Instant::now();
    let mut recheck = FolderRecheck::new(start);
    let second = Duration::from_secs(1);
    assert!(!recheck.due(true, start), "startup is not a focus change");
    assert!(!recheck.due(false, start + second));
    assert!(recheck.due(true, start + 2 * second));
    assert!(!recheck.due(true, start + 3 * second));
    let later = start + 2 * second + FOLDER_RECHECK_INTERVAL;
    assert!(!recheck.due(true, later - second));
    assert!(recheck.due(false, later), "unfocused windows check too");
    assert_eq!(recheck.remaining(later), FOLDER_RECHECK_INTERVAL);
}
