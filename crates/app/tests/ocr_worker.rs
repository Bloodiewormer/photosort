// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::ocr_worker::{OcrEvent, OcrImageReader, OcrScope, OcrWorker, format_eta};
use app::settings::{OcrDevice, Settings, load, save};
use app::worker::{IndexEvent, IndexWorker};
use catalog::{AssetId, Catalog, Filter, OCR_STAGE, OCR_VERSION, OcrLine, OcrRecognizer, Sort};
use fsguard::{DestPath, LibraryRoot, ensure_writable};
use image::{ImageBuffer, Rgba};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

fn write_png(path: &Path, color: [u8; 4]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = ImageBuffer::from_pixel(16, 12, Rgba(color));
    img.save(path).unwrap();
}

struct Setup {
    _tmp: tempfile::TempDir,
    root_a: PathBuf,
    root_b: PathBuf,
    db_path: PathBuf,
    roots: Vec<LibraryRoot>,
}

impl Setup {
    fn db(&self) -> DestPath {
        ensure_writable(&self.db_path, &self.roots).unwrap()
    }

    fn asset(&self, root: &Path, rel: &str) -> AssetId {
        let cat = Catalog::open(&self.db()).unwrap();
        let path = dunce::canonicalize(root.join(rel)).unwrap();
        cat.asset_of(&path).unwrap().expect("indexed")
    }
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("lib_a");
    let dir_b = tmp.path().join("lib_b");

    write_png(&dir_a.join("one.png"), [255, 0, 0, 255]);
    write_png(&dir_a.join("two.png"), [0, 255, 0, 255]);
    write_png(&dir_a.join("three.png"), [0, 0, 255, 255]);
    write_png(&dir_b.join("four.png"), [255, 255, 0, 255]);

    let roots = vec![
        LibraryRoot::new(&dir_a).unwrap(),
        LibraryRoot::new(&dir_b).unwrap(),
    ];
    let root_a = roots[0].as_path().to_path_buf();
    let root_b = roots[1].as_path().to_path_buf();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("catalog.sqlite");
    let db = ensure_writable(&db_path, &roots).unwrap();

    let idx_roots = vec![
        LibraryRoot::new(&dir_a).unwrap(),
        LibraryRoot::new(&dir_b).unwrap(),
    ];
    let indexer = IndexWorker::start(db, idx_roots, None);
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

    Setup {
        _tmp: tmp,
        root_a,
        root_b,
        db_path,
        roots,
    }
}

enum Behavior {
    Lines(Vec<OcrLine>),
    Fail,
    Panic,
}

struct FakeReader {
    by_name: HashMap<String, Behavior>,
    calls: Arc<AtomicUsize>,
}

impl OcrImageReader for FakeReader {
    fn read_image(&self, path: &Path) -> Result<Vec<OcrLine>, String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        match self.by_name.get(&name) {
            Some(Behavior::Lines(lines)) => Ok(lines.clone()),
            Some(Behavior::Fail) => Err("decode error".into()),
            Some(Behavior::Panic) => panic!("simulated decoder panic"),
            None => Ok(Vec::new()),
        }
    }
}

#[test]
fn worker_saves_lines_handles_errors_and_panics_and_leaves_them_pending() {
    let s = setup();
    let a1 = s.asset(&s.root_a, "one.png");
    let a2 = s.asset(&s.root_a, "two.png");
    let a3 = s.asset(&s.root_a, "three.png");
    let b1 = s.asset(&s.root_b, "four.png");

    let mut by_name = HashMap::new();
    by_name.insert(
        "one.png".into(),
        Behavior::Lines(vec![OcrLine {
            text: "Crónicas del Relojero".into(),
            score: 0.92,
            recognizer: OcrRecognizer::V6,
            rect: [0.0, 0.0, 100.0, 20.0],
            top_strip: false,
        }]),
    );
    by_name.insert("two.png".into(), Behavior::Fail);
    by_name.insert("three.png".into(), Behavior::Panic);

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_bg = Arc::clone(&calls);

    // Restrict scope to root_a only (3 images: 1 ok, 1 error, 1 panic; root_b untouched)
    let worker = OcrWorker::start_with_loader(
        s.db(),
        OcrScope::Roots(vec![s.root_a.clone()]),
        move || {
            Ok(FakeReader {
                by_name,
                calls: calls_bg,
            })
        },
        None,
    );

    let mut saw_loading = false;
    let mut saved_assets = Vec::new();
    let mut finished = None;
    while let Ok(ev) = worker.recv() {
        match ev {
            OcrEvent::LoadingModel => saw_loading = true,
            OcrEvent::Progress { saved_asset, .. } => {
                if let Some(a) = saved_asset {
                    saved_assets.push(a);
                }
            }
            OcrEvent::Finished {
                stopped,
                processed,
                errors,
            } => {
                finished = Some((stopped, processed, errors));
                break;
            }
            OcrEvent::Failed(e) => panic!("unexpected worker failure: {e}"),
        }
    }

    assert!(saw_loading);
    assert_eq!(finished, Some((false, 1, 2)));
    assert_eq!(saved_assets, vec![a1]);
    assert_eq!(calls.load(Ordering::Relaxed), 3);

    let cat = Catalog::open(&s.db()).unwrap();
    assert!(cat.has_ocr_done(a1).unwrap());
    assert!(!cat.has_ocr_done(a2).unwrap());
    assert!(!cat.has_ocr_done(a3).unwrap());
    assert!(!cat.has_ocr_done(b1).unwrap());

    let found = cat
        .entries(
            &Filter {
                search: Some("relojero".into()),
                ..Default::default()
            },
            Sort::PathAsc,
        )
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].asset, a1);
}

