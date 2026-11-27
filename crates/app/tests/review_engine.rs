// Tests create synthetic fixtures in temp dirs, never inside any real user folder.
#![allow(clippy::disallowed_methods)]

use app::ocr_worker::OcrImageReader;
use app::review_engine::{
    EnginePhase, OcrLoader, ReviewConfig, ReviewEngine, ReviewEngineEvent, StageKind, VisionLoader,
};
use app::review_flow::ReviewFlow;
use app::settings::OcrDevice;
use app::vision_worker::{VisionImageOutput, VisionReader};
use catalog::{
    AssetId, Catalog, EMBEDDING_DIM, Filter, LabelKind, OCR_STAGE, OCR_VERSION, OcrLine,
    OcrRecognizer, ReviewedFilter, SIGLIP2_MODEL_ID, Sort, Source, StageSpec, Status, VISION_STAGE,
    VISION_VERSION,
};
use fsguard::{LibraryRoot, ensure_writable};
use image::{ImageBuffer, RgbImage, Rgba};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn write_png(path: &Path, color: [u8; 4]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = ImageBuffer::from_pixel(24, 18, Rgba(color));
    img.save(path).unwrap();
}

struct FakeVision {
    active_gpu: Arc<AtomicUsize>,
    concurrent_violation: Arc<AtomicBool>,
    fail_name: Option<String>,
    calls: Arc<AtomicUsize>,
}

impl Drop for FakeVision {
    fn drop(&mut self) {
        self.active_gpu.fetch_sub(1, Ordering::SeqCst);
    }
}

impl VisionReader for FakeVision {
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

    fn decode_image(&self, path: &Path) -> Result<RgbImage, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(bad) = &self.fail_name
            && path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().contains(bad))
        {
            return Err("simulated decode error".into());
        }
        Ok(RgbImage::new(16, 16))
    }

    fn analyze_images(&mut self, images: &[&RgbImage]) -> Result<Vec<VisionImageOutput>, String> {
        if self.active_gpu.load(Ordering::SeqCst) > 1 {
            self.concurrent_violation.store(true, Ordering::SeqCst);
        }
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

struct FakeOcr {
    active_gpu: Arc<AtomicUsize>,
    concurrent_violation: Arc<AtomicBool>,
    fail_name: Option<String>,
    calls: Arc<AtomicUsize>,
    text_prefix: String,
    delay_ms: u64,
}

impl Drop for FakeOcr {
    fn drop(&mut self) {
        self.active_gpu.fetch_sub(1, Ordering::SeqCst);
    }
}

impl OcrImageReader for FakeOcr {
    fn read_image(&self, path: &Path) -> Result<Vec<OcrLine>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.active_gpu.load(Ordering::SeqCst) > 1 {
            self.concurrent_violation.store(true, Ordering::SeqCst);
        }
        if self.delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(self.delay_ms));
        }
        if let Some(bad) = &self.fail_name
            && path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().contains(bad))
        {
            return Err("simulated ocr failure".into());
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(vec![OcrLine {
            text: format!("{}{stem}", self.text_prefix),
            score: 0.94,
            recognizer: OcrRecognizer::V6,
            rect: [4.0, 4.0, 120.0, 24.0],
            top_strip: false,
        }])
    }
}

fn wait_until<F>(engine: &ReviewEngine, timeout: Duration, mut pred: F) -> bool
where
    F: FnMut(&ReviewEngineEvent) -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match engine.recv_timeout(Duration::from_millis(50)) {
            Ok(ev) => {
                if pred(&ev) {
                    return true;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return false,
        }
    }
    false
}

#[test]
fn reviewed_derivation_and_enabling_a_stage() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    write_png(&lib.join("a.png"), [255, 0, 0, 255]);
    write_png(&lib.join("b.png"), [0, 255, 0, 255]);
    let root = LibraryRoot::new(&lib).unwrap();
    let roots = vec![root.as_path().to_path_buf()];
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db = ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&root).unwrap();

    let entries = cat.entries(&Filter::default(), Sort::PathAsc).unwrap();
    let a = entries[0].asset;
    let b = entries[1].asset;

    // With no stages enabled, nothing counts as reviewed.
    let rc_none = cat.reviewed_counts(&[], Some(&roots)).unwrap();
    assert_eq!((rc_none.all, rc_none.pending, rc_none.reviewed), (2, 2, 0));
    let f_rev_none = Filter {
        roots: Some(roots.clone()),
        reviewed: Some((ReviewedFilter::Reviewed, vec![])),
        ..Default::default()
    };
    assert!(cat.entries(&f_rev_none, Sort::PathAsc).unwrap().is_empty());

    // Mark rules done on `a` only.
    cat.mark_done(a, rules::STAGE, rules::VERSION).unwrap();
    let rules_only = vec![StageSpec::new(rules::STAGE, rules::VERSION)];
    let rc_rules = cat.reviewed_counts(&rules_only, Some(&roots)).unwrap();
    assert_eq!(
        (rc_rules.all, rc_rules.pending, rc_rules.reviewed),
        (2, 1, 1)
    );
    assert!(cat.is_asset_reviewed(a, &rules_only).unwrap());
    assert!(!cat.is_asset_reviewed(b, &rules_only).unwrap());

    // Enabling a second stage (`vision`) makes `a` pending again!
    let rules_and_vision = vec![
        StageSpec::new(rules::STAGE, rules::VERSION),
        StageSpec::new(VISION_STAGE, VISION_VERSION),
    ];
    let rc_two = cat
        .reviewed_counts(&rules_and_vision, Some(&roots))
        .unwrap();
    assert_eq!((rc_two.all, rc_two.pending, rc_two.reviewed), (2, 2, 0));

    // Completing vision on `a` makes `a` reviewed again.
    cat.mark_done(a, VISION_STAGE, VISION_VERSION).unwrap();
    let rc_two_after = cat
        .reviewed_counts(&rules_and_vision, Some(&roots))
        .unwrap();
    assert_eq!(
        (
            rc_two_after.all,
            rc_two_after.pending,
            rc_two_after.reviewed
        ),
        (2, 1, 1)
    );
    let f_rev = Filter {
        roots: Some(roots.clone()),
        reviewed: Some((ReviewedFilter::Reviewed, rules_and_vision.clone())),
        ..Default::default()
    };
    let rev_entries = cat.entries(&f_rev, Sort::PathAsc).unwrap();
    assert_eq!(rev_entries.len(), 1);
    assert_eq!(rev_entries[0].asset, a);
}

