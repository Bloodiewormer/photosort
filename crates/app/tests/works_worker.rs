// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::ocr_worker::{OcrEvent, OcrImageReader, OcrScope, OcrWorker};
use app::worker::{IndexEvent, IndexWorker};
use app::works_worker::{WorksEvent, WorksWorker};
use catalog::{Catalog, LabelKind, OcrLine, OcrRecognizer, PROMO_CATEGORY, Source, Status};
use fsguard::{LibraryRoot, ensure_writable};
use image::{ImageBuffer, Rgba};
use std::fs;
use std::path::Path;

fn write_png(path: &Path, color: [u8; 4]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    ImageBuffer::from_pixel(16, 12, Rgba(color))
        .save(path)
        .unwrap();
}

#[test]
fn ocr_worker_runs_work_matcher_after_saving_lines_and_works_worker_rematches() {
    struct PromoReader;

    impl OcrImageReader for PromoReader {
        fn read_image(&self, _path: &Path) -> Result<Vec<OcrLine>, String> {
            Ok(vec![
                OcrLine {
                    text: "PROYECTOS ACTIVOS".into(),
                    score: 0.94,
                    recognizer: OcrRecognizer::V6,
                    rect: [10.0, 10.0, 240.0, 32.0],
                    top_strip: false,
                },
                OcrLine {
                    text: "Academia Nebulosa".into(),
                    score: 0.91,
                    recognizer: OcrRecognizer::V6,
                    rect: [10.0, 100.0, 200.0, 122.0],
                    top_strip: false,
                },
                OcrLine {
                    text: "Isla de Cristal".into(),
                    score: 0.89,
                    recognizer: OcrRecognizer::V6,
                    rect: [10.0, 180.0, 200.0, 202.0],
                    top_strip: false,
                },
            ])
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let img_path = library.join("promo.png");
    write_png(&img_path, [100, 120, 140, 255]);

    let root = LibraryRoot::new(&library).unwrap();
    let root_path = root.as_path().to_path_buf();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("catalog.sqlite");

    let db_index = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_main = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_ocr = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_works = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();

    let indexer = IndexWorker::start(db_index, vec![root], None);
    while let Ok(ev) = indexer.recv() {
        if let IndexEvent::Finished { stopped, .. } = ev {
            assert!(!stopped);
            break;
        }
    }

    // Create one work before OCR runs.
    let cat = Catalog::open(&db_main).unwrap();
    let w1 = cat.create_work("Academia Nebulosa").unwrap();
    let asset = cat
        .asset_of(&dunce::canonicalize(&img_path).unwrap())
        .unwrap()
        .unwrap();

    // OCR worker saves OCR lines AND immediately runs the work matcher on that image.
    let ocr = OcrWorker::start_with_loader(
        db_ocr,
        OcrScope::Roots(vec![root_path]),
        || Ok(PromoReader),
        None,
    );
    while let Ok(ev) = ocr.recv() {
        if let OcrEvent::Finished {
            stopped,
            processed,
            errors,
        } = ev
        {
            assert!(!stopped);
            assert_eq!(processed, 1);
            assert_eq!(errors, 0);
            break;
        }
    }

    let labels_after_ocr = cat.labels_of(asset).unwrap();
    assert!(
        labels_after_ocr
            .iter()
            .any(|a| a.label == w1 && a.source == Source::Ocr && a.status == Status::Suggested),
        "OCR worker must suggest matching works right after saving OCR"
    );
    assert!(
        labels_after_ocr
            .iter()
            .any(|a| a.kind == LabelKind::Category && a.name == PROMO_CATEGORY),
        "OCR worker must also suggest promo category when header matches"
    );

    // Add a second work later and run WorksWorker over all assets with OCR.
    let w2 = cat.create_work("Isla de Cristal").unwrap();
    let works_worker = WorksWorker::start(db_works, None);
    let mut finished = None;
    while let Ok(ev) = works_worker.recv() {
        match ev {
            WorksEvent::Finished {
                stopped,
                processed,
                matched_assets,
                suggestions,
            } => {
                finished = Some((stopped, processed, matched_assets, suggestions));
                break;
            }
            WorksEvent::Failed(e) => panic!("works worker failed: {e}"),
            WorksEvent::Progress { .. } => {}
        }
    }
    assert_eq!(finished, Some((false, 1, 1, 3)));

    let labels_after_works = cat.labels_of(asset).unwrap();
    assert!(labels_after_works.iter().any(|a| a.label == w1));
    assert!(labels_after_works.iter().any(|a| a.label == w2));
}
