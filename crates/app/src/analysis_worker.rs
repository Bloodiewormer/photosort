use crate::worker::Notify;
use catalog::Catalog;
use fsguard::DestPath;
use rules::{Control, RunProgress};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

#[derive(Debug)]
pub enum AnalysisEvent {
    Progress {
        analyzed: usize,
        pending: usize,
    },
    Finished {
        stopped: bool,
        analyzed: usize,
        suggestions: usize,
        errors: usize,
    },
    Failed(String),
}

/// Runs rule-based analysis in a dedicated background thread with its own SQLite connection.
pub struct AnalysisWorker {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<AnalysisEvent>,
    join: Option<JoinHandle<()>>,
}

impl AnalysisWorker {
    pub fn start(db: DestPath, notify: Option<Notify>) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_bg = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();

        let join = thread::spawn(move || {
            let emit = |ev: AnalysisEvent| {
                let _ = tx.send(ev);
                if let Some(cb) = &notify {
                    cb();
                }
            };

            let cat = match Catalog::open(&db) {
                Ok(c) => c,
                Err(e) => {
                    emit(AnalysisEvent::Failed(e.to_string()));
                    return;
                }
            };

            let res = rules::run_with(&cat, &mut |p: &RunProgress| {
                emit(AnalysisEvent::Progress {
                    analyzed: p.analyzed,
                    pending: p.pending,
                });
                if cancel_bg.load(Ordering::Relaxed) {
                    Control::Stop
                } else {
                    Control::Continue
                }
            });

            match res {
                Ok(report) => emit(AnalysisEvent::Finished {
                    stopped: report.stopped,
                    analyzed: report.analyzed,
                    suggestions: report.suggestions,
                    errors: report.errors.len(),
                }),
                Err(e) => emit(AnalysisEvent::Failed(e.to_string())),
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

    pub fn try_recv(&self) -> Option<AnalysisEvent> {
        self.rx.try_recv().ok()
    }

    pub fn recv(&self) -> Result<AnalysisEvent, mpsc::RecvError> {
        self.rx.recv()
    }
}

impl Drop for AnalysisWorker {
    fn drop(&mut self) {
        self.cancel();
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}