#[test]
fn engine_marks_chunks_progressively_and_never_holds_both_gpu_models() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    for i in 0..4 {
        write_png(
            &lib.join(format!("img_{i}.png")),
            [40 * i as u8, 120, 200, 255],
        );
    }
    let root = LibraryRoot::new(&lib).unwrap();
    let roots = vec![root.as_path().to_path_buf()];
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("catalog.sqlite");
    let db = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();

    let active_gpu = Arc::new(AtomicUsize::new(0));
    let concurrent_violation = Arc::new(AtomicBool::new(false));
    let vis_calls = Arc::new(AtomicUsize::new(0));
    let ocr_calls = Arc::new(AtomicUsize::new(0));

    let ag1 = Arc::clone(&active_gpu);
    let cv1 = Arc::clone(&concurrent_violation);
    let vc = Arc::clone(&vis_calls);
    let vision_loader: VisionLoader = Box::new(move |_dir, _dev| {
        let prev = ag1.fetch_add(1, Ordering::SeqCst);
        if prev > 0 {
            cv1.store(true, Ordering::SeqCst);
        }
        Ok(Box::new(FakeVision {
            active_gpu: Arc::clone(&ag1),
            concurrent_violation: Arc::clone(&cv1),
            fail_name: None,
            calls: Arc::clone(&vc),
        }))
    });

    let ag2 = Arc::clone(&active_gpu);
    let cv2 = Arc::clone(&concurrent_violation);
    let oc = Arc::clone(&ocr_calls);
    let ocr_loader: OcrLoader = Box::new(move |_dir, _dev| {
        let prev = ag2.fetch_add(1, Ordering::SeqCst);
        if prev > 0 {
            cv2.store(true, Ordering::SeqCst);
        }
        Ok(Box::new(FakeOcr {
            active_gpu: Arc::clone(&ag2),
            concurrent_violation: Arc::clone(&cv2),
            fail_name: None,
            calls: Arc::clone(&oc),
            text_prefix: "txt_".into(),
            delay_ms: 0,
        }))
    });

    let cfg = ReviewConfig {
        enabled: true,
        rules: true,
        vision: true,
        ocr: true,
        roots: roots.clone(),
        vision_models_dir: None,
        ocr_models_dir: None,
        device: OcrDevice::Cpu,
        chunk_size: 2, // 4 images -> 2 chunks of 2
        ..Default::default()
    };

    let engine = ReviewEngine::start_with_loaders(db, cfg, vision_loader, ocr_loader, None);
    let mut reviewed_at_first_chunk = 0usize;
    let mut reviewed_seen: HashSet<AssetId> = HashSet::new();
    let mut chunks = 0usize;

    let reached_idle = wait_until(&engine, Duration::from_secs(10), |ev| match ev {
        ReviewEngineEvent::AssetProgress {
            asset,
            now_reviewed: true,
        } => {
            reviewed_seen.insert(*asset);
            false
        }
        ReviewEngineEvent::ChunkFinished => {
            chunks += 1;
            if chunks == 1 {
                reviewed_at_first_chunk = reviewed_seen.len();
            }
            false
        }
        ReviewEngineEvent::Status(snap) => matches!(snap.phase, EnginePhase::Idle),
        _ => false,
    });
    assert!(
        reached_idle,
        "engine should finish all 4 images and go idle"
    );
    assert_eq!(
        reviewed_at_first_chunk, 2,
        "first chunk of 2 must be reviewed before chunk 2 starts"
    );
    assert_eq!(reviewed_seen.len(), 4);
    assert!(
        !concurrent_violation.load(Ordering::SeqCst),
        "Vision and OCR must never be loaded in GPU memory simultaneously"
    );
    assert_eq!(active_gpu.load(Ordering::SeqCst), 0);
}

