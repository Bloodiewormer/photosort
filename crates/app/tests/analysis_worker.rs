// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::analysis_worker::{AnalysisEvent, AnalysisWorker};
use catalog::Catalog;
use fsguard::{LibraryRoot, ensure_writable};
use image::{ImageBuffer, Rgba};
use std::fs;
use std::path::Path;

fn write_png(path: &Path, w: u32, h: u32, color: [u8; 4]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let img = ImageBuffer::from_pixel(w, h, Rgba(color));
    img.save(path).unwrap();
}

#[test]
fn analysis_worker_analyzes_pending_images_reports_progress_and_shares_wal_db() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    write_png(&lib.join("Screenshot_01.png"), 64, 64, [255, 0, 0, 255]);
    write_png(
        &lib.join("IMG-20240501-WA0001.png"),
        32,
        32,
        [0, 255, 0, 255],
    );
    write_png(&lib.join("plain.png"), 40, 40, [0, 0, 255, 255]);

    let root = LibraryRoot::new(&lib).unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_ui = ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let db_worker =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();

    let mut ui_cat = Catalog::open(&db_ui).unwrap();
    ui_cat.index(&root).unwrap();

    let worker = AnalysisWorker::start(db_worker, None);
    let mut progress_seen = Vec::new();
    let mut finished = None;
    while let Ok(ev) = worker.recv() {
        match ev {
            AnalysisEvent::Progress { analyzed, pending } => {
                progress_seen.push((analyzed, pending));
            }
            AnalysisEvent::Finished {
                stopped,
                analyzed,
                suggestions,
                errors,
            } => {
                finished = Some((stopped, analyzed, suggestions, errors));
                break;
            }
            AnalysisEvent::Failed(e) => panic!("unexpected analysis failure: {e}"),
        }
    }

    assert_eq!(finished, Some((false, 3, 2, 0)));
    assert_eq!(progress_seen.first(), Some(&(0, 3)));
    assert_eq!(progress_seen.last(), Some(&(3, 0)));
    assert!(
        progress_seen
            .windows(2)
            .all(|w| w[0].0 <= w[1].0 && w[0].1 >= w[1].1)
    );
    assert_eq!(ui_cat.labels_in_use().unwrap().len(), 2);
}

#[test]
fn analysis_worker_honours_cancel_and_later_run_finishes_remaining() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    for i in 0..8 {
        write_png(
            &lib.join(format!("Screenshot_{i:02}.png")),
            16,
            16,
            [i as u8 * 20, 40, 80, 255],
        );
    }

    let root = LibraryRoot::new(&lib).unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_init =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let db_first =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let db_second =
        ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();

    let mut cat = Catalog::open(&db_init).unwrap();
    cat.index(&root).unwrap();

    let worker = AnalysisWorker::start(db_first, None);
    let mut cancelled = false;
    let mut first_analyzed = 0usize;
    let mut stopped_flag = false;
    while let Ok(ev) = worker.recv() {
        match ev {
            AnalysisEvent::Progress { analyzed, .. } if !cancelled && analyzed >= 1 => {
                worker.cancel();
                cancelled = true;
            }
            AnalysisEvent::Finished {
                stopped, analyzed, ..
            } => {
                stopped_flag = stopped;
                first_analyzed = analyzed;
                break;
            }
            AnalysisEvent::Failed(e) => panic!("unexpected failure: {e}"),
            _ => {}
        }
    }

    assert!(cancelled);
    assert!(stopped_flag);
    assert!(first_analyzed >= 1);

    // Running again finishes whatever remained without duplicating work.
    let worker2 = AnalysisWorker::start(db_second, None);
    let mut second_analyzed = 0usize;
    let mut second_stopped = true;
    while let Ok(ev) = worker2.recv() {
        match ev {
            AnalysisEvent::Finished {
                stopped, analyzed, ..
            } => {
                second_stopped = stopped;
                second_analyzed = analyzed;
                break;
            }
            AnalysisEvent::Failed(e) => panic!("unexpected failure: {e}"),
            _ => {}
        }
    }

    assert!(!second_stopped);
    assert_eq!(first_analyzed + second_analyzed, 8);
    assert!(cat.pending("rules", 1, 100).unwrap().is_empty());
}
