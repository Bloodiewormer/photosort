// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::ocr_worker::{OcrEvent, OcrScope, OcrWorker};
use app::settings::OcrDevice;
use app::worker::{IndexEvent, IndexWorker};
use catalog::{Catalog, Filter, Sort};
use fsguard::{LibraryRoot, ensure_writable};
use ocr::synth::{Category, Fonts, Split, generate};
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

/// End-to-end test using the real ONNX OCR engine on synthetic images.
/// Skips cleanly when `PHOTOSORT_OCR_MODELS` is not set.
#[test]
fn real_engine_reads_synthetic_images_and_makes_them_searchable() {
    let Some(models_dir) = std::env::var_os("PHOTOSORT_OCR_MODELS").map(PathBuf::from) else {
        eprintln!("skipping real_engine_reads_synthetic_images: PHOTOSORT_OCR_MODELS not set");
        return;
    };

    let fonts = Fonts::load().unwrap();
    let samples = generate(Split::Tune, &fonts);
    let latin = samples
        .iter()
        .find(|s| s.category == Category::Latin)
        .unwrap();
    let korean = samples
        .iter()
        .find(|s| s.category == Category::Korean)
        .unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir_all(&library).unwrap();
    let latin_path = library.join("latin_synth.png");
    let korean_path = library.join("korean_synth.png");
    latin.image.save(&latin_path).unwrap();
    korean.image.save(&korean_path).unwrap();

    let root = LibraryRoot::new(&library).unwrap();
    let root_path = root.as_path().to_path_buf();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_index =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let db_ocr =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let db_read =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();

    let indexer = IndexWorker::start(db_index, vec![root], None);
    while let Ok(ev) = indexer.recv() {
        match ev {
            IndexEvent::Finished { stopped, new, .. } => {
                assert!(!stopped);
                assert_eq!(new, 2);
                break;
            }
            IndexEvent::Failed(e) => panic!("index failed: {e}"),
            _ => {}
        }
    }

    snapshot(&library);
    let before = snapshot(&library);

    // One provider per test process: this test binary runs DirectML once.
    let worker = OcrWorker::start(
        db_ocr,
        OcrScope::Roots(vec![root_path]),
        models_dir,
        OcrDevice::DirectMl,
        None,
    );
    let mut finished = None;
    while let Ok(ev) = worker.recv() {
        match ev {
            OcrEvent::Finished {
                stopped,
                processed,
                errors,
            } => {
                finished = Some((stopped, processed, errors));
                break;
            }
            OcrEvent::Failed(e) => panic!("real OCR engine failed: {e}"),
            _ => {}
        }
    }
    assert_eq!(finished, Some((false, 2, 0)));
    assert_eq!(before, snapshot(&library));

    let cat = Catalog::open(&db_read).unwrap();
    let latin_asset = cat
        .asset_of(&dunce::canonicalize(&latin_path).unwrap())
        .unwrap()
        .unwrap();
    let korean_asset = cat
        .asset_of(&dunce::canonicalize(&korean_path).unwrap())
        .unwrap()
        .unwrap();

    let latin_lines = cat.ocr_lines(latin_asset).unwrap();
    let korean_lines = cat.ocr_lines(korean_asset).unwrap();
    assert!(
        !latin_lines.is_empty(),
        "expected OCR lines for Latin sample"
    );
    assert!(
        !korean_lines.is_empty(),
        "expected OCR lines for Korean sample"
    );

    // Search using a phrase from the first ground-truth line of each synthetic image.
    let latin_query = latin.lines[0]
        .text
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    let latin_hits = cat
        .entries(
            &Filter {
                search: Some(latin_query.clone()),
                ..Default::default()
            },
            Sort::PathAsc,
        )
        .unwrap();
    assert!(
        latin_hits.iter().any(|e| e.asset == latin_asset),
        "expected Latin asset to match query"
    );

    let korean_query = korean.lines[0]
        .text
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    let korean_hits = cat
        .entries(
            &Filter {
                search: Some(korean_query.clone()),
                ..Default::default()
            },
            Sort::PathAsc,
        )
        .unwrap();
    assert!(
        korean_hits.iter().any(|e| e.asset == korean_asset),
        "expected Korean asset to match query"
    );
}