#[test]
fn engine_skips_failing_photo_without_retry_loop_until_retry_requested() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    write_png(&lib.join("good.png"), [10, 200, 10, 255]);
    write_png(&lib.join("bad_ocr.png"), [200, 10, 10, 255]);
    let root = LibraryRoot::new(&lib).unwrap();
    let roots = vec![root.as_path().to_path_buf()];
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db = ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();

    let ocr_calls = Arc::new(AtomicUsize::new(0));
    let oc = Arc::clone(&ocr_calls);
    let active_gpu = Arc::new(AtomicUsize::new(0));
    let cv = Arc::new(AtomicBool::new(false));
    let ocr_loader: OcrLoader = Box::new(move |_dir, _dev| {
        active_gpu.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeOcr {
            active_gpu: Arc::clone(&active_gpu),
            concurrent_violation: Arc::clone(&cv),
            fail_name: Some("bad_ocr".into()),
            calls: Arc::clone(&oc),
            text_prefix: "ok_".into(),
            delay_ms: 0,
        }))
    });
    let vision_loader: VisionLoader = Box::new(|_, _| Err("unused".into()));

    let cfg = ReviewConfig {
        enabled: true,
        rules: true,
        vision: false,
        ocr: true,
        roots,
        chunk_size: 4,
        ..Default::default()
    };

    let engine = ReviewEngine::start_with_loaders(db, cfg, vision_loader, ocr_loader, None);
    let mut failed_count = 0usize;
    let idle = wait_until(&engine, Duration::from_secs(5), |ev| match ev {
        ReviewEngineEvent::Status(snap) => {
            failed_count = snap.failed_count;
            matches!(snap.phase, EnginePhase::Idle)
        }
        _ => false,
    });
    assert!(idle);
    assert_eq!(failed_count, 1);
    // Called once for good.png and once for bad_ocr.png — NO retry loop!
    assert_eq!(ocr_calls.load(Ordering::SeqCst), 2);

    // Explicitly clicking "Reintentar" tries bad_ocr.png once more (not good.png).
    engine.retry_failed();
    let idle2 = wait_until(&engine, Duration::from_secs(5), |ev| {
        matches!(
            ev,
            ReviewEngineEvent::Status(snap) if matches!(snap.phase, EnginePhase::Idle) && snap.failed_count == 1
        )
    });
    assert!(idle2);
    assert_eq!(ocr_calls.load(Ordering::SeqCst), 3);
}

#[test]
fn engine_model_load_failure_pauses_only_that_stage_while_others_continue() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    write_png(&lib.join("Screenshot_01.png"), [10, 10, 200, 255]);
    let root = LibraryRoot::new(&lib).unwrap();
    let roots = vec![root.as_path().to_path_buf()];
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("catalog.sqlite");
    let db = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_check = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();

    let load_attempts = Arc::new(AtomicUsize::new(0));
    let la = Arc::clone(&load_attempts);
    let vision_loader: VisionLoader = Box::new(move |_, _| {
        la.fetch_add(1, Ordering::SeqCst);
        Err("DirectML no disponible".into())
    });
    let ocr_loader: OcrLoader = Box::new(|_, _| Err("unused".into()));

    let cfg = ReviewConfig {
        enabled: true,
        rules: true,
        vision: true,
        ocr: false,
        roots,
        ..Default::default()
    };

    let engine = ReviewEngine::start_with_loaders(db, cfg, vision_loader, ocr_loader, None);
    let mut paused_reason = None;
    let idle = wait_until(&engine, Duration::from_secs(5), |ev| match ev {
        ReviewEngineEvent::Status(snap) => {
            if let Some(p) = snap
                .paused_stages
                .iter()
                .find(|p| p.stage == StageKind::Vision)
            {
                paused_reason = Some(p.reason.clone());
            }
            matches!(snap.phase, EnginePhase::Idle)
        }
        _ => false,
    });
    assert!(idle);
    assert_eq!(
        load_attempts.load(Ordering::SeqCst),
        1,
        "no retry loop on load error"
    );
    assert_eq!(paused_reason.as_deref(), Some("DirectML no disponible"));

    // Rules stage MUST still have completed even though Vision failed to load!
    let cat = Catalog::open(&db_check).unwrap();
    let rules_only = vec![StageSpec::new(rules::STAGE, rules::VERSION)];
    let rc = cat.reviewed_counts(&rules_only, None).unwrap();
    assert_eq!(rc.reviewed, 1);
}

