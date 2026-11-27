use crate::worker::Notify;
use catalog::{Catalog, NearDuplicateProgress, SIGLIP2_MODEL_ID};
use fsguard::DestPath;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

#[derive(Debug, Clone, PartialEq)]
pub enum NearDupesEvent {
    Progress {
        done_pairs: u64,
        total_pairs: u64,
    },
    Finished {
        stopped: bool,
        group_count: usize,
        threshold: f32,
    },
    Failed(String),
}

/// Runs multi-threaded all-pairs near-duplicate detection over persisted SigLIP 2 embeddings
/// on a dedicated background thread with its own SQLite connection.
pub struct NearDupesWorker {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<NearDupesEvent>,
    join: Option<JoinHandle<()>>,
}

impl NearDupesWorker {
    pub fn start(
        db: DestPath,
        roots: Vec<PathBuf>,
        threshold: f32,
        notify: Option<Notify>,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_bg = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();

        let join = thread::spawn(move || {
            let emit = |ev: NearDupesEvent| {
                let _ = tx.send(ev);
                if let Some(cb) = &notify {
                    cb();
                }
            };

            let mut cat = match Catalog::open(&db) {
                Ok(c) => c,
                Err(e) => {
                    emit(NearDupesEvent::Failed(e.to_string()));
                    return;
                }
            };

            let tx_progress = tx.clone();
            let notify_progress = notify.clone();
            let on_progress = move |p: NearDuplicateProgress| {
                let _ = tx_progress.send(NearDupesEvent::Progress {
                    done_pairs: p.done_pairs,
                    total_pairs: p.total_pairs,
                });
                if let Some(cb) = &notify_progress {
                    cb();
                }
            };

            match cat.compute_and_save_near_duplicates_in_roots(
                SIGLIP2_MODEL_ID,
                &roots,
                threshold,
                &cancel_bg,
                &on_progress,
            ) {
                Ok(Some(groups)) => {
                    emit(NearDupesEvent::Finished {
                        stopped: false,
                        group_count: groups.len(),
                        threshold,
                    });
                }
                Ok(None) => {
                    emit(NearDupesEvent::Finished {
                        stopped: true,
                        group_count: 0,
                        threshold,
                    });
                }
                Err(e) => {
                    emit(NearDupesEvent::Failed(e.to_string()));
                }
            }
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

    pub fn try_recv(&self) -> Option<NearDupesEvent> {
        self.rx.try_recv().ok()
    }
}

impl Drop for NearDupesWorker {
    fn drop(&mut self) {
        self.cancel();
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}
