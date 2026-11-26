// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::settings::OcrDevice;
use app::vision_worker::{VisionEvent, VisionScope, VisionWorker};
use app::worker::{IndexEvent, IndexWorker};
use catalog::{Catalog, SIGLIP2_MODEL_ID};
use fsguard::{LibraryRoot, ensure_writable};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use vision::synth::{Group, verification_set};
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

/// Real-engine end-to-end test on the 4 synthetic scene mocks (SigLIP 2 + YuNet + catalog rules).
/// Skips cleanly when `PHOTOSORT_VISION_MODELS` is not set. Uses one provider per test process.
#[test]
fn real_engine_recognizes_synthetic_scene_mocks_and_leaves_originals_untouched() {
    let Some(models_dir) = std::env::var_os("PHOTOSORT_VISION_MODELS").map(PathBuf::from) else {
        eprintln!(
            "skipping real_engine_recognizes_synthetic_scene_mocks: PHOTOSORT_VISION_MODELS not set"
        );
        return;
    };

    let scenes: Vec<_> = verification_set()
        .into_iter()
        .filter(|item| item.group == Group::SceneMock)
        .collect();
    assert_eq!(scenes.len(), 4);

    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir_all(&library).unwrap();

    let mut paths: Vec<(&'static str, PathBuf)> = Vec::new();
    for item in &scenes {
        let p = library.join(format!("{}.png", item.id));
        item.image.save(&p).unwrap();
        paths.push((item.id, p));
    }

    let root = LibraryRoot::new(&library).unwrap();
    let root_path = root.as_path().to_path_buf();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("catalog.sqlite");
    let db_idx = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_vis = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_read = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();

    let indexer = IndexWorker::start(db_idx, vec![root], None);
    while let Ok(ev) = indexer.recv() {
        match ev {
            IndexEvent::Finished { stopped, new, .. } => {
                assert!(!stopped);
                assert_eq!(new, 4);
                break;
            }
            IndexEvent::Failed(e) => panic!("index failed: {e}"),
            _ => {}
        }
    }

    snapshot(&library);
    let before = snapshot(&library);

    // One provider per test process: DirectML batch 8.
    let worker = VisionWorker::start(
        db_vis,
        VisionScope::Roots(vec![root_path]),
        models_dir,
        OcrDevice::DirectMl,
        None,
    );
    let mut finished = None;
    while let Ok(ev) = worker.recv() {
        match ev {
            VisionEvent::Finished {
                stopped,
                processed,
                errors,
            } => {
                finished = Some((stopped, processed, errors));
                break;
            }
            VisionEvent::Failed(e) => panic!("real vision engine failed: {e}"),
            _ => {}
        }
    }
    assert_eq!(finished, Some((false, 4, 0)));
    assert_eq!(before, snapshot(&library));

    let cat = Catalog::open(&db_read).unwrap();
    assert_eq!(cat.count_assets_with_vision().unwrap(), 4);

    for (id, p) in &paths {
        let asset = cat
            .asset_of(&dunce::canonicalize(p).unwrap())
            .unwrap()
            .unwrap();
        assert!(cat.has_vision_done(asset).unwrap());
        let (model, emb) = cat.asset_embedding(asset).unwrap().unwrap();
        assert_eq!(model, SIGLIP2_MODEL_ID);
        assert_eq!(emb.len(), catalog::EMBEDDING_DIM);
        let faces = cat.asset_faces(asset).unwrap();
        let labels = cat.labels_of(asset).unwrap();
        let summary: Vec<String> = labels
            .iter()
            .map(|a| format!("{} ({:.2})", a.name, a.confidence))
            .collect();
        eprintln!(
            "e2e scene mock [{id}]: faces={}, suggestions=[{}]",
            faces.len(),
            summary.join(", ")
        );
    }
}