#[test]
fn engine_pause_resume_and_yield_for_heavy_task() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    for i in 0..4 {
        write_png(
            &lib.join(format!("p_{i}.png")),
            [50, 50 * i as u8, 100, 255],
        );
    }
    let root = LibraryRoot::new(&lib).unwrap();
    let roots = vec![root.as_path().to_path_buf()];
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db = ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();

    let ag = Arc::new(AtomicUsize::new(0));
    let cv = Arc::new(AtomicBool::new(false));
    let oc = Arc::new(AtomicUsize::new(0));
    let ocr_loader: OcrLoader = Box::new(move |_, _| {
        ag.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeOcr {
            active_gpu: Arc::clone(&ag),
            concurrent_violation: Arc::clone(&cv),
            fail_name: None,
            calls: Arc::clone(&oc),
            text_prefix: "p_".into(),
            delay_ms: 40,
        }))
    });
    let vision_loader: VisionLoader = Box::new(|_, _| Err("unused".into()));

    let cfg = ReviewConfig {
        enabled: true,
        rules: false,
        vision: false,
        ocr: true,
        roots,
        chunk_size: 4,
        ..Default::default()
    };

    let engine = ReviewEngine::start_with_loaders(db, cfg, vision_loader, ocr_loader, None);
    // Wait for first asset progress, then pause.
    assert!(wait_until(&engine, Duration::from_secs(5), |ev| {
        matches!(ev, ReviewEngineEvent::AssetProgress { .. })
    }));
    engine.set_paused(true);
    assert!(wait_until(&engine, Duration::from_secs(5), |ev| {
        matches!(ev, ReviewEngineEvent::Status(s) if matches!(s.phase, EnginePhase::Paused))
    }));

    // Resume, then immediately yield for a heavy task.
    engine.set_yielded(true);
    engine.set_paused(false);
    assert!(wait_until(&engine, Duration::from_secs(5), |ev| {
        matches!(ev, ReviewEngineEvent::Status(s) if matches!(s.phase, EnginePhase::Yielded))
    }));

    // Release yield -> finishes all 4 images cleanly.
    engine.set_yielded(false);
    assert!(wait_until(&engine, Duration::from_secs(10), |ev| {
        matches!(ev, ReviewEngineEvent::Status(s) if matches!(s.phase, EnginePhase::Idle))
    }));
}

#[test]
fn engine_priority_rereview_runs_when_off_and_preserves_decisions_and_old_data_until_replaced() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    write_png(&lib.join("a.png"), [100, 100, 100, 255]);
    write_png(&lib.join("b.png"), [200, 200, 200, 255]);
    let root = LibraryRoot::new(&lib).unwrap();
    let roots = vec![root.as_path().to_path_buf()];
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("catalog.sqlite");
    let db_setup = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_engine = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();

    let mut cat = Catalog::open(&db_setup).unwrap();
    cat.index(&root).unwrap();
    let entries = cat.entries(&Filter::default(), Sort::PathAsc).unwrap();
    let asset_a = entries[0].asset;
    let asset_b = entries[1].asset;

    // Seed old OCR text + user accepted & rejected decisions on `a`.
    cat.save_ocr_lines(
        asset_a,
        &[OcrLine {
            text: "old_text".into(),
            score: 0.9,
            recognizer: OcrRecognizer::V6,
            rect: [0.0, 0.0, 50.0, 20.0],
            top_strip: false,
        }],
    )
    .unwrap();
    let lbl_keep = cat.label(LabelKind::Tag, "favorito").unwrap();
    let lbl_rej = cat.label(LabelKind::Tag, "descartar").unwrap();
    cat.accept(asset_a, lbl_keep).unwrap();
    cat.reject(asset_a, lbl_rej).unwrap();

    // Reset OCR stage on `a`: old OCR text MUST stay until new results replace it!
    cat.reset_stages(&[asset_a], &[OCR_STAGE]).unwrap();
    let lines_before = cat.ocr_lines(asset_a).unwrap();
    assert_eq!(lines_before.len(), 1);
    assert_eq!(lines_before[0].text, "old_text");

    let ag = Arc::new(AtomicUsize::new(0));
    let cv = Arc::new(AtomicBool::new(false));
    let oc = Arc::new(AtomicUsize::new(0));
    let ocr_loader: OcrLoader = Box::new(move |_, _| {
        ag.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeOcr {
            active_gpu: Arc::clone(&ag),
            concurrent_violation: Arc::clone(&cv),
            fail_name: None,
            calls: Arc::clone(&oc),
            text_prefix: "fresh_".into(),
            delay_ms: 0,
        }))
    });
    let vision_loader: VisionLoader = Box::new(|_, _| Err("unused".into()));

    // Master switch is OFF!
    let cfg = ReviewConfig {
        enabled: false,
        rules: false,
        vision: false,
        ocr: true,
        roots,
        ..Default::default()
    };
    let engine = ReviewEngine::start_with_loaders(db_engine, cfg, vision_loader, ocr_loader, None);

    // Request priority re-review for `asset_a` only.
    engine.prioritize(vec![asset_a]);
    assert!(wait_until(&engine, Duration::from_secs(5), |ev| {
        matches!(
            ev,
            ReviewEngineEvent::AssetProgress {
                asset,
                now_reviewed: true
            } if *asset == asset_a
        )
    }));
    // Returns to Off without touching `asset_b`.
    assert!(wait_until(&engine, Duration::from_secs(5), |ev| {
        matches!(ev, ReviewEngineEvent::Status(s) if matches!(s.phase, EnginePhase::Off))
    }));

    // Verify `a` got fresh OCR text, `b` was NOT processed, and user decisions were preserved!
    let lines_after = cat.ocr_lines(asset_a).unwrap();
    assert_eq!(lines_after[0].text, "fresh_a");
    assert!(!cat.has_ocr_done(asset_b).unwrap());

    let assigns = cat.labels_of(asset_a).unwrap();
    assert!(
        assigns
            .iter()
            .any(|a| a.label == lbl_keep && a.status == Status::Accepted)
    );
    assert!(
        assigns
            .iter()
            .any(|a| a.label == lbl_rej && a.status == Status::Rejected)
    );
}

