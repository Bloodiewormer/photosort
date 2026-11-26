// Synthetic fixtures in temp dirs, never inside a real library.
#![allow(clippy::disallowed_methods)]

use app::review_engine::{EnginePhase, ReviewConfig, ReviewEngine, ReviewEngineEvent, StageKind};
use app::settings::OcrDevice;
use catalog::{Catalog, Filter, Sort};
use fsguard::{LibraryRoot, ensure_writable};
use ocr::synth::{Fonts, Split, generate};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Every file below `root` with what must not change.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u64, SystemTime, SystemTime, String)> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| {
            let path = entry.into_path();
            let metadata = fs::metadata(&path).unwrap();
            let state = (
                metadata.len(),
                metadata.modified().unwrap(),
                metadata.created().unwrap(),
                scanner::sha256_file(&path).unwrap(),
            );
            (path, state)
        })
        .collect()
}

/// The engine as the application runs it: real loaders, the three stages, both GPU models taking
/// turns chunk by chunk, until every photo is reviewed.
#[test]
#[ignore = "necesita los modelos: ver CONTRIBUTING.md"]
fn the_background_review_runs_every_stage_with_the_real_models() {
    let models = |variable: &str| {
        PathBuf::from(std::env::var_os(variable).unwrap_or_else(|| panic!("set {variable}")))
    };
    let (ocr_models, vision_models) = (
        models("PHOTOSORT_OCR_MODELS"),
        models("PHOTOSORT_VISION_MODELS"),
    );

    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("library");
    fs::create_dir_all(&library).unwrap();
    for (i, sample) in generate(Split::Tune, &Fonts::load().unwrap())
        .iter()
        .step_by(7)
        .take(4)
        .enumerate()
    {
        sample.image.save(library.join(format!("{i}.png"))).unwrap();
    }
    let root = LibraryRoot::new(&library).unwrap();
    let data = temp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let database =
        || ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    snapshot(&library);
    let before = snapshot(&library);

    let config = ReviewConfig {
        enabled: true,
        rules: true,
        vision: true,
        ocr: true,
        roots: vec![root.as_path().to_path_buf()],
        vision_models_dir: Some(vision_models),
        ocr_models_dir: Some(ocr_models),
        device: OcrDevice::DirectMl,
        // Two chunks, so that each engine is unloaded and loaded again.
        chunk_size: 2,
        ..Default::default()
    };
    let engine = ReviewEngine::start(database(), config, None);

    let deadline = Instant::now() + Duration::from_secs(400);
    let mut loads = Vec::new();
    let mut reviewed = 0;
    let last = loop {
        assert!(Instant::now() < deadline, "the review did not finish");
        match engine.recv_timeout(Duration::from_millis(200)) {
            Ok(ReviewEngineEvent::AssetProgress {
                now_reviewed: true, ..
            }) => reviewed += 1,
            Ok(ReviewEngineEvent::Status(status)) => {
                if let EnginePhase::LoadingModel(stage) = status.phase {
                    loads.push(stage);
                }
                assert!(
                    status.paused_stages.is_empty(),
                    "{:?}",
                    status.paused_stages
                );
                assert_eq!(status.failed_count, 0, "{:?}", status.last_error);
                if status.phase == EnginePhase::Idle {
                    break status;
                }
            }
            _ => {}
        }
    };
    assert_eq!(last.last_error, None);
    assert_eq!(reviewed, 4);
    assert_eq!(
        loads,
        [
            StageKind::Vision,
            StageKind::Ocr,
            StageKind::Vision,
            StageKind::Ocr
        ]
    );

    let cat = Catalog::open(&database()).unwrap();
    let entries = cat.entries(&Filter::default(), Sort::PathAsc).unwrap();
    assert_eq!(entries.len(), 4);
    for entry in &entries {
        assert!(cat.has_vision_done(entry.asset).unwrap());
        assert!(cat.has_ocr_done(entry.asset).unwrap());
    }
    assert!(
        entries
            .iter()
            .any(|e| !cat.ocr_lines(e.asset).unwrap().is_empty()),
        "no text was read from the synthetic text images"
    );
    assert_eq!(snapshot(&library), before);
}
