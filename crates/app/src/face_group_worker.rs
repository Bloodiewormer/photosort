use crate::settings::OcrDevice;
use crate::vision_worker::{VISION_DECODE_MAX_PX, session_options};
use crate::worker::Notify;
use catalog::{AssetId, Catalog, SFACE_SIMILARITY_THRESHOLD};
use fsguard::DestPath;
use image::RgbImage;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaceGroupEvent {
    /// Photos handled so far, out of those with faces pending.
    Progress {
        done: usize,
        total: usize,
    },
    Finished {
        stopped: bool,
        total: usize,
        grouped_faces: usize,
        grouped_photos: usize,
        unreadable_photos: usize,
    },
    Failed(String),
}

/// Computes face embeddings and groups them into persons on a dedicated background thread with
/// its own SQLite connection. Originals are only decoded in memory.
pub struct FaceGroupWorker {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<FaceGroupEvent>,
    join: Option<JoinHandle<()>>,
}

impl FaceGroupWorker {
    pub fn start(
        db: DestPath,
        models_dir: PathBuf,
        device: OcrDevice,
        notify: Option<Notify>,
    ) -> Self {
        let load = move || {
            if !models_dir.join(vision::manifest::SFACE_MODEL).is_file() {
                return Err("falta el modelo SFace en la carpeta de modelos de visión".to_string());
            }
            let mut sface = vision::SFace::load(&models_dir, &session_options(device))?;
            Ok(move |img: &RgbImage, landmarks: &[[f32; 2]; 5]| {
                sface.feature(&vision::align_face(img, landmarks))
            })
        };
        Self::start_with_loader(db, load, notify)
    }

    /// `load` runs on the worker thread, only when there is pending work, and returns the
    /// function that computes the embedding of one face. Tests inject a fake one.
    pub fn start_with_loader<L, E>(db: DestPath, load: L, notify: Option<Notify>) -> Self
    where
        L: FnOnce() -> Result<E, String> + Send + 'static,
        E: FnMut(&RgbImage, &[[f32; 2]; 5]) -> Result<[f32; 128], String>,
    {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_bg = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();

        let join = thread::spawn(move || {
            let emit = |ev: FaceGroupEvent| {
                let _ = tx.send(ev);
                if let Some(cb) = &notify {
                    cb();
                }
            };
            let last = match run(&db, load, &cancel_bg, &emit) {
                Ok(finished) => finished,
                Err(e) => FaceGroupEvent::Failed(e),
            };
            emit(last);
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

    pub fn try_recv(&self) -> Option<FaceGroupEvent> {
        self.rx.try_recv().ok()
    }

    pub fn recv(&self) -> Result<FaceGroupEvent, mpsc::RecvError> {
        self.rx.recv()
    }
}

impl Drop for FaceGroupWorker {
    fn drop(&mut self) {
        self.cancel();
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

fn run<L, E>(
    db: &DestPath,
    load: L,
    cancel: &AtomicBool,
    emit: &dyn Fn(FaceGroupEvent),
) -> Result<FaceGroupEvent, String>
where
    L: FnOnce() -> Result<E, String>,
    E: FnMut(&RgbImage, &[[f32; 2]; 5]) -> Result<[f32; 128], String>,
{
    let mut cat = Catalog::open(db).map_err(|e| e.to_string())?;
    let assets: Vec<AssetId> = cat
        .assets_with_faces_needing_embeddings()
        .map_err(|e| format!("no se pudieron buscar rostros pendientes: {e}"))?;
    let total = assets.len();

    let mut stopped = false;
    let mut grouped_faces = 0;
    let mut grouped_photos = 0;
    let mut unreadable_photos = 0;

    if total > 0 {
        emit(FaceGroupEvent::Progress { done: 0, total });
        let mut embed = load().map_err(|e| format!("no se pudo cargar el modelo SFace: {e}"))?;

        for (index, asset) in assets.into_iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                stopped = true;
                break;
            }

            let pending: Vec<_> = cat
                .asset_face_details(asset)
                .unwrap_or_default()
                .into_iter()
                .filter(|f| !f.has_embedding && catalog::is_groupable_face(f.score, f.bbox))
                .collect();

            if !pending.is_empty()
                && let Ok(Some(entry)) = cat.entry_for(asset)
            {
                match thumbs::decode(&entry.path, VISION_DECODE_MAX_PX) {
                    Ok(img) => {
                        let img = img.to_rgb8();
                        for face in pending {
                            let Ok(embedding) = embed(&img, &face.landmarks) else {
                                continue;
                            };
                            if cat
                                .group_face(face.id, &embedding, SFACE_SIMILARITY_THRESHOLD)
                                .is_ok()
                            {
                                grouped_faces += 1;
                            }
                        }
                        grouped_photos += 1;
                    }
                    Err(_) => unreadable_photos += 1,
                }
            }

            emit(FaceGroupEvent::Progress {
                done: index + 1,
                total,
            });
        }
    }

    Ok(FaceGroupEvent::Finished {
        stopped,
        total,
        grouped_faces,
        grouped_photos,
        unreadable_photos,
    })
}