#[test]
fn removed_folder_is_hidden_everywhere_but_kept_and_restored_when_readded() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_a = tmp.path().join("folder_a");
    let lib_b = tmp.path().join("folder_b");
    write_png(&lib_a.join("a1.png"), [255, 0, 0, 255]);
    write_png(&lib_b.join("b1.png"), [0, 255, 0, 255]);
    write_png(&lib_b.join("b2_dup.png"), [0, 255, 0, 255]); // duplicate inside folder_b

    let root_a = LibraryRoot::new(&lib_a).unwrap();
    let root_b = LibraryRoot::new(&lib_b).unwrap();
    let path_a = root_a.as_path().to_path_buf();
    let path_b = root_b.as_path().to_path_buf();
    let both_roots = vec![path_a.clone(), path_b.clone()];
    let only_a = vec![path_a.clone()];

    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let roots_guard = [root_a, root_b];
    let db = ensure_writable(&data.join("catalog.sqlite"), &roots_guard).unwrap();
    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&roots_guard[0]).unwrap();
    cat.index(&roots_guard[1]).unwrap();

    let all_entries = cat.entries(&Filter::default(), Sort::PathAsc).unwrap();
    assert_eq!(all_entries.len(), 3);
    let asset_b = all_entries
        .iter()
        .find(|e| e.path.starts_with(&path_b))
        .unwrap()
        .asset;

    let tag = cat.label(LabelKind::Tag, "especial").unwrap();
    let sug = cat.label(LabelKind::Tag, "dudosa").unwrap();
    cat.accept(asset_b, tag).unwrap();
    cat.suggest(asset_b, sug, Source::Ai, 0.40).unwrap();
    let mut emb = vec![0.0f32; EMBEDDING_DIM];
    emb[0] = 1.0;
    cat.save_vision_result(asset_b, SIGLIP2_MODEL_ID, &emb, &[])
        .unwrap();

    // Remove folder_b: every view scoped to `only_a` hides folder_b completely!
    let f_a = Filter {
        roots: Some(only_a.clone()),
        ..Default::default()
    };
    assert_eq!(cat.entries(&f_a, Sort::PathAsc).unwrap().len(), 1);
    assert_eq!(cat.counts_in_roots(&only_a).unwrap().locations, 1);
    assert!(cat.labels_in_use_in_roots(&only_a).unwrap().is_empty());
    assert!(cat.review_queue_in_roots(0.60, &only_a).unwrap().is_empty());
    assert!(cat.duplicate_groups_in_roots(&only_a).unwrap().is_empty());
    assert_eq!(cat.count_assets_with_vision_in_roots(&only_a).unwrap(), 0);
    assert!(
        cat.rank_by_embeddings_in_roots(SIGLIP2_MODEL_ID, &[&emb], &only_a, 10)
            .unwrap()
            .is_empty()
    );
    let mut flow = ReviewFlow::new();
    flow.reload_in_roots(&cat, &only_a).unwrap();
    assert_eq!(flow.total_items(), 0);

    // Re-add folder_b: every photo, duplicate, suggestion, and user decision is restored!
    let f_both = Filter {
        roots: Some(both_roots.clone()),
        ..Default::default()
    };
    assert_eq!(cat.entries(&f_both, Sort::PathAsc).unwrap().len(), 3);
    assert_eq!(cat.counts_in_roots(&both_roots).unwrap().locations, 3);
    assert_eq!(cat.duplicate_groups_in_roots(&both_roots).unwrap().len(), 1);
    assert_eq!(
        cat.count_assets_with_vision_in_roots(&both_roots).unwrap(),
        1
    );
    flow.reload_in_roots(&cat, &both_roots).unwrap();
    assert_eq!(flow.total_items(), 1);
    assert!(
        cat.labels_of(asset_b)
            .unwrap()
            .iter()
            .any(|a| a.label == tag && a.status == Status::Accepted)
    );

    // Keep Mutex import used? Clean check:
    let _ = Mutex::new(0usize);
    let _ = PathBuf::new();
    let _ = OCR_VERSION;
}

