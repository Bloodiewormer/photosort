use crate::settings::OcrDevice;
use crate::worker::Notify;
use catalog::{AssetId, Catalog, OCR_STAGE, OCR_VERSION, OcrLine, OcrRecognizer, is_top_strip};
use fsguard::DestPath;
use ocr::engine::{Engine, Options, Provider};
use ocr::eval::chosen;
use ocr::select::{RECOMMENDED, Source, Strategy, choose};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrScope {
    Assets(Vec<AssetId>),
    Roots(Vec<PathBuf>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum OcrEvent {
    LoadingModel,
    Progress {
        done: usize,
        total: usize,
        errors: usize,
        elapsed_secs: f64,
        saved_asset: Option<AssetId>,
    },
    Finished {
        stopped: bool,
        processed: usize,
        errors: usize,
    },
    Failed(String),
}

/// Injectable image reader so the OCR worker is fully testable without ONNX models.
pub trait OcrImageReader: Send + 'static {
    fn read_image(&self, path: &Path) -> Result<Vec<OcrLine>, String>;
}

/// Maps the saved UI device preference to engine options (DirectML batch 6, CPU batch 1).
pub fn engine_options(device: OcrDevice) -> Options {
    match device {
        OcrDevice::DirectMl => Options {
            provider: Provider::DirectMl { device: 0 },
            rec_batch: 6,
            ..Options::default()
        },
        OcrDevice::Cpu => Options {
            provider: Provider::Cpu,
            rec_batch: 1,
            ..Options::default()
        },
    }
}

/// Real OCR reader backed by `ocr::engine::Engine` and `thumbs::decode`.
pub struct EngineOcrReader {
    engine: Engine,
}

impl EngineOcrReader {
    pub fn load(models_dir: &Path, device: OcrDevice) -> Result<Self, String> {
        let options = engine_options(device);
        let engine = Engine::load(models_dir, &options)?;
        Ok(Self { engine })
    }
}

impl OcrImageReader for EngineOcrReader {
    fn read_image(&self, path: &Path) -> Result<Vec<OcrLine>, String> {
        // Error messages must never echo file paths or private data.
        let pixels = thumbs::decode(path, u32::MAX)
            .map_err(|_| "no se pudo decodificar la imagen".to_string())?
            .to_rgb8();
        let (width, height) = (pixels.width(), pixels.height());
        let (raw_lines, _) = self.engine.read(&pixels, |_| true, &mut Vec::new())?;
        let strategy = Strategy::Both(RECOMMENDED);
        let mut out = Vec::new();
        for line in &raw_lines {
            let reading = chosen(strategy, line);
            if reading.text.trim().is_empty() {
                continue;
            }
            let recognizer = match choose(strategy, &line.v6, line.korean.as_ref()) {
                Source::V6 => OcrRecognizer::V6,
                Source::Korean => OcrRecognizer::Korean,
            };
            out.push(OcrLine {
                text: reading.text.clone(),
                score: reading.score,
                recognizer,
                rect: line.rect,
                top_strip: is_top_strip(width, height, line.rect),
            });
        }
        Ok(out)
    }
}

/// Formats a remaining-time estimate from the measured speed of the current run.
/// Returns `None` until at least one image has been processed.
pub fn format_eta(done: usize, total: usize, elapsed_secs: f64) -> Option<String> {
    if done == 0 || done >= total || !elapsed_secs.is_finite() || elapsed_secs <= 0.0 {
        return None;
    }
    let remaining = total - done;
    let secs = ((elapsed_secs / done as f64) * remaining as f64).round() as u64;
    if secs < 60 {
        Some(format!("~{secs} s"))
    } else if secs < 3600 {
        let mins = secs / 60;
        let rem_s = secs % 60;
        Some(format!("~{mins} min {rem_s} s"))
    } else {
        let hours = secs / 3600;
        let rem_m = (secs % 3600) / 60;
        Some(format!("~{hours} h {rem_m} min"))
    }
}

/// Runs OCR in a dedicated background thread with its own SQLite connection.
pub struct OcrWorker {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<OcrEvent>,
    join: Option<JoinHandle<()>>,
}

impl OcrWorker {
    pub fn start(
        db: DestPath,
        scope: OcrScope,
        models_dir: PathBuf,
        device: OcrDevice,
        notify: Option<Notify>,
    ) -> Self {
        Self::start_with_loader(
            db,
            scope,
            move || EngineOcrReader::load(&models_dir, device),
            notify,
        )
    }

    pub fn start_with_loader<R, F>(
        db: DestPath,
        scope: OcrScope,
        loader: F,
        notify: Option<Notify>,
    ) -> Self
    where
        R: OcrImageReader,
        F: FnOnce() -> Result<R, String> + Send + 'static,
    {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_bg = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();

        let join = thread::spawn(move || {
            let emit = |ev: OcrEvent| {
                let _ = tx.send(ev);
                if let Some(cb) = &notify {
                    cb();
                }
            };

            let mut cat = match Catalog::open(&db) {
                Ok(c) => c,
                Err(e) => {
                    emit(OcrEvent::Failed(e.to_string()));
                    return;
                }
            };

            let pending = match &scope {
                OcrScope::Assets(ids) => {
                    cat.pending_in_assets(OCR_STAGE, OCR_VERSION, ids, usize::MAX)
                }
                OcrScope::Roots(roots) => {
                    cat.pending_in_roots(OCR_STAGE, OCR_VERSION, roots, usize::MAX)
                }
            };
            let pending = match pending {
                Ok(list) => list,
                Err(e) => {
                    emit(OcrEvent::Failed(e.to_string()));
                    return;
                }
            };

            let total = pending.len();
            if total == 0 {
                emit(OcrEvent::Finished {
                    stopped: false,
                    processed: 0,
                    errors: 0,
                });
                return;
            }

            if cancel_bg.load(Ordering::Relaxed) {
                emit(OcrEvent::Finished {
                    stopped: true,
                    processed: 0,
                    errors: 0,
                });
                return;
            }

            emit(OcrEvent::LoadingModel);
            let reader = match loader() {
                Ok(r) => r,
                Err(e) => {
                    emit(OcrEvent::Failed(e));
                    return;
                }
            };

            let started = Instant::now();
            let mut processed = 0usize;
            let mut errors = 0usize;
            let mut stopped = false;

            emit(OcrEvent::Progress {
                done: 0,
                total,
                errors: 0,
                elapsed_secs: 0.0,
                saved_asset: None,
            });

            for (idx, item) in pending.into_iter().enumerate() {
                if cancel_bg.load(Ordering::Relaxed) {
                    stopped = true;
                    break;
                }

                // A failing or panicking image is counted and stays pending without stopping the batch.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    reader.read_image(&item.path)
                }))
                .unwrap_or_else(|_| Err("error interno al leer la imagen".into()));

                let saved_asset = match outcome {
                    Ok(lines) => match cat.save_ocr_lines(item.asset, &lines) {
                        Ok(()) => {
                            processed += 1;
                            Some(item.asset)
                        }
                        Err(e) => {
                            emit(OcrEvent::Failed(e.to_string()));
                            return;
                        }
                    },
                    Err(_) => {
                        errors += 1;
                        None
                    }
                };

                emit(OcrEvent::Progress {
                    done: idx + 1,
                    total,
                    errors,
                    elapsed_secs: started.elapsed().as_secs_f64(),
                    saved_asset,
                });
            }

            emit(OcrEvent::Finished {
                stopped,
                processed,
                errors,
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

    pub fn try_recv(&self) -> Option<OcrEvent> {
        self.rx.try_recv().ok()
    }

    pub fn recv(&self) -> Result<OcrEvent, mpsc::RecvError> {
        self.rx.recv()
    }
}

impl Drop for OcrWorker {
    fn drop(&mut self) {
        self.cancel();
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}
