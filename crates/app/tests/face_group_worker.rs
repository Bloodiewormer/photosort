// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::face_group_worker::{FaceGroupEvent, FaceGroupWorker};
use app::worker::{IndexEvent, IndexWorker};
use catalog::{AssetId, Catalog, DetectedFace, EMBEDDING_DIM, SIGLIP2_MODEL_ID};
use fsguard::{DestPath, LibraryRoot, ensure_writable};
use image::{ImageBuffer, Rgba};
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::SystemTime;

struct Library {
    _tmp: tempfile::TempDir,
    root: LibraryRoot,
    db: PathBuf,
    files: Vec<PathBuf>,
    assets: Vec<AssetId>,
}

impl Library {
    fn db(&self) -> DestPath {
        ensure_writable(&self.db, std::slice::from_ref(&self.root)).unwrap()
    }

    fn originals(&self) -> Vec<(Vec<u8>, SystemTime)> {
        self.files
            .iter()
            .map(|p| {
                let modified = fs::metadata(p).unwrap().modified().unwrap();
                (fs::read(p).unwrap(), modified)
            })
            .collect()
    }

    fn grouped(&self) -> Vec<bool> {
        let cat = Catalog::open(&self.db()).unwrap();
        self.assets
            .iter()
            .map(|a| cat.asset_face_details(*a).unwrap()[0].person_id.is_some())
            .collect()
    }
}

/// Three indexed photos with one clear face each; the one at `broken` stops being decodable.
fn library(broken: Option<usize>) -> Library {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("library");
    fs::create_dir_all(&dir).unwrap();
    let files: Vec<PathBuf> = (0..3).map(|i| dir.join(format!("{i}.png"))).collect();
    for (i, path) in files.iter().enumerate() {
        if broken == Some(i) {
            fs::write(path, b"not an image").unwrap();
        } else {
            ImageBuffer::from_pixel(16, 12, Rgba([i as u8 * 80, 100, 100, 255]))
                .save(path)
                .unwrap();
        }
    }

    let root = LibraryRoot::new(&dir).unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db = data.join("catalog.sqlite");

    let mut lib = Library {
        _tmp: tmp,
        root,
        db,
        files,
        assets: Vec::new(),
    };
    let indexer = IndexWorker::start(lib.db(), vec![LibraryRoot::new(&dir).unwrap()], None);
    while let Ok(ev) = indexer.recv() {
        if let IndexEvent::Finished { .. } = ev {
            break;
        }
    }

    let mut cat = Catalog::open(&lib.db()).unwrap();
    for path in &lib.files {
        let asset = cat
            .asset_of(&dunce::canonicalize(path).unwrap())
            .unwrap()
            .unwrap();
        cat.save_vision_result(
            asset,
            SIGLIP2_MODEL_ID,
            &vec![0.0; EMBEDDING_DIM],
            &[DetectedFace::new([10.0, 10.0, 100.0, 100.0], 0.95)],
        )
        .unwrap();
        lib.assets.push(asset);
    }
    lib
}

fn embedding() -> [f32; 128] {
    let mut e = [0.0; 128];
    e[0] = 1.0;
    e
}

#[test]
fn groups_faces_reporting_progress_and_skips_unreadable_photos() {
    let lib = library(Some(1));
    let before = lib.originals();

    let worker = FaceGroupWorker::start_with_loader(
        lib.db(),
        || Ok(|_: &image::RgbImage, _: &[[f32; 2]; 5]| Ok(embedding())),
        None,
    );
    let mut events = Vec::new();
    while let Ok(ev) = worker.recv() {
        events.push(ev);
    }

    let mut expected: Vec<FaceGroupEvent> = (0..=3)
        .map(|done| FaceGroupEvent::Progress { done, total: 3 })
        .collect();
    expected.push(FaceGroupEvent::Finished {
        stopped: false,
        total: 3,
        grouped_faces: 2,
        grouped_photos: 2,
        unreadable_photos: 1,
    });
    assert_eq!(events, expected);
    assert_eq!(lib.grouped(), [true, false, true]);
    assert_eq!(before, lib.originals());
}

#[test]
fn stops_after_the_current_photo_when_cancelled() {
    let lib = library(None);
    let (started_tx, started_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel::<()>();

    let worker = FaceGroupWorker::start_with_loader(
        lib.db(),
        move || {
            Ok(move |_: &image::RgbImage, _: &[[f32; 2]; 5]| {
                started_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
                Ok(embedding())
            })
        },
        None,
    );
    started_rx.recv().unwrap();
    worker.cancel();
    resume_tx.send(()).unwrap();

    let mut last = None;
    while let Ok(ev) = worker.recv() {
        last = Some(ev);
    }
    assert_eq!(
        last,
        Some(FaceGroupEvent::Finished {
            stopped: true,
            total: 3,
            grouped_faces: 1,
            grouped_photos: 1,
            unreadable_photos: 0,
        })
    );
    assert_eq!(lib.grouped(), [true, false, false]);
}