#[test]
#[ignore = "measures real Vision and OCR model load times on synthetic images; requires PHOTOSORT_VISION_MODELS and PHOTOSORT_OCR_MODELS"]
fn measure_real_model_load_times_on_synthetic_images() {
    use app::ocr_worker::EngineOcrReader;
    use app::vision_worker::EngineVisionReader;

    let vision_dir = PathBuf::from(
        std::env::var("PHOTOSORT_VISION_MODELS").expect("PHOTOSORT_VISION_MODELS must be set"),
    );
    let ocr_dir = PathBuf::from(
        std::env::var("PHOTOSORT_OCR_MODELS").expect("PHOTOSORT_OCR_MODELS must be set"),
    );

    let tmp = tempfile::tempdir().unwrap();
    let img_path = tmp.path().join("synth.png");
    write_png(&img_path, [180, 120, 60, 255]);

    let t0 = Instant::now();
    let mut vis = EngineVisionReader::load(&vision_dir, OcrDevice::DirectMl).unwrap();
    let vis_load_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let dec = vis.decode_image(&img_path).unwrap();
    let t1 = Instant::now();
    let _ = vis.analyze_images(&[&dec]).unwrap();
    let vis_infer_ms = t1.elapsed().as_secs_f64() * 1000.0;
    drop(vis);

    let t2 = Instant::now();
    let ocr = EngineOcrReader::load(&ocr_dir, OcrDevice::DirectMl).unwrap();
    let ocr_load_ms = t2.elapsed().as_secs_f64() * 1000.0;
    let t3 = Instant::now();
    let _ = ocr.read_image(&img_path).unwrap();
    let ocr_infer_ms = t3.elapsed().as_secs_f64() * 1000.0;
    drop(ocr);

    println!(
        "Real model timings (DirectML): Vision load={vis_load_ms:.1} ms, 1-img={vis_infer_ms:.1} ms | OCR load={ocr_load_ms:.1} ms, 1-img={ocr_infer_ms:.1} ms"
    );
}

/// An indexed synthetic library, with two catalog handles: one for the test and one for the engine.
fn indexed(
    folders: &[&str],
    files: &[(&str, [u8; 4])],
) -> (tempfile::TempDir, Vec<PathBuf>, Catalog, fsguard::DestPath) {
    let tmp = tempfile::tempdir().unwrap();
    for (name, color) in files {
        write_png(&tmp.path().join(name), *color);
    }
    let roots: Vec<LibraryRoot> = folders
        .iter()
        .map(|f| LibraryRoot::new(&tmp.path().join(f)).unwrap())
        .collect();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("catalog.sqlite");
    let mut cat = Catalog::open(&ensure_writable(&db_path, &roots).unwrap()).unwrap();
    for root in &roots {
        cat.index(root).unwrap();
    }
    let engine_db = ensure_writable(&db_path, &roots).unwrap();
    let paths = roots.iter().map(|r| r.as_path().to_path_buf()).collect();
    (tmp, paths, cat, engine_db)
}

/// Counts model loads and records every file it is asked to read.
fn recording_ocr(
    delay_ms: u64,
    loads: Arc<AtomicUsize>,
    read: Arc<Mutex<Vec<PathBuf>>>,
) -> OcrLoader {
    struct Recording(u64, Arc<Mutex<Vec<PathBuf>>>);
    impl OcrImageReader for Recording {
        fn read_image(&self, path: &Path) -> Result<Vec<OcrLine>, String> {
            std::thread::sleep(Duration::from_millis(self.0));
            self.1.lock().unwrap().push(path.to_path_buf());
            Ok(Vec::new())
        }
    }
    Box::new(move |_, _| {
        loads.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Recording(delay_ms, Arc::clone(&read))))
    })
}

fn six_files() -> Vec<(&'static str, [u8; 4])> {
    [
        "lib/a.png",
        "lib/b.png",
        "lib/c.png",
        "lib/d.png",
        "lib/e.png",
        "lib/f.png",
    ]
    .into_iter()
    .enumerate()
    .map(|(i, name)| (name, [i as u8 * 30, 10, 10, 255]))
    .collect()
}

fn is_progress(ev: &ReviewEngineEvent) -> bool {
    matches!(ev, ReviewEngineEvent::AssetProgress { .. })
}

fn all_ocr_done(cat: &Catalog) -> bool {
    let entries = cat.entries(&Filter::default(), Sort::PathAsc).unwrap();
    entries.iter().all(|e| cat.has_ocr_done(e.asset).unwrap())
}

