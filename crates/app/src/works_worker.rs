use crate::worker::Notify;
use catalog::Catalog;
use fsguard::DestPath;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorksEvent {
    Progress {
        done: usize,
        total: usize,
        matched_assets: usize,
        suggestions: usize,
    },
    Finished {
        stopped: bool,
        processed: usize,
        matched_assets: usize,
        suggestions: usize,
    },
    Failed(String),
}

/// Runs work matching over all assets that already have OCR, on a dedicated thread with its own
/// SQLite connection.
pub struct WorksWorker {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<WorksEvent>,
    join: Option<JoinHandle<()>>,
}

impl WorksWorker {
    pub fn start(db: DestPath, notify: Option<Notify>) -> Self {
        Self::start_in_roots(db, None, notify)
    }

    pub fn start_in_roots(
        db: DestPath,
        roots: Option<Vec<PathBuf>>,
        notify: Option<Notify>,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_bg = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();

        let join = thread::spawn(move || {
            let emit = |ev: WorksEvent| {
                let _ = tx.send(ev);
                if let Some(cb) = &notify {
                    cb();
                }
            };

            let mut cat = match Catalog::open(&db) {
                Ok(c) => c,
                Err(e) => {
                    emit(WorksEvent::Failed(e.to_string()));
                    return;
                }
            };

            let filename_matched = match cat.apply_filename_works(roots.as_deref()) {
                Ok((_, matched)) => matched,
                Err(e) => {
                    emit(WorksEvent::Failed(e.to_string()));
                    return;
                }
            };

            let folder_matched = match cat.apply_folder_work_groups(roots.as_deref()) {
                Ok(matched) => matched,
                Err(e) => {
                    emit(WorksEvent::Failed(e.to_string()));
                    return;
                }
            };

            let initial_matched = filename_matched + folder_matched;

            let matcher = match cat.load_work_matcher() {
                Ok(m) => m,
                Err(e) => {
                    emit(WorksEvent::Failed(e.to_string()));
                    return;
                }
            };

            let assets = match cat.assets_with_ocr() {
                Ok(list) => list,
                Err(e) => {
                    emit(WorksEvent::Failed(e.to_string()));
                    return;
                }
            };

            let total = assets.len();
            if total == 0 {
                emit(WorksEvent::Finished {
                    stopped: false,
                    processed: 0,
                    matched_assets: initial_matched,
                    suggestions: initial_matched,
                });
                return;
            }

            let mut processed = 0usize;
            let mut matched_assets = initial_matched;
            let mut suggestions = initial_matched;
            let mut stopped = false;

            emit(WorksEvent::Progress {
                done: 0,
                total,
                matched_assets: 0,
                suggestions,
            });

            for (idx, asset) in assets.into_iter().enumerate() {
                if cancel_bg.load(Ordering::Relaxed) {
                    stopped = true;
                    break;
                }

                if let Some(r) = &roots {
                    let in_roots = if let Ok(Some(entry)) = cat.entry_for(asset) {
                        r.iter().any(|root| entry.path.starts_with(root))
                    } else {
                        false
                    };
                    if !in_roots {
                        continue;
                    }
                }

                match cat.apply_work_matches(asset, &matcher) {
                    Ok(res) => {
                        processed += 1;
                        let count = res.works.len()
                            + usize::from(res.promo_confidence.is_some())
                            + usize::from(res.credits_confidence.is_some())
                            + usize::from(res.recruitment_confidence.is_some());
                        if count > 0 {
                            matched_assets += 1;
                            suggestions += count;
                        }

                        if cat.is_promo_page(asset).unwrap_or(false)
                            && let Ok(Some(entry)) = cat.entry_for(asset)
                            && let Ok(img) = thumbs::decode(&entry.path, u32::MAX)
                        {
                            let lines = cat.ocr_lines(asset).unwrap_or_default();
                            let is_screenshot =
                                cat.labels_of(asset).unwrap_or_default().iter().any(|a| {
                                    a.kind == catalog::LabelKind::Category
                                        && a.name.eq_ignore_ascii_case(catalog::SCREENSHOT_CATEGORY)
                                        && a.status != catalog::Status::Rejected
                                });
                            let covers = crate::promo_covers::detect_and_associate(
                                asset,
                                &img,
                                &lines,
                                &matcher,
                                &cat,
                                is_screenshot,
                            );
                            let _ = cat.save_asset_covers(asset, &covers);
                        }
                    }
                    Err(e) => {
                        emit(WorksEvent::Failed(e.to_string()));
                        return;
                    }
                }

                // Emit progress regularly without flooding the channel on huge libraries.
                let done = idx + 1;
                if done == total || done % 16 == 0 {
                    emit(WorksEvent::Progress {
                        done,
                        total,
                        matched_assets,
                        suggestions,
                    });
                }
            }

            emit(WorksEvent::Finished {
                stopped,
                processed,
                matched_assets,
                suggestions,
            });
        });

        Self {
            cancel,
            rx,
            join: Some(join),
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn try_recv(&self) -> Option<WorksEvent> {
        self.rx.try_recv().ok()
    }

    pub fn recv(&self) -> Result<WorksEvent, mpsc::RecvError> {
        self.rx.recv()
    }
}

impl Drop for WorksWorker {
    fn drop(&mut self) {
        self.cancel();
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}
