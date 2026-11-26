// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::analysis_worker::{AnalysisEvent, AnalysisWorker};
use app::ocr_worker::{OcrEvent, OcrImageReader, OcrScope, OcrWorker};
use app::settings::OcrDevice;
use app::vision_worker::{
    TagsApplyEvent, TagsApplyWorker, VisionEvent, VisionImageOutput, VisionReader, VisionScope,
    VisionWorker,
};
use app::worker::{IndexEvent, IndexWorker, PreviewWorker};
use app::works_worker::{WorksEvent, WorksWorker};
use catalog::{Catalog, EMBEDDING_DIM, OcrLine, OcrRecognizer};
use fsguard::{LibraryRoot, ensure_writable};
use image::{ImageBuffer, Rgba};
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

fn write_png(path: &Path, color: [u8; 4]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = ImageBuffer::from_pixel(16, 12, Rgba(color));
    img.save(path).unwrap();
}

#[test]
fn worker_indexing_and_preview_leave_the_originals_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let a = library.join("a.png");
    let b = library.join("sub/b.png");
    let c = library.join("sub/deep/c.png");
    write_png(&a, [255, 0, 0, 255]);
    write_png(&b, [0, 255, 0, 255]);
    write_png(&c, [0, 0, 255, 255]);
    fs::write(library.join("sub/notes.txt"), b"text").unwrap();

    let root = LibraryRoot::new(&library).unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db = ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();

    // NTFS refreshes a directory's modified time lazily the first time it is listed from its parent,
    // so one throwaway read settles it. Without this the test compares a stale time with a fresh one.
    snapshot(&library);
    let before = snapshot(&library);

    let worker = IndexWorker::start(db, vec![root], None);
    while let Ok(ev) = worker.recv() {
        match ev {
            IndexEvent::Finished { stopped, new, .. } => {
                assert!(!stopped);
                assert_eq!(new, 3);
                break;
            }
            IndexEvent::Failed(e) => panic!("indexing failed: {e}"),
            _ => {}
        }
    }

    // Also exercise read-only original decoding via the inspector preview worker.
    let preview = PreviewWorker::new(8, None);
    preview.request(a.clone());
    let loaded = preview.recv().unwrap();
    assert!(loaded.image.is_ok());

    assert_eq!(before, snapshot(&library));
}

#[test]
fn analysis_worker_leaves_the_originals_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    write_png(&library.join("Screenshot_01.png"), [255, 0, 0, 255]);
    write_png(
        &library.join("sub/IMG-20240501-WA0001.png"),
        [0, 255, 0, 255],
    );
    write_png(&library.join("sub/deep/plain.png"), [0, 0, 255, 255]);
    fs::write(library.join("sub/corrupt.jpg"), b"not an image").unwrap();

    let root = LibraryRoot::new(&library).unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_index =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let db_analyze =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();

    let indexer = IndexWorker::start(db_index, vec![root], None);
    while let Ok(ev) = indexer.recv() {
        match ev {
            IndexEvent::Finished { stopped, .. } => {
                assert!(!stopped);
                break;
            }
            IndexEvent::Failed(e) => panic!("indexing failed: {e}"),
            _ => {}
        }
    }

    // Settle NTFS lazy directory mtime before capturing the baseline snapshot.
    snapshot(&library);
    let before = snapshot(&library);

    let analyzer = AnalysisWorker::start(db_analyze, None);
    let mut finished = false;
    while let Ok(ev) = analyzer.recv() {
        match ev {
            AnalysisEvent::Finished {
                stopped,
                analyzed,
                errors,
                ..
            } => {
                assert!(!stopped);
                assert_eq!(analyzed, 3);
                assert_eq!(errors, 1);
                finished = true;
                break;
            }
            AnalysisEvent::Failed(e) => panic!("analysis failed: {e}"),
            AnalysisEvent::Progress { .. } => {}
        }
    }
    assert!(finished);
    assert_eq!(before, snapshot(&library));
}

