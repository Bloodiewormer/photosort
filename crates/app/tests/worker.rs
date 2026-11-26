// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::worker::{
    IndexEvent, IndexWorker, NoThumbnails, PreviewWorker, ThumbPool, ThumbRequest,
    ThumbnailProvider,
};
use catalog::Catalog;
use fsguard::{LibraryRoot, ensure_writable};
use image::{ImageBuffer, Rgba};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

fn write_png(path: &Path, w: u32, h: u32, color: [u8; 4]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = ImageBuffer::from_pixel(w, h, Rgba(color));
    img.save(path).unwrap();
}

#[test]
fn index_worker_indexes_multiple_roots_reports_progress_and_shares_wal_db() {
    let tmp = tempfile::tempdir().unwrap();
    let lib1 = tmp.path().join("lib1");
    let lib2 = tmp.path().join("lib2");
    write_png(&lib1.join("a.png"), 4, 4, [255, 0, 0, 255]);
    write_png(&lib1.join("b.png"), 4, 4, [0, 255, 0, 255]);
    write_png(&lib2.join("c.png"), 4, 4, [0, 0, 255, 255]);

    let r1 = LibraryRoot::new(&lib1).unwrap();
    let r2 = LibraryRoot::new(&lib2).unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_ui = ensure_writable(&data.join("catalog.sqlite"), &[r1, r2]).unwrap();
    let r1 = LibraryRoot::new(&lib1).unwrap();
    let r2 = LibraryRoot::new(&lib2).unwrap();
    let db_worker = ensure_writable(&data.join("catalog.sqlite"), &[r1, r2]).unwrap();
    let r1 = LibraryRoot::new(&lib1).unwrap();
    let r2 = LibraryRoot::new(&lib2).unwrap();

    // Open UI connection BEFORE worker starts so both connections share WAL concurrently.
    let ui_cat = Catalog::open(&db_ui).unwrap();
    let worker = IndexWorker::start(db_worker, vec![r1, r2], None);

    let mut progress_seen = Vec::new();
    let mut finished = None;
    while let Ok(ev) = worker.recv() {
        match ev {
            IndexEvent::Progress {
                root_index,
                done,
                total,
                ..
            } => progress_seen.push((root_index, done, total)),
            IndexEvent::Finished {
                stopped,
                new,
                errors,
                ..
            } => {
                finished = Some((stopped, new, errors));
                break;
            }
            IndexEvent::Failed(e) => panic!("unexpected index failure: {e}"),
            IndexEvent::RootDone { .. } => {}
        }
    }

    assert_eq!(finished, Some((false, 3, 0)));
    assert!(progress_seen.contains(&(0, 2, 2)));
    assert!(progress_seen.contains(&(1, 1, 1)));
    assert_eq!(ui_cat.counts().unwrap().locations, 3);
}

#[test]
fn index_worker_honours_cancel_and_keeps_committed_work() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    for i in 0..8 {
        write_png(
            &lib.join(format!("img_{i:02}.png")),
            4,
            4,
            [i as u8 * 20, 10, 10, 255],
        );
    }
    let root = LibraryRoot::new(&lib).unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db = ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let db_check =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();

    let worker = IndexWorker::start(db, vec![root], None);
    let mut cancelled = false;
    let mut stopped_flag = false;
    while let Ok(ev) = worker.recv() {
        match ev {
            IndexEvent::Progress { done, .. } if !cancelled && done >= 1 => {
                worker.cancel();
                cancelled = true;
            }
            IndexEvent::Finished { stopped, .. } => {
                stopped_flag = stopped;
                break;
            }
            IndexEvent::Failed(e) => panic!("unexpected failure: {e}"),
            _ => {}
        }
    }

    assert!(cancelled);
    assert!(stopped_flag);
    let cat = Catalog::open(&db_check).unwrap();
    let counts = cat.counts().unwrap();
    assert!(counts.locations >= 1);
    assert_eq!(counts.missing, 0);
}

/// Deterministic gated provider: blocks on `block_key` until `gate_rx` receives a token,
/// recording the exact order in which keys are processed.
struct GatedProvider {
    thumb_file: PathBuf,
    block_key: String,
    entered_tx: Mutex<Option<mpsc::SyncSender<()>>>,
    gate_rx: Mutex<Option<mpsc::Receiver<()>>>,
    order: Arc<Mutex<Vec<String>>>,
}

