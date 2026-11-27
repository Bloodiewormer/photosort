// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::vision_worker::{VisionEvent, VisionImageOutput, VisionReader, VisionScope, VisionWorker};
use app::worker::{IndexEvent, IndexWorker};
use catalog::{Catalog, DetectedFace, EMBEDDING_DIM, VISION_STAGE, VISION_VERSION};
use fsguard::{LibraryRoot, ensure_writable};
use image::{ImageBuffer, Rgb, RgbImage};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn write_png(path: &Path, color: [u8; 3]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img: RgbImage = ImageBuffer::from_pixel(32, 32, Rgb(color));
    img.save(path).unwrap();
}

struct FakeVisionReader {
    batch: usize,
    panic_on_red: bool,
    calls: Arc<AtomicUsize>,
}

impl VisionReader for FakeVisionReader {
    fn batch_size(&self) -> usize {
        self.batch
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

    fn analyze_images(&mut self, images: &[&RgbImage]) -> Result<Vec<VisionImageOutput>, String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let mut out = Vec::with_capacity(images.len());
        for img in images {
            let px = img.get_pixel(0, 0);
            if self.panic_on_red && px[0] > 200 && px[1] < 50 {
                panic!("simulated panic on red image");
            }
            let mut emb = vec![0.0f32; EMBEDDING_DIM];
            emb[0] = 1.0;
            out.push(VisionImageOutput {
                embedding: emb,
                faces: vec![DetectedFace::new([4.0, 4.0, 20.0, 20.0], 0.9)],
            });
        }
        Ok(out)
    }
}

#[test]
fn vision_worker_isolates_failing_and_panicking_images_within_a_batch() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    write_png(&library.join("01_good.png"), [20, 180, 40]);
    write_png(&library.join("02_panic_red.png"), [240, 10, 10]);
    write_png(&library.join("03_good.png"), [30, 60, 220]);
    fs::write(library.join("04_corrupt.jpg"), b"not an image").unwrap();

    let root = LibraryRoot::new(&library).unwrap();
    let root_path = root.as_path().to_path_buf();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_idx =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let db_vis =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let db_chk =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();

    let idx = IndexWorker::start(db_idx, vec![root], None);
    while let Ok(ev) = idx.recv() {
        if let IndexEvent::Finished { .. } = ev {
            break;
        }
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let calls_bg = Arc::clone(&calls);
    let worker = VisionWorker::start_with_loader(
        db_vis,
        VisionScope::Roots(vec![root_path.clone()]),
        move || {
            Ok(FakeVisionReader {
                batch: 4,
                panic_on_red: true,
                calls: calls_bg,
            })
        },
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
            VisionEvent::Failed(e) => panic!("unexpected failure: {e}"),
            _ => {}
        }
    }

    // 2 healthy images succeeded despite corrupt decode and panicked batch mate; 2 stayed pending.
    assert_eq!(finished, Some((false, 2, 2)));
    let cat = Catalog::open(&db_chk).unwrap();
    assert_eq!(
        cat.count_pending_in_root(VISION_STAGE, VISION_VERSION, &root_path)
            .unwrap(),
        2
    );
}

#[test]
fn vision_worker_cancel_and_resume_finishes_remaining_only() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    for i in 0..5 {
        write_png(
            &library.join(format!("img_{i:02}.png")),
            [20, 120 + i * 10, 200],
        );
    }

    let root = LibraryRoot::new(&library).unwrap();
    let root_path = root.as_path().to_path_buf();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_idx =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let db_run1 =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let db_run2 =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let db_chk =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();

    let idx = IndexWorker::start(db_idx, vec![root], None);
    while let Ok(ev) = idx.recv() {
        if let IndexEvent::Finished { .. } = ev {
            break;
        }
    }

    // Run 1 (batch 1): cancel right after first saved asset.
    let worker1 = VisionWorker::start_with_loader(
        db_run1,
        VisionScope::Roots(vec![root_path.clone()]),
        || {
            Ok(FakeVisionReader {
                batch: 1,
                panic_on_red: false,
                calls: Arc::new(AtomicUsize::new(0)),
            })
        },
        None,
    );
    let mut processed1 = 0usize;
    let mut stopped1 = false;
    while let Ok(ev) = worker1.recv() {
        match ev {
            VisionEvent::Progress {
                saved_asset: Some(_),
                ..
            } => {
                worker1.cancel();
            }
            VisionEvent::Finished {
                stopped, processed, ..
            } => {
                stopped1 = stopped;
                processed1 = processed;
                break;
            }
            VisionEvent::Failed(e) => panic!("run 1 failed: {e}"),
            _ => {}
        }
    }
    assert!(stopped1);
    assert!((1..5).contains(&processed1));

    // Run 2: resumes and finishes only the remaining assets.
    let worker2 = VisionWorker::start_with_loader(
        db_run2,
        VisionScope::Roots(vec![root_path.clone()]),
        || {
            Ok(FakeVisionReader {
                batch: 1,
                panic_on_red: false,
                calls: Arc::new(AtomicUsize::new(0)),
            })
        },
        None,
    );
    let mut processed2 = 0usize;
    while let Ok(ev) = worker2.recv() {
        match ev {
            VisionEvent::Finished {
                stopped,
                processed,
                errors,
            } => {
                assert!(!stopped);
                assert_eq!(errors, 0);
                processed2 = processed;
                break;
            }
            VisionEvent::Failed(e) => panic!("run 2 failed: {e}"),
            _ => {}
        }
    }
    assert_eq!(processed1 + processed2, 5);
    let cat = Catalog::open(&db_chk).unwrap();
    assert_eq!(
        cat.count_pending_in_root(VISION_STAGE, VISION_VERSION, &root_path)
            .unwrap(),
        0
    );
}