#[test]
fn worker_cancel_stops_after_current_image_and_resumes_only_pending() {
    let s = setup();
    let a1 = s.asset(&s.root_a, "one.png");
    let a2 = s.asset(&s.root_a, "two.png");
    let a3 = s.asset(&s.root_a, "three.png");

    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();

    struct GateReader {
        started: mpsc::Sender<()>,
        release: std::sync::Mutex<mpsc::Receiver<()>>,
        calls: Arc<AtomicUsize>,
    }

    impl OcrImageReader for GateReader {
        fn read_image(&self, _path: &Path) -> Result<Vec<OcrLine>, String> {
            let n = self.calls.fetch_add(1, Ordering::Relaxed);
            if n == 0 {
                let _ = self.started.send(());
                let _ = self.release.lock().unwrap().recv();
            }
            Ok(vec![OcrLine {
                text: format!("Line {n}"),
                score: 0.9,
                recognizer: OcrRecognizer::V6,
                rect: [0.0, 0.0, 50.0, 10.0],
                top_strip: false,
            }])
        }
    }

    let first_calls = Arc::new(AtomicUsize::new(0));
    let first_calls_bg = Arc::clone(&first_calls);
    let worker = OcrWorker::start_with_loader(
        s.db(),
        OcrScope::Assets(vec![a1, a2, a3]),
        move || {
            Ok(GateReader {
                started: started_tx,
                release: std::sync::Mutex::new(release_rx),
                calls: first_calls_bg,
            })
        },
        None,
    );

    started_rx.recv().unwrap();
    worker.cancel();
    release_tx.send(()).unwrap();

    let mut first_result = None;
    while let Ok(ev) = worker.recv() {
        if let OcrEvent::Finished {
            stopped,
            processed,
            errors,
        } = ev
        {
            first_result = Some((stopped, processed, errors));
            break;
        }
    }
    assert_eq!(first_result, Some((true, 1, 0)));
    assert_eq!(first_calls.load(Ordering::Relaxed), 1);

    // Resume on the same scope: only the 2 remaining pending assets should be read.
    let second_calls = Arc::new(AtomicUsize::new(0));
    let second_calls_bg = Arc::clone(&second_calls);
    let worker2 = OcrWorker::start_with_loader(
        s.db(),
        OcrScope::Assets(vec![a1, a2, a3]),
        move || {
            Ok(FakeReader {
                by_name: HashMap::new(),
                calls: second_calls_bg,
            })
        },
        None,
    );

    let mut second_result = None;
    while let Ok(ev) = worker2.recv() {
        if let OcrEvent::Finished {
            stopped,
            processed,
            errors,
        } = ev
        {
            second_result = Some((stopped, processed, errors));
            break;
        }
    }
    assert_eq!(second_result, Some((false, 2, 0)));
    assert_eq!(second_calls.load(Ordering::Relaxed), 2);

    let cat = Catalog::open(&s.db()).unwrap();
    assert_eq!(
        cat.count_pending_in_assets(OCR_STAGE, OCR_VERSION, &[a1, a2, a3])
            .unwrap(),
        0
    );
}

#[test]
fn loader_failure_emits_failed_without_fallback() {
    let s = setup();
    let worker = OcrWorker::start_with_loader(
        s.db(),
        OcrScope::Roots(vec![s.root_a.clone()]),
        || -> Result<FakeReader, String> { Err("no DirectML adapter".into()) },
        None,
    );

    let mut err = None;
    while let Ok(ev) = worker.recv() {
        if let OcrEvent::Failed(msg) = ev {
            err = Some(msg);
            break;
        }
    }
    assert_eq!(err.as_deref(), Some("no DirectML adapter"));
}

#[test]
fn format_eta_only_returns_estimate_after_measured_progress() {
    assert_eq!(format_eta(0, 10, 5.0), None);
    assert_eq!(format_eta(10, 10, 5.0), None);
    assert_eq!(format_eta(2, 10, 0.0), None);
    assert_eq!(format_eta(2, 10, 2.0), Some("~8 s".into()));
    assert_eq!(format_eta(1, 10, 10.0), Some("~1 min 30 s".into()));
    assert_eq!(format_eta(1, 361, 10.0), Some("~1 h 0 min".into()));
}

#[test]
fn ocr_settings_default_to_directml_and_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();

    // Old settings without OCR fields load with defaults
    fs::write(
        data.join("settings.json"),
        br#"{"library_roots":[],"last_export_dir":null,"theme":"Dark"}"#,
    )
    .unwrap();
    let loaded = load(&data);
    assert!(loaded.warning.is_none());
    assert_eq!(loaded.settings.ocr_device, OcrDevice::DirectMl);
    assert_eq!(loaded.settings.ocr_models_dir, None);

    let mut updated = loaded.settings;
    updated.ocr_device = OcrDevice::Cpu;
    updated.ocr_models_dir = Some(PathBuf::from(r"C:\models\ocr"));
    save(&data, &updated).unwrap();

    let reloaded = load(&data);
    assert_eq!(reloaded.settings.ocr_device, OcrDevice::Cpu);
    assert_eq!(
        reloaded.settings.ocr_models_dir,
        Some(PathBuf::from(r"C:\models\ocr"))
    );
    assert_eq!(Settings::default().ocr_device, OcrDevice::DirectMl);
}