/// Polls the catalog until `done` or the time runs out.
fn eventually(mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

#[test]
fn pausing_a_requested_review_does_not_lose_the_photos_still_waiting() {
    let (_tmp, roots, cat, db) = indexed(&["lib"], &six_files());
    let ids: Vec<AssetId> = cat
        .entries(&Filter::default(), Sort::PathAsc)
        .unwrap()
        .iter()
        .map(|e| e.asset)
        .collect();
    let (loads, read) = (Arc::default(), Arc::default());
    // The background switch is off: only the requested queue can bring these photos back.
    let cfg = ReviewConfig {
        enabled: false,
        rules: false,
        vision: false,
        ocr: true,
        roots,
        ..Default::default()
    };
    let engine = ReviewEngine::start_with_loaders(
        db,
        cfg,
        Box::new(|_, _| Err("unused".into())),
        recording_ocr(60, loads, read),
        None,
    );
    engine.prioritize(ids);
    assert!(wait_until(&engine, Duration::from_secs(5), is_progress));
    engine.set_paused(true);
    assert!(wait_until(&engine, Duration::from_secs(5), |ev| {
        matches!(ev, ReviewEngineEvent::Status(s) if s.phase == EnginePhase::Paused)
    }));
    assert!(!all_ocr_done(&cat), "it was paused half way");
    engine.set_paused(false);
    assert!(eventually(|| all_ocr_done(&cat)));
}

#[test]
fn saving_unchanged_settings_neither_interrupts_nor_reloads_the_models() {
    let (_tmp, roots, cat, db) = indexed(&["lib"], &six_files());
    let (loads, read): (Arc<AtomicUsize>, _) = (Arc::default(), Arc::default());
    let cfg = ReviewConfig {
        enabled: true,
        rules: false,
        vision: false,
        ocr: true,
        roots,
        ..Default::default()
    };
    let engine = ReviewEngine::start_with_loaders(
        db,
        cfg.clone(),
        Box::new(|_, _| Err("unused".into())),
        recording_ocr(60, Arc::clone(&loads), read),
        None,
    );
    assert!(wait_until(&engine, Duration::from_secs(5), is_progress));
    for _ in 0..5 {
        engine.update_config(cfg.clone());
        std::thread::sleep(Duration::from_millis(40));
    }
    assert!(eventually(|| all_ocr_done(&cat)));
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

#[test]
fn an_automatic_folder_check_during_a_review_does_not_reload_the_model() {
    let (_tmp, roots, cat, db) = indexed(&["lib"], &six_files());
    let (loads, read): (Arc<AtomicUsize>, _) = (Arc::default(), Arc::default());
    let cfg = ReviewConfig {
        enabled: true,
        rules: false,
        vision: false,
        ocr: true,
        chunk_size: 2,
        roots,
        ..Default::default()
    };
    let engine = ReviewEngine::start_with_loaders(
        db,
        cfg,
        Box::new(|_, _| Err("unused".into())),
        recording_ocr(60, Arc::clone(&loads), read),
        None,
    );
    assert!(wait_until(&engine, Duration::from_secs(5), is_progress));
    engine.request_index();
    assert!(wait_until(&engine, Duration::from_secs(5), |ev| matches!(
        ev,
        ReviewEngineEvent::IndexFinished { .. }
    )));
    assert!(eventually(|| all_ocr_done(&cat)));
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

#[test]
fn photos_are_never_read_from_a_folder_that_was_removed() {
    // The same bytes in both folders: one asset, and the removed copy sorts first.
    let (_tmp, roots, cat, db) = indexed(
        &["a_removed", "b_kept"],
        &[
            ("a_removed/same.png", [9, 9, 9, 255]),
            ("b_kept/same.png", [9, 9, 9, 255]),
        ],
    );
    let kept = roots[1].clone();
    let (loads, read): (_, Arc<Mutex<Vec<PathBuf>>>) = (Arc::default(), Arc::default());
    let cfg = ReviewConfig {
        enabled: true,
        rules: false,
        vision: false,
        ocr: true,
        roots: vec![kept.clone()],
        ..Default::default()
    };
    let _engine = ReviewEngine::start_with_loaders(
        db,
        cfg,
        Box::new(|_, _| Err("unused".into())),
        recording_ocr(0, loads, Arc::clone(&read)),
        None,
    );
    assert!(eventually(|| all_ocr_done(&cat)));
    let read = read.lock().unwrap();
    assert!(!read.is_empty());
    assert!(read.iter().all(|path| path.starts_with(&kept)), "{read:?}");
}

#[test]
fn a_result_the_catalog_rejects_is_not_analysed_over_and_over() {
    struct BadVision(Arc<AtomicUsize>);
    impl VisionReader for BadVision {
        fn batch_size(&self) -> usize {
            1
        }
        fn embed_texts(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
            Ok(texts.iter().map(|_| vec![0.5; EMBEDDING_DIM]).collect())
        }
        fn decode_image(&self, _: &Path) -> Result<RgbImage, String> {
            Ok(RgbImage::new(8, 8))
        }
        fn analyze_images(&mut self, _: &[&RgbImage]) -> Result<Vec<VisionImageOutput>, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            // The catalog refuses a vector of the wrong size when saving it.
            Ok(vec![VisionImageOutput {
                embedding: vec![1.0; 3],
                faces: Vec::new(),
            }])
        }
    }
    let (_tmp, roots, cat, db) = indexed(&["lib"], &six_files());
    let stage_roots = roots.clone();
    let analysed: Arc<AtomicUsize> = Arc::default();
    let counter = Arc::clone(&analysed);
    let cfg = ReviewConfig {
        enabled: true,
        rules: false,
        vision: true,
        ocr: false,
        roots,
        error_backoff: Duration::from_millis(5),
        ..Default::default()
    };
    let _engine = ReviewEngine::start_with_loaders(
        db,
        cfg,
        Box::new(move |_, _| Ok(Box::new(BadVision(Arc::clone(&counter))))),
        Box::new(|_, _| Err("unused".into())),
        None,
    );
    std::thread::sleep(Duration::from_millis(1500));
    let count = analysed.load(Ordering::SeqCst);
    assert!((1..=6).contains(&count), "analysed {count} times");
    // The catalog failed, not the photo: nothing may be held back in later sessions.
    assert!(
        cat.list_stage_errors_in_roots(&stage_roots)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_photo_that_cannot_be_opened_is_not_marked_as_broken_for_later_sessions() {
    let (tmp, roots, cat, db) = indexed(&["lib"], &[("lib/gone.png", [1, 2, 3, 255])]);
    // A drive unplugged while a chunk is running: the engine has already picked the photo when
    // the model loads. The whole library goes, so the photo is not flagged as missing.
    let (library, away) = (roots[0].clone(), tmp.path().join("away"));
    let calls: Arc<AtomicUsize> = Arc::default();
    let counter = Arc::clone(&calls);
    let cfg = ReviewConfig {
        enabled: true,
        rules: false,
        vision: false,
        ocr: true,
        roots: roots.clone(),
        ..Default::default()
    };
    let engine = ReviewEngine::start_with_loaders(
        db,
        cfg,
        Box::new(|_, _| Err("unused".into())),
        Box::new(move |_, _| {
            let _ = fs::rename(&library, &away);
            Ok(Box::new(FakeOcr {
                active_gpu: Arc::new(AtomicUsize::new(1)),
                concurrent_violation: Arc::default(),
                fail_name: Some("gone".into()),
                calls: Arc::clone(&counter),
                text_prefix: String::new(),
                delay_ms: 0,
            }))
        }),
        None,
    );
    assert!(wait_until(&engine, Duration::from_secs(5), |ev| matches!(
        ev,
        ReviewEngineEvent::Status(s) if matches!(s.phase, EnginePhase::Idle) && s.failed_count == 1
    )));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(cat.list_stage_errors_in_roots(&roots).unwrap().is_empty());
}

#[test]
fn persisted_stage_errors_are_skipped_across_sessions_until_retried() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    write_png(&lib.join("good.png"), [10, 200, 10, 255]);
    write_png(&lib.join("bad.png"), [200, 10, 10, 255]);
    let root = LibraryRoot::new(&lib).unwrap();
    let roots = vec![root.as_path().to_path_buf()];
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("catalog.sqlite");
    let db0 = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db1 = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db2 = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();

    let mut cat = Catalog::open(&db0).unwrap();
    cat.index(&root).unwrap();
    let bad_asset = cat
        .asset_of(&dunce::canonicalize(lib.join("bad.png")).unwrap())
        .unwrap()
        .unwrap();

    // Session 1: OCR fails on "bad.png".
    let calls_1 = Arc::new(AtomicUsize::new(0));
    let calls_1_bg = Arc::clone(&calls_1);
    let active_1 = Arc::new(AtomicUsize::new(0));
    let viol_1 = Arc::new(AtomicBool::new(false));
    let cfg = ReviewConfig {
        enabled: true,
        rules: false,
        vision: false,
        ocr: true,
        roots: roots.clone(),
        ..Default::default()
    };
    let engine1 = ReviewEngine::start_with_loaders(
        db1,
        cfg.clone(),
        Box::new(|_, _| Err("unused".into())),
        Box::new(move |_, _| {
            active_1.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeOcr {
                active_gpu: Arc::clone(&active_1),
                concurrent_violation: Arc::clone(&viol_1),
                fail_name: Some("bad".into()),
                calls: Arc::clone(&calls_1_bg),
                text_prefix: "ok".into(),
                delay_ms: 0,
            }))
        }),
        None,
    );
    assert!(eventually(|| cat
        .count_stage_error_assets_in_roots(&roots)
        .unwrap()
        == 1));
    drop(engine1);

    let persisted = cat.list_stage_errors_in_roots(&roots).unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].asset, bad_asset);
    assert_eq!(persisted[0].stage, OCR_STAGE);
    assert!(!persisted[0].reason.contains("bad.png"));

    // Session 2: a fresh engine starts with a working OCR reader, but MUST skip the persisted failure!
    let calls_2 = Arc::new(AtomicUsize::new(0));
    let calls_2_bg = Arc::clone(&calls_2);
    let active_2 = Arc::new(AtomicUsize::new(0));
    let viol_2 = Arc::new(AtomicBool::new(false));
    let engine2 = ReviewEngine::start_with_loaders(
        db2,
        cfg,
        Box::new(|_, _| Err("unused".into())),
        Box::new(move |_, _| {
            active_2.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeOcr {
                active_gpu: Arc::clone(&active_2),
                concurrent_violation: Arc::clone(&viol_2),
                fail_name: None,
                calls: Arc::clone(&calls_2_bg),
                text_prefix: "ok".into(),
                delay_ms: 0,
            }))
        }),
        None,
    );
    // Wait until engine2 goes Idle without retrying bad.png.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_idle = false;
    while Instant::now() < deadline {
        if let Ok(ReviewEngineEvent::Status(s)) = engine2.recv_timeout(Duration::from_millis(50))
            && matches!(s.phase, EnginePhase::Idle)
        {
            assert_eq!(s.failed_count, 1);
            saw_idle = true;
            break;
        }
    }
    assert!(saw_idle);
    assert_eq!(calls_2.load(Ordering::SeqCst), 0);

    // Explicit retry_asset clears the persisted error and processes bad.png!
    engine2.retry_asset(bad_asset);
    assert!(eventually(|| cat.has_ocr_done(bad_asset).unwrap()));
    assert_eq!(cat.count_stage_error_assets_in_roots(&roots).unwrap(), 0);
    assert_eq!(calls_2.load(Ordering::SeqCst), 1);
    drop(engine2);
}
