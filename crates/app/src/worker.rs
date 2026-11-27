use catalog::{Catalog, Control, IndexReport, Progress};
use eframe::egui::ColorImage;
use fsguard::{DestPath, LibraryRoot};
use image::ImageReader;
use image::imageops::FilterType;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// Supplies a cached thumbnail image file on disk for an indexed asset.
pub trait ThumbnailProvider: Send + Sync {
    fn get(&self, source: &Path, sha256: &str) -> io::Result<PathBuf>;
}

/// Placeholder provider used in tests and until the thumbnail cache crate is wired in.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoThumbnails;

impl ThumbnailProvider for NoThumbnails {
    fn get(&self, _source: &Path, _sha256: &str) -> io::Result<PathBuf> {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "thumbnail provider not configured",
        ))
    }
}

/// Decodes an image file opened strictly read-only, optionally downscaling its longest edge.
pub fn decode_image_file(path: &Path, max_edge: Option<u32>) -> Result<ColorImage, String> {
    if thumbs::is_video_path(path) {
        let frame = thumbs::extract_frame(path, 0.1, max_edge.unwrap_or(u32::MAX))
            .map_err(|e| e.to_string())?;
        let rgba = image::DynamicImage::ImageRgb8(frame).to_rgba8();
        return Ok(ColorImage::from_rgba_unmultiplied(
            [rgba.width() as usize, rgba.height() as usize],
            rgba.as_raw(),
        ));
    }
    let file = File::open(path).map_err(|e| e.to_string())?;
    let reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut img = reader.decode().map_err(|e| e.to_string())?;
    if let Some(limit) = max_edge
        && limit > 0
        && (img.width() > limit || img.height() > limit)
    {
        img = img.resize(limit, limit, FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    Ok(ColorImage::from_rgba_unmultiplied(
        [rgba.width() as usize, rgba.height() as usize],
        rgba.as_raw(),
    ))
}

#[derive(Debug)]
pub enum IndexEvent {
    Progress {
        root_index: usize,
        root_count: usize,
        root_path: PathBuf,
        done: usize,
        total: usize,
    },
    RootDone {
        root_path: PathBuf,
        report: IndexReport,
    },
    Finished {
        stopped: bool,
        new: usize,
        changed: usize,
        unchanged: usize,
        missing: usize,
        errors: usize,
    },
    Failed(String),
}

/// Runs catalog indexing in a dedicated background thread with its own SQLite connection.
pub struct IndexWorker {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<IndexEvent>,
    join: Option<JoinHandle<()>>,
}

impl IndexWorker {
    pub fn start(db: DestPath, roots: Vec<LibraryRoot>, notify: Option<Notify>) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_bg = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();

        let join = thread::spawn(move || {
            let emit = |ev: IndexEvent| {
                let _ = tx.send(ev);
                if let Some(cb) = &notify {
                    cb();
                }
            };

            let mut cat = match Catalog::open(&db) {
                Ok(c) => c,
                Err(e) => {
                    emit(IndexEvent::Failed(e.to_string()));
                    return;
                }
            };

            let root_count = roots.len();
            let mut stopped = false;
            let mut total_new = 0usize;
            let mut total_changed = 0usize;
            let mut total_unchanged = 0usize;
            let mut total_missing = 0usize;
            let mut total_errors = 0usize;

            for (root_index, root) in roots.into_iter().enumerate() {
                if cancel_bg.load(Ordering::Relaxed) {
                    stopped = true;
                    break;
                }
                let root_path = root.as_path().to_path_buf();
                let res = cat.index_with(&root, &mut |p: &Progress| {
                    emit(IndexEvent::Progress {
                        root_index,
                        root_count,
                        root_path: root_path.clone(),
                        done: p.done,
                        total: p.total,
                    });
                    if cancel_bg.load(Ordering::Relaxed) {
                        Control::Stop
                    } else {
                        Control::Continue
                    }
                });

                match res {
                    Ok(report) => {
                        total_new += report.new;
                        total_changed += report.changed;
                        total_unchanged += report.unchanged;
                        total_missing += report.missing;
                        total_errors += report.errors.len();
                        let was_stopped = report.stopped;
                        emit(IndexEvent::RootDone { root_path, report });
                        if was_stopped {
                            stopped = true;
                            break;
                        }
                    }
                    Err(e) => {
                        emit(IndexEvent::Failed(e.to_string()));
                        return;
                    }
                }
            }

            emit(IndexEvent::Finished {
                stopped,
                new: total_new,
                changed: total_changed,
                unchanged: total_unchanged,
                missing: total_missing,
                errors: total_errors,
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

    pub fn try_recv(&self) -> Option<IndexEvent> {
        self.rx.try_recv().ok()
    }

    pub fn recv(&self) -> Result<IndexEvent, mpsc::RecvError> {
        self.rx.recv()
    }
}

impl Drop for IndexWorker {
    fn drop(&mut self) {
        self.cancel();
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Debug, Clone)]
pub struct ThumbRequest {
    pub source: PathBuf,
    pub sha256: String,
}

pub struct ThumbResult {
    pub sha256: String,
    pub image: Result<ColorImage, String>,
}

struct ThumbQueue {
    /// LIFO stack so the most recently requested (currently visible) thumbnails decode first.
    pending: Vec<ThumbRequest>,
    visible: HashSet<String>,
    in_flight: HashSet<String>,
    shutdown: bool,
}

/// Fixed-size worker pool that fetches and decodes thumbnails with LIFO priority and offscreen skipping.
pub struct ThumbPool {
    shared: Arc<(Mutex<ThumbQueue>, Condvar)>,
    rx: mpsc::Receiver<ThumbResult>,
    workers: Vec<JoinHandle<()>>,
}

impl ThumbPool {
    pub fn new(
        threads: usize,
        provider: Arc<dyn ThumbnailProvider>,
        notify: Option<Notify>,
    ) -> Self {
        let count = threads.clamp(1, 8);
        let shared = Arc::new((
            Mutex::new(ThumbQueue {
                pending: Vec::new(),
                visible: HashSet::new(),
                in_flight: HashSet::new(),
                shutdown: false,
            }),
            Condvar::new(),
        ));
        let (tx, rx) = mpsc::channel();
        let mut workers = Vec::with_capacity(count);

        for _ in 0..count {
            let shared = Arc::clone(&shared);
            let provider = Arc::clone(&provider);
            let tx = tx.clone();
            let notify = notify.clone();
            workers.push(thread::spawn(move || {
                loop {
                    let req = {
                        let (lock, cvar) = &*shared;
                        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if q.shutdown {
                                return;
                            }
                            if let Some(req) = q.pending.pop() {
                                if !q.visible.contains(&req.sha256) {
                                    continue;
                                }
                                q.in_flight.insert(req.sha256.clone());
                                break req;
                            }
                            q = cvar.wait(q).unwrap_or_else(|e| e.into_inner());
                        }
                    };

                    let image = match provider.get(&req.source, &req.sha256) {
                        Ok(thumb_path) => {
                            let still_wanted = {
                                let (lock, _) = &*shared;
                                let q = lock.lock().unwrap_or_else(|e| e.into_inner());
                                !q.shutdown && q.visible.contains(&req.sha256)
                            };
                            if !still_wanted {
                                let (lock, _) = &*shared;
                                lock.lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .in_flight
                                    .remove(&req.sha256);
                                continue;
                            }
                            decode_image_file(&thumb_path, None)
                        }
                        Err(e) => Err(e.to_string()),
                    };

                    let should_emit = {
                        let (lock, _) = &*shared;
                        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
                        q.in_flight.remove(&req.sha256);
                        !q.shutdown && q.visible.contains(&req.sha256)
                    };

                    if should_emit {
                        let _ = tx.send(ThumbResult {
                            sha256: req.sha256,
                            image,
                        });
                        if let Some(cb) = &notify {
                            cb();
                        }
                    }
                }
            }));
        }

        Self {
            shared,
            rx,
            workers,
        }
    }

    pub fn request(&self, req: ThumbRequest) {
        let (lock, cvar) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.visible.insert(req.sha256.clone());
        if q.in_flight.contains(&req.sha256) {
            return;
        }
        q.pending.retain(|r| r.sha256 != req.sha256);
        q.pending.push(req);
        cvar.notify_one();
    }

    /// Replaces the visible set and immediately drops queued requests that scrolled offscreen.
    pub fn set_visible(&self, visible: &HashSet<String>) {
        let (lock, _) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.visible.clone_from(visible);
        q.pending.retain(|r| visible.contains(&r.sha256));
    }

    pub fn cancel_all(&self) {
        let (lock, _) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.visible.clear();
        q.pending.clear();
    }

    pub fn try_recv(&self) -> Option<ThumbResult> {
        self.rx.try_recv().ok()
    }

    pub fn recv(&self) -> Result<ThumbResult, mpsc::RecvError> {
        self.rx.recv()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<ThumbResult, mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }
}

impl Drop for ThumbPool {
    fn drop(&mut self) {
        {
            let (lock, cvar) = &*self.shared;
            let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
            q.shutdown = true;
            q.visible.clear();
            q.pending.clear();
            cvar.notify_all();
        }
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

pub struct PreviewResult {
    pub path: PathBuf,
    pub image: Result<ColorImage, String>,
}

struct PreviewQueue {
    next: Option<PathBuf>,
    shutdown: bool,
}

/// Single-thread background decoder for the inspector preview; newer requests supersede older ones.
pub struct PreviewWorker {
    shared: Arc<(Mutex<PreviewQueue>, Condvar)>,
    rx: mpsc::Receiver<PreviewResult>,
    join: Option<JoinHandle<()>>,
}

impl PreviewWorker {
    pub fn new(max_edge: u32, notify: Option<Notify>) -> Self {
        let shared = Arc::new((
            Mutex::new(PreviewQueue {
                next: None,
                shutdown: false,
            }),
            Condvar::new(),
        ));
        let (tx, rx) = mpsc::channel();
        let shared_bg = Arc::clone(&shared);

        let join = thread::spawn(move || {
            loop {
                let path = {
                    let (lock, cvar) = &*shared_bg;
                    let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if q.shutdown {
                            return;
                        }
                        if let Some(p) = q.next.take() {
                            break p;
                        }
                        q = cvar.wait(q).unwrap_or_else(|e| e.into_inner());
                    }
                };
                let image = decode_image_file(&path, Some(max_edge));
                let _ = tx.send(PreviewResult { path, image });
                if let Some(cb) = &notify {
                    cb();
                }
            }
        });

        Self {
            shared,
            rx,
            join: Some(join),
        }
    }

    pub fn request(&self, path: PathBuf) {
        let (lock, cvar) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.next = Some(path);
        cvar.notify_one();
    }

    pub fn clear(&self) {
        let (lock, _) = &*self.shared;
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.next = None;
    }

    pub fn try_recv(&self) -> Option<PreviewResult> {
        self.rx.try_recv().ok()
    }

    pub fn recv(&self) -> Result<PreviewResult, mpsc::RecvError> {
        self.rx.recv()
    }
}

impl Drop for PreviewWorker {
    fn drop(&mut self) {
        {
            let (lock, cvar) = &*self.shared;
            let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
            q.shutdown = true;
            q.next = None;
            cvar.notify_all();
        }
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}