#[test]
fn ocr_worker_leaves_the_originals_untouched() {
    struct DecodeOnlyReader;

    impl OcrImageReader for DecodeOnlyReader {
        fn read_image(&self, path: &Path) -> Result<Vec<OcrLine>, String> {
            let pixels = thumbs::decode(path, u32::MAX)
                .map_err(|_| "decode failed".to_string())?
                .to_rgb8();
            Ok(vec![OcrLine {
                text: format!("{}x{}", pixels.width(), pixels.height()),
                score: 0.95,
                recognizer: OcrRecognizer::V6,
                rect: [0.0, 0.0, 10.0, 10.0],
                top_strip: false,
            }])
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    write_png(&library.join("a.png"), [255, 0, 0, 255]);
    write_png(&library.join("sub/b.png"), [0, 255, 0, 255]);
    write_png(&library.join("sub/deep/c.png"), [0, 0, 255, 255]);
    fs::write(library.join("sub/corrupt.jpg"), b"not an image").unwrap();

    let root = LibraryRoot::new(&library).unwrap();
    let root_path = root.as_path().to_path_buf();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_index =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let db_ocr =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();

    let indexer = IndexWorker::start(db_index, vec![root], None);
    while let Ok(ev) = indexer.recv() {
        match ev {
            IndexEvent::Finished { stopped, .. } => {
                assert!(!stopped);
                break;
            }
            IndexEvent::Failed(e) => panic!("indexing failed: {e}"),
            _ => {}
        }
    }

    // Settle NTFS lazy directory mtime before capturing the baseline snapshot.
    snapshot(&library);
    let before = snapshot(&library);

    let worker = OcrWorker::start_with_loader(
        db_ocr,
        OcrScope::Roots(vec![root_path]),
        || Ok(DecodeOnlyReader),
        None,
    );
    let mut finished = false;
    while let Ok(ev) = worker.recv() {
        match ev {
            OcrEvent::Finished {
                stopped,
                processed,
                errors,
            } => {
                assert!(!stopped);
                assert_eq!(processed, 3);
                assert_eq!(errors, 1);
                finished = true;
                break;
            }
            OcrEvent::Failed(e) => panic!("ocr failed: {e}"),
            _ => {}
        }
    }
    assert!(finished);
    assert_eq!(before, snapshot(&library));
}

#[test]
fn works_worker_leaves_the_originals_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let img_path = library.join("promo.png");
    write_png(&img_path, [120, 40, 200, 255]);

    let root = LibraryRoot::new(&library).unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("catalog.sqlite");

    let db_index = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_setup = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_works = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();

    let indexer = IndexWorker::start(db_index, vec![root], None);
    while let Ok(ev) = indexer.recv() {
        if let IndexEvent::Finished { stopped, .. } = ev {
            assert!(!stopped);
            break;
        }
    }

    let mut cat = Catalog::open(&db_setup).unwrap();
    cat.create_work("Academia Nebulosa").unwrap();
    let asset = cat
        .asset_of(&dunce::canonicalize(&img_path).unwrap())
        .unwrap()
        .unwrap();
    cat.save_ocr_lines(
        asset,
        &[OcrLine {
            text: "Academia Nebulosa".into(),
            score: 0.92,
            recognizer: OcrRecognizer::V6,
            rect: [10.0, 10.0, 180.0, 32.0],
            top_strip: false,
        }],
    )
    .unwrap();

    snapshot(&library);
    let before = snapshot(&library);

    let worker = WorksWorker::start(db_works, None);
    let mut finished = false;
    while let Ok(ev) = worker.recv() {
        match ev {
            WorksEvent::Finished {
                stopped,
                processed,
                matched_assets,
                ..
            } => {
                assert!(!stopped);
                assert_eq!(processed, 1);
                assert_eq!(matched_assets, 1);
                finished = true;
                break;
            }
            WorksEvent::Failed(e) => panic!("works worker failed: {e}"),
            WorksEvent::Progress { .. } => {}
        }
    }
    assert!(finished);
    assert_eq!(before, snapshot(&library));
}

#[test]
fn vision_worker_and_tags_apply_leave_the_originals_untouched() {
    struct DecodeOnlyVisionReader;

    impl VisionReader for DecodeOnlyVisionReader {
        fn batch_size(&self) -> usize {
            2
        }

        fn embed_texts(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
            Ok(texts
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let mut v = vec![0.0f32; EMBEDDING_DIM];
                    v[i % 8] = 1.0;
                    v
                })
                .collect())
        }

        fn analyze_images(
            &mut self,
            images: &[&image::RgbImage],
        ) -> Result<Vec<VisionImageOutput>, String> {
            Ok(images
                .iter()
                .map(|_| {
                    let mut v = vec![0.0f32; EMBEDDING_DIM];
                    v[0] = 1.0;
                    VisionImageOutput {
                        embedding: v,
                        faces: Vec::new(),
                    }
                })
                .collect())
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    write_png(&library.join("a.png"), [255, 0, 0, 255]);
    write_png(&library.join("sub/b.png"), [0, 255, 0, 255]);
    fs::write(library.join("sub/corrupt.jpg"), b"not an image").unwrap();

    let root = LibraryRoot::new(&library).unwrap();
    let root_path = root.as_path().to_path_buf();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("catalog.sqlite");
    let db_idx = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_vis = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_apply = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();

    let indexer = IndexWorker::start(db_idx, vec![root], None);
    while let Ok(ev) = indexer.recv() {
        if let IndexEvent::Finished { stopped, .. } = ev {
            assert!(!stopped);
            break;
        }
    }

    snapshot(&library);
    let before = snapshot(&library);

    let vis = VisionWorker::start_with_loader(
        db_vis,
        VisionScope::Roots(vec![root_path]),
        || Ok(DecodeOnlyVisionReader),
        None,
    );
    while let Ok(ev) = vis.recv() {
        match ev {
            VisionEvent::Finished {
                stopped,
                processed,
                errors,
            } => {
                assert!(!stopped);
                assert_eq!(processed, 2);
                assert_eq!(errors, 1);
                break;
            }
            VisionEvent::Failed(e) => panic!("vision failed: {e}"),
            _ => {}
        }
    }

    let apply = TagsApplyWorker::start(db_apply, None, OcrDevice::Cpu, None);
    while let Ok(ev) = apply.recv() {
        match ev {
            TagsApplyEvent::Finished {
                stopped, processed, ..
            } => {
                assert!(!stopped);
                assert_eq!(processed, 2);
                break;
            }
            TagsApplyEvent::Failed(e) => panic!("tags apply failed: {e}"),
            _ => {}
        }
    }

    assert_eq!(before, snapshot(&library));
}