impl ThumbnailProvider for GatedProvider {
    fn get(&self, _source: &Path, sha256: &str) -> io::Result<PathBuf> {
        self.order.lock().unwrap().push(sha256.to_owned());
        if sha256 == self.block_key {
            if let Some(tx) = self.entered_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
            if let Some(rx) = self.gate_rx.lock().unwrap().take() {
                let _ = rx.recv();
            }
        }
        Ok(self.thumb_file.clone())
    }
}

#[test]
fn thumb_pool_prioritises_most_recently_requested_and_skips_offscreen() {
    let tmp = tempfile::tempdir().unwrap();
    let thumb = tmp.path().join("thumb.png");
    write_png(&thumb, 8, 6, [10, 20, 30, 255]);

    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (gate_tx, gate_rx) = mpsc::sync_channel(1);
    let order = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(GatedProvider {
        thumb_file: thumb,
        block_key: "first".into(),
        entered_tx: Mutex::new(Some(entered_tx)),
        gate_rx: Mutex::new(Some(gate_rx)),
        order: Arc::clone(&order),
    });

    // Single worker thread guarantees deterministic scheduling after `first` unblocks.
    let pool = ThumbPool::new(1, provider, None);
    pool.request(ThumbRequest {
        source: PathBuf::from("first.png"),
        sha256: "first".into(),
    });
    // Wait deterministically until the worker has popped `first` and blocked inside `get`.
    entered_rx.recv().unwrap();

    // Queue three more items while worker is busy on `first`.
    for key in ["offscreen", "older", "newest"] {
        pool.request(ThumbRequest {
            source: PathBuf::from(format!("{key}.png")),
            sha256: key.into(),
        });
    }
    // Mark only `first`, `older`, `newest` as visible: `offscreen` must be skipped completely.
    let visible: HashSet<String> = ["first", "older", "newest"]
        .into_iter()
        .map(String::from)
        .collect();
    pool.set_visible(&visible);

    // Unblock `first`.
    gate_tx.send(()).unwrap();

    let mut results = Vec::new();
    for _ in 0..3 {
        let r = pool.recv().unwrap();
        let img = r.image.expect("valid png");
        assert_eq!(img.size, [8, 6]);
        results.push(r.sha256);
    }

    assert_eq!(results, vec!["first", "newest", "older"]);
    assert_eq!(*order.lock().unwrap(), vec!["first", "newest", "older"]);
}

#[test]
fn thumb_pool_cancel_all_drops_pending_work_and_no_thumbnails_reports_error() {
    let tmp = tempfile::tempdir().unwrap();
    let thumb = tmp.path().join("thumb.png");
    write_png(&thumb, 4, 4, [1, 2, 3, 255]);

    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (gate_tx, gate_rx) = mpsc::sync_channel(1);
    let order = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(GatedProvider {
        thumb_file: thumb,
        block_key: "busy".into(),
        entered_tx: Mutex::new(Some(entered_tx)),
        gate_rx: Mutex::new(Some(gate_rx)),
        order: Arc::clone(&order),
    });

    let pool = ThumbPool::new(1, provider, None);
    pool.request(ThumbRequest {
        source: PathBuf::from("busy.png"),
        sha256: "busy".into(),
    });
    entered_rx.recv().unwrap();
    pool.request(ThumbRequest {
        source: PathBuf::from("dropped.png"),
        sha256: "dropped".into(),
    });
    pool.cancel_all();
    gate_tx.send(()).unwrap();

    // Drop joins worker threads deterministically before we assert `dropped` was never called.
    drop(pool);
    assert_eq!(*order.lock().unwrap(), vec!["busy"]);

    // NoThumbnails returns an Err marker cleanly without panicking.
    let no_pool = ThumbPool::new(1, Arc::new(NoThumbnails), None);
    no_pool.request(ThumbRequest {
        source: PathBuf::from("x.png"),
        sha256: "abc".into(),
    });
    let res = no_pool.recv().unwrap();
    assert_eq!(res.sha256, "abc");
    assert!(res.image.is_err());
}

#[test]
fn preview_worker_decodes_and_downscales_read_only_original() {
    let tmp = tempfile::tempdir().unwrap();
    let big = tmp.path().join("big.png");
    write_png(&big, 200, 100, [100, 150, 200, 255]);

    let worker = PreviewWorker::new(50, None);
    worker.request(big.clone());
    let res = worker.recv().unwrap();
    assert_eq!(res.path, big);
    let img = res.image.expect("decoded preview");
    assert_eq!(img.size, [50, 25]);
}
