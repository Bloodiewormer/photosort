use crate::ocr_worker::{EngineOcrReader, OcrImageReader};
use crate::settings::OcrDevice;
use crate::vision_worker::{
    EngineVisionReader, VisionImageOutput, VisionReader, compile_or_embed_rules,
};
use crate::worker::Notify;
use catalog::{
    AssetId, Catalog, Control, IndexReport, OCR_STAGE, OCR_VERSION, Pending, SIGLIP2_MODEL_ID,
    StageSpec, VISION_STAGE, VISION_VERSION,
};
use fsguard::{DestPath, LibraryRoot};
use image::RgbImage;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Pause after the catalog refuses a write, so that a full disk or a locked database cannot turn
/// into a loop that analyses the same photos without rest.
pub const DEFAULT_ERROR_BACKOFF: Duration = Duration::from_secs(30);

/// How often, at most, finished chunks make the interface reload the whole catalog.
pub const CHUNK_RELOAD_INTERVAL: Duration = Duration::from_secs(10);

/// A full reload costs hundreds of milliseconds on a large library and runs on the interface
/// thread: while the engine works it is spaced out, and when it stops it happens at once.
pub fn chunk_reload_due(pending: bool, since_last: Duration, phase: &EnginePhase) -> bool {
    let working = matches!(
        phase,
        EnginePhase::Running { .. } | EnginePhase::LoadingModel(_) | EnginePhase::Indexing { .. }
    );
    pending && (!working || since_last >= CHUNK_RELOAD_INTERVAL)
}

/// How often the registered folders are listed again while the application stays open.
pub const FOLDER_RECHECK_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Least time between two checks started by the window getting the focus back: switching windows
/// often must not list 35.000 photos each time.
pub const FOCUS_RECHECK_MIN_GAP: Duration = Duration::from_secs(60);

/// Decides when to look for changes in the folders: when the window gets the focus back (at most
/// once per [`FOCUS_RECHECK_MIN_GAP`], later rather than never), and every
/// [`FOLDER_RECHECK_INTERVAL`].
pub struct FolderRecheck {
    was_focused: bool,
    focus_pending: bool,
    last: Instant,
}

impl FolderRecheck {
    pub fn new(now: Instant) -> Self {
        Self {
            was_focused: true,
            focus_pending: false,
            last: now,
        }
    }

    pub fn due(&mut self, focused: bool, now: Instant) -> bool {
        let was_focused = std::mem::replace(&mut self.was_focused, focused);
        self.focus_pending |= focused && !was_focused;
        let since = now - self.last;
        let due = since >= FOLDER_RECHECK_INTERVAL
            || (self.focus_pending && since >= FOCUS_RECHECK_MIN_GAP);
        if due {
            self.last = now;
            self.focus_pending = false;
        }
        due
    }

    /// Time left until the next check, so that an idle window wakes up for it.
    pub fn remaining(&self, now: Instant) -> Duration {
        let wait = if self.focus_pending {
            FOCUS_RECHECK_MIN_GAP
        } else {
            FOLDER_RECHECK_INTERVAL
        };
        wait.saturating_sub(now - self.last)
    }
}

/// Number of pending assets processed together through all enabled stages before moving on.
/// Chosen so photos become reviewed progressively while amortizing the reload of both engines
/// when Vision and OCR are enabled: about 6 s per chunk with DirectML in a release build.
pub const DEFAULT_CHUNK_SIZE: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StageKind {
    Rules,
    Vision,
    Ocr,
}

impl StageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rules => rules::STAGE,
            Self::Vision => VISION_STAGE,
            Self::Ocr => OCR_STAGE,
        }
    }

    pub fn version(self) -> u32 {
        match self {
            Self::Rules => rules::VERSION,
            Self::Vision => VISION_VERSION,
            Self::Ocr => OCR_VERSION,
        }
    }

    pub fn label_es(self) -> &'static str {
        match self {
            Self::Rules => "Reglas rápidas",
            Self::Vision => "Reconocer imágenes",
            Self::Ocr => "Leer texto",
        }
    }

    pub fn from_stage_str(stage: &str) -> Option<Self> {
        match stage {
            rules::STAGE => Some(Self::Rules),
            VISION_STAGE => Some(Self::Vision),
            OCR_STAGE => Some(Self::Ocr),
            _ => None,
        }
    }

    pub fn spec(self) -> StageSpec {
        StageSpec::new(self.as_str(), self.version())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewConfig {
    pub enabled: bool,
    pub rules: bool,
    pub vision: bool,
    pub ocr: bool,
    pub roots: Vec<PathBuf>,
    pub vision_models_dir: Option<PathBuf>,
    pub ocr_models_dir: Option<PathBuf>,
    pub device: OcrDevice,
    pub chunk_size: usize,
    pub error_backoff: Duration,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            rules: true,
            vision: false,
            ocr: false,
            roots: Vec::new(),
            vision_models_dir: None,
            ocr_models_dir: None,
            device: OcrDevice::DirectMl,
            chunk_size: DEFAULT_CHUNK_SIZE,
            error_backoff: DEFAULT_ERROR_BACKOFF,
        }
    }
}

impl ReviewConfig {
    pub fn enabled_stage_kinds(&self) -> Vec<StageKind> {
        let mut out = Vec::with_capacity(3);
        if self.rules {
            out.push(StageKind::Rules);
        }
        if self.vision {
            out.push(StageKind::Vision);
        }
        if self.ocr {
            out.push(StageKind::Ocr);
        }
        out
    }

    pub fn enabled_stage_specs(&self) -> Vec<StageSpec> {
        self.enabled_stage_kinds()
            .into_iter()
            .map(StageKind::spec)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagePause {
    pub stage: StageKind,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnginePhase {
    Off,
    Paused,
    Yielded,
    Indexing {
        root_index: usize,
        root_count: usize,
        done: usize,
        total: usize,
    },
    LoadingModel(StageKind),
    Running {
        done: usize,
        total: usize,
        stage: StageKind,
        elapsed_secs: f64,
    },
    Idle,
    Blocked(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct EngineSnapshot {
    pub phase: EnginePhase,
    pub failed_count: usize,
    pub paused_stages: Vec<StagePause>,
    pub last_error: Option<String>,
    pub active_step: bool,
}

impl Default for EngineSnapshot {
    fn default() -> Self {
        Self {
            phase: EnginePhase::Off,
            failed_count: 0,
            paused_stages: Vec::new(),
            last_error: None,
            active_step: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReviewEngineEvent {
    Status(EngineSnapshot),
    IndexFinished {
        new: usize,
        changed: usize,
        missing: usize,
        restored: usize,
        errors: usize,
    },
    AssetProgress {
        asset: AssetId,
        now_reviewed: bool,
    },
    ChunkFinished,
}

pub type VisionLoader =
    Box<dyn Fn(Option<&Path>, OcrDevice) -> Result<Box<dyn VisionReader>, String> + Send + 'static>;
pub type OcrLoader = Box<
    dyn Fn(Option<&Path>, OcrDevice) -> Result<Box<dyn OcrImageReader>, String> + Send + 'static,
>;

pub fn default_vision_loader() -> VisionLoader {
    Box::new(|dir, device| {
        let Some(dir) = dir else {
            return Err("Falta la carpeta de modelos de visión en Configuración".into());
        };
        if !dir.is_dir() {
            return Err("La carpeta de modelos de visión no existe".into());
        }
        let reader = EngineVisionReader::load(dir, device)?;
        Ok(Box::new(reader))
    })
}

pub fn default_ocr_loader() -> OcrLoader {
    Box::new(|dir, device| {
        let Some(dir) = dir else {
            return Err("Falta la carpeta de modelos OCR en Configuración".into());
        };
        if !dir.is_dir() {
            return Err("La carpeta de modelos OCR no existe".into());
        }
        let reader = EngineOcrReader::load(dir, device)?;
        Ok(Box::new(reader))
    })
}

struct EngineControl {
    config: ReviewConfig,
    paused: bool,
    yielded: bool,
    shutdown: bool,
    needs_index: bool,
    /// The folders are being listed right now.
    indexing: bool,
    priority: VecDeque<AssetId>,
    clear_failed: bool,
    retry_assets: Vec<AssetId>,
    clear_paused_stages: Vec<StageKind>,
    config_gen: u64,
}

/// Persistent background review engine. Processes registered folders in chunks through all enabled
/// stages (rules, vision, OCR+works) without ever holding Vision and OCR in GPU memory at once.
pub struct ReviewEngine {
    shared: Arc<(Mutex<EngineControl>, Condvar)>,
    rx: mpsc::Receiver<ReviewEngineEvent>,
    join: Option<JoinHandle<()>>,
}

impl ReviewEngine {
    pub fn start(db: DestPath, config: ReviewConfig, notify: Option<Notify>) -> Self {
        Self::start_with_loaders(
            db,
            config,
            default_vision_loader(),
            default_ocr_loader(),
            notify,
        )
    }

    pub fn start_with_loaders(
        db: DestPath,
        config: ReviewConfig,
        vision_loader: VisionLoader,
        ocr_loader: OcrLoader,
        notify: Option<Notify>,
    ) -> Self {
        let needs_index = config.enabled && !config.roots.is_empty();
        let shared = Arc::new((
            Mutex::new(EngineControl {
                config,
                paused: false,
                yielded: false,
                shutdown: false,
                needs_index,
                indexing: false,
                priority: VecDeque::new(),
                clear_failed: false,
                retry_assets: Vec::new(),
                clear_paused_stages: Vec::new(),
                config_gen: 1,
            }),
            Condvar::new(),
        ));
        let (tx, rx) = mpsc::channel();
        let shared_bg = Arc::clone(&shared);

        let join = thread::spawn(move || {
            run_engine_loop(db, shared_bg, tx, notify, vision_loader, ocr_loader);
        });

        Self {
            shared,
            rx,
            join: Some(join),
        }
    }

    pub fn update_config(&self, next: ReviewConfig) {
        let (lock, cvar) = &*self.shared;
        let mut ctrl = lock.lock().unwrap_or_else(|e| e.into_inner());
        // Saving settings that do not concern the engine, such as the theme, must not interrupt it.
        if next == ctrl.config {
            return;
        }
        let trigger_index = next.enabled
            && (!ctrl.config.enabled || next.roots != ctrl.config.roots)
            && !next.roots.is_empty();
        let clear_vision = next.vision_models_dir != ctrl.config.vision_models_dir
            || next.device != ctrl.config.device
            || (next.vision && !ctrl.config.vision);
        let clear_ocr = next.ocr_models_dir != ctrl.config.ocr_models_dir
            || next.device != ctrl.config.device
            || (next.ocr && !ctrl.config.ocr);
        if trigger_index {
            ctrl.needs_index = true;
        }
        if clear_vision {
            ctrl.clear_paused_stages.push(StageKind::Vision);
        }
        if clear_ocr {
            ctrl.clear_paused_stages.push(StageKind::Ocr);
        }
        ctrl.config = next;
        ctrl.config_gen = ctrl.config_gen.wrapping_add(1);
        cvar.notify_all();
    }

    pub fn set_paused(&self, paused: bool) {
        let (lock, cvar) = &*self.shared;
        let mut ctrl = lock.lock().unwrap_or_else(|e| e.into_inner());
        ctrl.paused = paused;
        cvar.notify_all();
    }

    pub fn is_paused(&self) -> bool {
        let (lock, _) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).paused
    }

    pub fn set_yielded(&self, yielded: bool) {
        let (lock, cvar) = &*self.shared;
        let mut ctrl = lock.lock().unwrap_or_else(|e| e.into_inner());
        if ctrl.yielded != yielded {
            ctrl.yielded = yielded;
            cvar.notify_all();
        }
    }

    pub fn trigger_index(&self) {
        let (lock, cvar) = &*self.shared;
        let mut ctrl = lock.lock().unwrap_or_else(|e| e.into_inner());
        ctrl.needs_index = true;
        cvar.notify_all();
    }

    /// Like [`trigger_index`](Self::trigger_index), for automatic checks: does nothing when an index
    /// is already waiting or running, so they never pile up. While the engine is paused or yielded
    /// to a heavy task, the request waits.
    pub fn request_index(&self) {
        let (lock, cvar) = &*self.shared;
        let mut ctrl = lock.lock().unwrap_or_else(|e| e.into_inner());
        if !ctrl.needs_index && !ctrl.indexing {
            ctrl.needs_index = true;
            cvar.notify_all();
        }
    }

    /// Queues `assets` to be processed first (even if `config.enabled` is false).
    /// Also unpauses if paused so an explicit user action starts promptly.
    pub fn prioritize(&self, assets: Vec<AssetId>) {
        if assets.is_empty() {
            return;
        }
        let (lock, cvar) = &*self.shared;
        let mut ctrl = lock.lock().unwrap_or_else(|e| e.into_inner());
        let existing: HashSet<AssetId> = ctrl.priority.iter().copied().collect();
        for id in assets.into_iter().rev() {
            if !existing.contains(&id) {
                ctrl.priority.push_front(id);
            }
        }
        ctrl.paused = false;
        cvar.notify_all();
    }

    pub fn retry_failed(&self) {
        let (lock, cvar) = &*self.shared;
        let mut ctrl = lock.lock().unwrap_or_else(|e| e.into_inner());
        ctrl.clear_failed = true;
        ctrl.clear_paused_stages
            .extend([StageKind::Rules, StageKind::Vision, StageKind::Ocr]);
        ctrl.paused = false;
        cvar.notify_all();
    }

    /// Clears persisted errors for a single photo and queues it to be retried immediately.
    pub fn retry_asset(&self, asset: AssetId) {
        let (lock, cvar) = &*self.shared;
        let mut ctrl = lock.lock().unwrap_or_else(|e| e.into_inner());
        ctrl.retry_assets.push(asset);
        if !ctrl.priority.contains(&asset) {
            ctrl.priority.push_front(asset);
        }
        ctrl.clear_paused_stages
            .extend([StageKind::Rules, StageKind::Vision, StageKind::Ocr]);
        ctrl.paused = false;
        cvar.notify_all();
    }

    pub fn try_recv(&self) -> Option<ReviewEngineEvent> {
        self.rx.try_recv().ok()
    }

    pub fn recv(&self) -> Result<ReviewEngineEvent, mpsc::RecvError> {
        self.rx.recv()
    }

    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<ReviewEngineEvent, mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }
}

impl Drop for ReviewEngine {
    fn drop(&mut self) {
        {
            let (lock, cvar) = &*self.shared;
            let mut ctrl = lock.lock().unwrap_or_else(|e| e.into_inner());
            ctrl.shutdown = true;
            cvar.notify_all();
        }
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

struct SessionState {
    failed: HashSet<(AssetId, StageKind)>,
    paused_stages: HashMap<StageKind, String>,
    last_error: Option<String>,
    vision_reader: Option<Box<dyn VisionReader>>,
    ocr_reader: Option<Box<dyn OcrImageReader>>,
    session_started: Option<Instant>,
    session_done: usize,
    /// Set when the catalog failed: the next turn waits before trying anything else.
    catalog_failed: bool,
}

impl SessionState {
    fn new() -> Self {
        Self {
            failed: HashSet::new(),
            paused_stages: HashMap::new(),
            last_error: None,
            vision_reader: None,
            ocr_reader: None,
            session_started: None,
            session_done: 0,
            catalog_failed: false,
        }
    }

    fn catalog_error(&mut self, error: &catalog::CatalogError) {
        self.last_error = Some(error.to_string());
        self.catalog_failed = true;
    }

    fn release_models(&mut self) {
        self.vision_reader = None;
        self.ocr_reader = None;
    }

    fn load_persisted_failures(&mut self, cat: &Catalog, roots: &[PathBuf]) {
        if let Ok(pairs) = cat.failed_pairs_in_roots(roots) {
            self.failed.clear();
            for (asset, stage) in pairs {
                if let Some(kind) = StageKind::from_stage_str(&stage) {
                    self.failed.insert((asset, kind));
                }
            }
        }
    }

    /// A photo that failed to be analysed. It is remembered across sessions only while the file can
    /// be opened: a missing or locked file says nothing about the photo itself.
    fn record_failure(
        &mut self,
        cat: &Catalog,
        asset: AssetId,
        stage: StageKind,
        source: &Path,
        reason: &str,
    ) {
        // A photo that is no longer there is not a failure: once flagged, another copy of it can be
        // analysed.
        if cat.mark_location_missing(source).unwrap_or(false) {
            return;
        }
        self.failed.insert((asset, stage));
        if std::fs::File::open(source).is_ok() {
            let _ = cat.record_stage_error(asset, stage.as_str(), reason);
        }
    }

    fn failed_assets_count(&self) -> usize {
        self.failed
            .iter()
            .map(|(id, _)| *id)
            .collect::<HashSet<_>>()
            .len()
    }

    fn paused_list(&self) -> Vec<StagePause> {
        let mut out: Vec<StagePause> = self
            .paused_stages
            .iter()
            .map(|(&stage, reason)| StagePause {
                stage,
                reason: reason.clone(),
            })
            .collect();
        out.sort_by_key(|p| match p.stage {
            StageKind::Rules => 0,
            StageKind::Vision => 1,
            StageKind::Ocr => 2,
        });
        out
    }

    fn snapshot(&self, phase: EnginePhase, active_step: bool) -> EngineSnapshot {
        EngineSnapshot {
            phase,
            failed_count: self.failed_assets_count(),
            paused_stages: self.paused_list(),
            last_error: self.last_error.clone(),
            active_step,
        }
    }
}

fn should_interrupt(shared: &(Mutex<EngineControl>, Condvar), expected_gen: u64) -> bool {
    let ctrl = shared.0.lock().unwrap_or_else(|e| e.into_inner());
    ctrl.shutdown || ctrl.paused || ctrl.yielded || ctrl.config_gen != expected_gen
}

fn run_engine_loop(
    db: DestPath,
    shared: Arc<(Mutex<EngineControl>, Condvar)>,
    tx: mpsc::Sender<ReviewEngineEvent>,
    notify: Option<Notify>,
    vision_loader: VisionLoader,
    ocr_loader: OcrLoader,
) {
    let emit = |ev: ReviewEngineEvent| {
        let _ = tx.send(ev);
        if let Some(cb) = &notify {
            cb();
        }
    };

    let mut cat = match Catalog::open(&db) {
        Ok(c) => c,
        Err(e) => {
            emit(ReviewEngineEvent::Status(EngineSnapshot {
                phase: EnginePhase::Blocked(format!("No se pudo abrir el catálogo: {e}")),
                failed_count: 0,
                paused_stages: Vec::new(),
                last_error: Some(e.to_string()),
                active_step: false,
            }));
            return;
        }
    };

    let mut session = SessionState::new();
    let mut last_loaded_gen = 0u64;
    // Requested photos taken from the queue for the current turn.
    let mut in_flight: Vec<AssetId> = Vec::new();
    // What indexing changed and the interface has not been told yet: an interrupted run keeps it.
    let mut unreported = IndexReport::default();

    loop {
        if std::mem::take(&mut session.catalog_failed) {
            let reason = session.last_error.clone().unwrap_or_default();
            emit(ReviewEngineEvent::Status(session.snapshot(
                EnginePhase::Blocked(format!("Error del catálogo: {reason}")),
                false,
            )));
            back_off(&shared, last_loaded_gen);
        }
        if !in_flight.is_empty() {
            requeue(&cat, &shared, &session, std::mem::take(&mut in_flight));
        }

        // 1. Inspect control state and wait if there is nothing to do.
        let (cfg, cfg_gen, do_index, priority_batch) = {
            let (lock, cvar) = &*shared;
            let mut ctrl = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if ctrl.shutdown {
                    session.release_models();
                    return;
                }
                if ctrl.clear_failed {
                    let _ = cat.clear_all_stage_errors_in_roots(&ctrl.config.roots);
                    session.failed.clear();
                    session.last_error = None;
                    ctrl.clear_failed = false;
                }
                if !ctrl.retry_assets.is_empty() {
                    for id in ctrl.retry_assets.drain(..) {
                        let _ = cat.clear_stage_errors_for_asset(id);
                        for st in [StageKind::Rules, StageKind::Vision, StageKind::Ocr] {
                            session.failed.remove(&(id, st));
                        }
                    }
                }
                if !ctrl.clear_paused_stages.is_empty() {
                    for stage in ctrl.clear_paused_stages.drain(..) {
                        session.paused_stages.remove(&stage);
                    }
                    session.release_models();
                }
                if ctrl.config_gen != last_loaded_gen {
                    session.release_models();
                    session.load_persisted_failures(&cat, &ctrl.config.roots);
                    last_loaded_gen = ctrl.config_gen;
                }

                if ctrl.yielded {
                    session.release_models();
                    emit(ReviewEngineEvent::Status(
                        session.snapshot(EnginePhase::Yielded, false),
                    ));
                    ctrl = cvar.wait(ctrl).unwrap_or_else(|e| e.into_inner());
                    continue;
                }

                if ctrl.paused {
                    session.release_models();
                    emit(ReviewEngineEvent::Status(
                        session.snapshot(EnginePhase::Paused, false),
                    ));
                    ctrl = cvar.wait(ctrl).unwrap_or_else(|e| e.into_inner());
                    continue;
                }

                let has_priority = !ctrl.priority.is_empty();
                if !ctrl.config.enabled && !has_priority && !ctrl.needs_index {
                    session.release_models();
                    session.session_started = None;
                    session.session_done = 0;
                    emit(ReviewEngineEvent::Status(
                        session.snapshot(EnginePhase::Off, false),
                    ));
                    ctrl = cvar.wait(ctrl).unwrap_or_else(|e| e.into_inner());
                    continue;
                }

                let do_index = ctrl.needs_index;
                if do_index {
                    ctrl.needs_index = false;
                    ctrl.indexing = true;
                }
                let chunk_cap = ctrl.config.chunk_size.max(1);
                let mut priority_batch = Vec::new();
                if !do_index {
                    let mut seen = HashSet::new();
                    while priority_batch.len() < chunk_cap
                        && let Some(id) = ctrl.priority.pop_front()
                    {
                        if seen.insert(id) {
                            // Explicit re-review clears prior failures for this asset.
                            let _ = cat.clear_stage_errors_for_asset(id);
                            for st in [StageKind::Rules, StageKind::Vision, StageKind::Ocr] {
                                session.failed.remove(&(id, st));
                            }
                            priority_batch.push(id);
                        }
                    }
                }
                break (
                    ctrl.config.clone(),
                    ctrl.config_gen,
                    do_index,
                    priority_batch,
                );
            }
        };

        // 2. Index registered roots first when needed.
        if do_index {
            // Models stay loaded: listing the folders does not use them, and the periodic checks
            // would otherwise reload them every few minutes during a long review.
            let valid_roots: Vec<LibraryRoot> = cfg
                .roots
                .iter()
                .filter_map(|p| LibraryRoot::new(p).ok())
                .collect();
            let root_count = valid_roots.len();
            let mut stopped = false;
            let mut total_errors = 0usize;

            for (root_index, root) in valid_roots.into_iter().enumerate() {
                if should_interrupt(&shared, cfg_gen) {
                    stopped = true;
                    break;
                }
                let res = cat.index_with(&root, &mut |p| {
                    emit(ReviewEngineEvent::Status(session.snapshot(
                        EnginePhase::Indexing {
                            root_index,
                            root_count,
                            done: p.done,
                            total: p.total,
                        },
                        true,
                    )));
                    if should_interrupt(&shared, cfg_gen) {
                        Control::Stop
                    } else {
                        Control::Continue
                    }
                });
                match res {
                    Ok(report) => {
                        unreported.new += report.new;
                        unreported.changed += report.changed;
                        unreported.missing += report.missing;
                        unreported.restored += report.restored;
                        total_errors += report.errors.len();
                        if report.stopped {
                            stopped = true;
                            break;
                        }
                    }
                    Err(e) => {
                        session.last_error = Some(format!("Error al indexar: {e}"));
                        break;
                    }
                }
            }

            {
                let mut ctrl = shared.0.lock().unwrap_or_else(|e| e.into_inner());
                ctrl.indexing = false;
                ctrl.needs_index |= stopped;
            }
            if !stopped {
                let done = std::mem::take(&mut unreported);
                emit(ReviewEngineEvent::IndexFinished {
                    new: done.new,
                    changed: done.changed,
                    missing: done.missing,
                    restored: done.restored,
                    errors: total_errors,
                });
            }
            continue;
        }

        // 3. Determine enabled vs active (non-paused) stages.
        let enabled_kinds = cfg.enabled_stage_kinds();
        let enabled_specs = cfg.enabled_stage_specs();
        if enabled_kinds.is_empty() {
            session.release_models();
            if cfg.enabled || !priority_batch.is_empty() {
                emit(ReviewEngineEvent::Status(session.snapshot(
                    EnginePhase::Blocked("Activa al menos una etapa en Configuración".into()),
                    false,
                )));
            } else {
                emit(ReviewEngineEvent::Status(
                    session.snapshot(EnginePhase::Off, false),
                ));
            }
            wait_for_change(&shared, cfg_gen);
            continue;
        }

        let active_kinds: Vec<StageKind> = enabled_kinds
            .iter()
            .copied()
            .filter(|k| !session.paused_stages.contains_key(k))
            .collect();
        if active_kinds.is_empty() {
            session.release_models();
            let reason = session
                .paused_list()
                .first()
                .map(|p| format!("{}: {}", p.stage.label_es(), p.reason))
                .unwrap_or_else(|| "Todas las etapas activas están pausadas".into());
            emit(ReviewEngineEvent::Status(
                session.snapshot(EnginePhase::Blocked(reason), false),
            ));
            wait_for_change(&shared, cfg_gen);
            continue;
        }
        let active_specs: Vec<StageSpec> =
            active_kinds.iter().copied().map(StageKind::spec).collect();

        // 4. Select next chunk of assets (priority first, then background pending).
        let chunk_size = cfg.chunk_size.max(1);
        let chunk_assets: Vec<AssetId> = if !priority_batch.is_empty() {
            in_flight = priority_batch.clone();
            priority_batch
        } else if cfg.enabled {
            let fetch_limit = chunk_size
                .saturating_add(session.failed.len())
                .saturating_add(16);
            match cat.pending_any_stage_in_roots(&active_specs, &cfg.roots, fetch_limit) {
                Ok(candidates) => {
                    let ids: Vec<AssetId> = candidates.iter().map(|c| c.asset).collect();
                    let mut runnable =
                        unfinished(&cat, &ids, &active_kinds, &session.failed, &cfg.roots);
                    runnable.truncate(chunk_size);
                    runnable
                }
                Err(e) => {
                    session.last_error = Some(e.to_string());
                    emit(ReviewEngineEvent::Status(
                        session.snapshot(EnginePhase::Blocked(e.to_string()), false),
                    ));
                    wait_for_change(&shared, cfg_gen);
                    continue;
                }
            }
        } else {
            Vec::new()
        };

        if chunk_assets.is_empty() {
            session.release_models();
            session.session_started = None;
            session.session_done = 0;
            let phase = if cfg.enabled {
                EnginePhase::Idle
            } else {
                EnginePhase::Off
            };
            emit(ReviewEngineEvent::Status(session.snapshot(phase, false)));
            wait_for_change(&shared, cfg_gen);
            continue;
        }

        let started = *session.session_started.get_or_insert_with(Instant::now);
        let remaining_estimate = count_remaining_work(
            &cat,
            &active_specs,
            &cfg,
            &shared,
            &session,
            chunk_assets.len(),
        );
        let mut total_estimate = session.session_done.saturating_add(remaining_estimate);
        let both_gpu_stages =
            active_kinds.contains(&StageKind::Vision) && active_kinds.contains(&StageKind::Ocr);

        // 5. Run each active stage over this chunk in order: Rules -> Vision -> OCR.
        // 5a. Rules
        if active_kinds.contains(&StageKind::Rules) && !should_interrupt(&shared, cfg_gen) {
            let pending = match cat.pending_in_assets_in_roots(
                rules::STAGE,
                rules::VERSION,
                &chunk_assets,
                &cfg.roots,
            ) {
                Ok(p) => p,
                Err(e) => {
                    session.catalog_error(&e);
                    continue;
                }
            };
            for item in pending {
                if session.failed.contains(&(item.asset, StageKind::Rules)) {
                    continue;
                }
                if should_interrupt(&shared, cfg_gen) {
                    break;
                }
                emit(ReviewEngineEvent::Status(session.snapshot(
                    EnginePhase::Running {
                        done: session.session_done,
                        total: total_estimate.max(session.session_done + 1),
                        stage: StageKind::Rules,
                        elapsed_secs: started.elapsed().as_secs_f64(),
                    },
                    true,
                )));
                match rules::analyze_pending_item(&cat, &item) {
                    Ok(Ok(_)) => {
                        let reviewed = cat
                            .is_asset_reviewed(item.asset, &enabled_specs)
                            .unwrap_or(false);
                        if reviewed {
                            session.session_done += 1;
                            total_estimate = total_estimate.max(session.session_done);
                        }
                        emit(ReviewEngineEvent::AssetProgress {
                            asset: item.asset,
                            now_reviewed: reviewed,
                        });
                    }
                    Ok(Err(reason)) => {
                        session.record_failure(
                            &cat,
                            item.asset,
                            StageKind::Rules,
                            &item.path,
                            &reason,
                        );
                    }
                    Err(e) => {
                        session.failed.insert((item.asset, StageKind::Rules));
                        session.catalog_error(&e);
                        break;
                    }
                }
            }
        }

        // 5b. Vision
        if active_kinds.contains(&StageKind::Vision) && !should_interrupt(&shared, cfg_gen) {
            let pending: Vec<Pending> = match cat.pending_in_assets_in_roots(
                VISION_STAGE,
                VISION_VERSION,
                &chunk_assets,
                &cfg.roots,
            ) {
                Ok(p) => p
                    .into_iter()
                    .filter(|item| !session.failed.contains(&(item.asset, StageKind::Vision)))
                    .collect(),
                Err(e) => {
                    session.catalog_error(&e);
                    Vec::new()
                }
            };

            if !pending.is_empty() {
                // Never hold OCR and Vision in GPU memory simultaneously.
                session.ocr_reader = None;

                if session.vision_reader.is_none() {
                    emit(ReviewEngineEvent::Status(session.snapshot(
                        EnginePhase::LoadingModel(StageKind::Vision),
                        true,
                    )));
                    match vision_loader(cfg.vision_models_dir.as_deref(), cfg.device) {
                        Ok(r) => session.vision_reader = Some(r),
                        Err(reason) => {
                            session.paused_stages.insert(StageKind::Vision, reason);
                            emit(ReviewEngineEvent::Status(session.snapshot(
                                EnginePhase::Running {
                                    done: session.session_done,
                                    total: total_estimate,
                                    stage: StageKind::Vision,
                                    elapsed_secs: started.elapsed().as_secs_f64(),
                                },
                                false,
                            )));
                        }
                    }
                }

                if let Some(mut reader) = session.vision_reader.take() {
                    let compiled =
                        compile_or_embed_rules(&mut cat, |texts| reader.embed_texts(texts));
                    let mut keep_reader = !both_gpu_stages;
                    match compiled {
                        Ok(compiled) => {
                            let batch_size = reader.batch_size().max(1);
                            'batches: for sub in pending.chunks(batch_size) {
                                if should_interrupt(&shared, cfg_gen) {
                                    break;
                                }
                                emit(ReviewEngineEvent::Status(session.snapshot(
                                    EnginePhase::Running {
                                        done: session.session_done,
                                        total: total_estimate.max(session.session_done + 1),
                                        stage: StageKind::Vision,
                                        elapsed_secs: started.elapsed().as_secs_f64(),
                                    },
                                    true,
                                )));

                                let mut decoded: Vec<(AssetId, Vec<RgbImage>)> =
                                    Vec::with_capacity(sub.len());
                                for item in sub {
                                    if should_interrupt(&shared, cfg_gen) {
                                        break;
                                    }
                                    let dec = std::panic::catch_unwind(
                                        std::panic::AssertUnwindSafe(|| {
                                            reader.decode_media(&item.path)
                                        }),
                                    )
                                    .unwrap_or_else(|_| {
                                        Err("error interno al decodificar el archivo".into())
                                    });
                                    match dec {
                                        Ok(frames) => decoded.push((item.asset, frames)),
                                        Err(reason) => {
                                            session.record_failure(
                                                &cat,
                                                item.asset,
                                                StageKind::Vision,
                                                &item.path,
                                                &reason,
                                            );
                                        }
                                    }
                                }

                                if decoded.is_empty() {
                                    continue;
                                }

                                let outcomes = run_vision_batch(reader.as_mut(), &decoded);
                                for (asset, outcome) in outcomes {
                                    match outcome {
                                        Ok(out) => {
                                            match cat
                                                .save_vision_result(
                                                    asset,
                                                    SIGLIP2_MODEL_ID,
                                                    &out.embedding,
                                                    &out.faces,
                                                )
                                                .and_then(|()| {
                                                    cat.apply_vision_rules(asset, &compiled)
                                                }) {
                                                Ok(_) => {
                                                    let reviewed = cat
                                                        .is_asset_reviewed(asset, &enabled_specs)
                                                        .unwrap_or(false);
                                                    if reviewed {
                                                        session.session_done += 1;
                                                        total_estimate = total_estimate
                                                            .max(session.session_done);
                                                    }
                                                    emit(ReviewEngineEvent::AssetProgress {
                                                        asset,
                                                        now_reviewed: reviewed,
                                                    });
                                                }
                                                Err(e) => {
                                                    session
                                                        .failed
                                                        .insert((asset, StageKind::Vision));
                                                    session.catalog_error(&e);
                                                    break 'batches;
                                                }
                                            }
                                        }
                                        Err(reason) => {
                                            let source = sub.iter().find(|i| i.asset == asset);
                                            let source =
                                                source.map_or(Path::new(""), |i| i.path.as_path());
                                            session.record_failure(
                                                &cat,
                                                asset,
                                                StageKind::Vision,
                                                source,
                                                &reason,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Err(reason) => {
                            keep_reader = false;
                            session.paused_stages.insert(StageKind::Vision, reason);
                        }
                    }
                    if keep_reader {
                        session.vision_reader = Some(reader);
                    }
                }
            }
        }

        // 5c. OCR + Works matching
        if active_kinds.contains(&StageKind::Ocr) && !should_interrupt(&shared, cfg_gen) {
            let pending: Vec<Pending> = match cat.pending_in_assets_in_roots(
                OCR_STAGE,
                OCR_VERSION,
                &chunk_assets,
                &cfg.roots,
            ) {
                Ok(p) => p
                    .into_iter()
                    .filter(|item| !session.failed.contains(&(item.asset, StageKind::Ocr)))
                    .collect(),
                Err(e) => {
                    session.catalog_error(&e);
                    Vec::new()
                }
            };

            // Videos do not undergo OCR: mark them done for OCR immediately.
            let mut ocr_pending = Vec::new();
            for item in pending {
                if thumbs::is_video_path(&item.path) {
                    match cat.mark_done(item.asset, OCR_STAGE, OCR_VERSION) {
                        Ok(()) => {
                            let reviewed = cat
                                .is_asset_reviewed(item.asset, &enabled_specs)
                                .unwrap_or(false);
                            if reviewed {
                                session.session_done += 1;
                                total_estimate = total_estimate.max(session.session_done);
                            }
                            emit(ReviewEngineEvent::AssetProgress {
                                asset: item.asset,
                                now_reviewed: reviewed,
                            });
                        }
                        Err(e) => {
                            session.failed.insert((item.asset, StageKind::Ocr));
                            session.catalog_error(&e);
                            break;
                        }
                    }
                } else {
                    ocr_pending.push(item);
                }
            }
            let pending = ocr_pending;

            if !pending.is_empty() {
                // Never hold Vision and OCR in GPU memory simultaneously.
                session.vision_reader = None;

                if session.ocr_reader.is_none() {
                    emit(ReviewEngineEvent::Status(
                        session.snapshot(EnginePhase::LoadingModel(StageKind::Ocr), true),
                    ));
                    match ocr_loader(cfg.ocr_models_dir.as_deref(), cfg.device) {
                        Ok(r) => session.ocr_reader = Some(r),
                        Err(reason) => {
                            session.paused_stages.insert(StageKind::Ocr, reason);
                            emit(ReviewEngineEvent::Status(session.snapshot(
                                EnginePhase::Running {
                                    done: session.session_done,
                                    total: total_estimate,
                                    stage: StageKind::Ocr,
                                    elapsed_secs: started.elapsed().as_secs_f64(),
                                },
                                false,
                            )));
                        }
                    }
                }

                if let Some(reader) = session.ocr_reader.take() {
                    let matcher = match cat.load_work_matcher() {
                        Ok(m) => m,
                        Err(e) => {
                            if !both_gpu_stages {
                                session.ocr_reader = Some(reader);
                            }
                            session.catalog_error(&e);
                            continue;
                        }
                    };
                    for item in pending {
                        if should_interrupt(&shared, cfg_gen) {
                            break;
                        }
                        emit(ReviewEngineEvent::Status(session.snapshot(
                            EnginePhase::Running {
                                done: session.session_done,
                                total: total_estimate.max(session.session_done + 1),
                                stage: StageKind::Ocr,
                                elapsed_secs: started.elapsed().as_secs_f64(),
                            },
                            true,
                        )));

                        let outcome =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                reader.read_image(&item.path)
                            }))
                            .unwrap_or_else(|_| Err("error interno al leer la imagen".into()));

                        match outcome {
                            Ok(lines) => {
                                match cat
                                    .save_ocr_lines(item.asset, &lines)
                                    .and_then(|()| cat.apply_work_matches(item.asset, &matcher))
                                {
                                    Ok(_) => {
                                        let reviewed = cat
                                            .is_asset_reviewed(item.asset, &enabled_specs)
                                            .unwrap_or(false);
                                        if reviewed {
                                            session.session_done += 1;
                                            total_estimate =
                                                total_estimate.max(session.session_done);
                                        }
                                        emit(ReviewEngineEvent::AssetProgress {
                                            asset: item.asset,
                                            now_reviewed: reviewed,
                                        });
                                    }
                                    Err(e) => {
                                        session.failed.insert((item.asset, StageKind::Ocr));
                                        session.catalog_error(&e);
                                        break;
                                    }
                                }
                            }
                            Err(reason) => {
                                session.record_failure(
                                    &cat,
                                    item.asset,
                                    StageKind::Ocr,
                                    &item.path,
                                    &reason,
                                );
                            }
                        }
                    }
                    if !both_gpu_stages {
                        session.ocr_reader = Some(reader);
                    }
                }
            }
        }

        emit(ReviewEngineEvent::ChunkFinished);
    }
}

fn wait_for_change(shared: &(Mutex<EngineControl>, Condvar), observed_gen: u64) {
    let (lock, cvar) = shared;
    let mut ctrl = lock.lock().unwrap_or_else(|e| e.into_inner());
    while !ctrl.shutdown
        && ctrl.config_gen == observed_gen
        && !ctrl.needs_index
        && ctrl.priority.is_empty()
        && !ctrl.clear_failed
        && ctrl.retry_assets.is_empty()
        && ctrl.clear_paused_stages.is_empty()
        && !ctrl.paused
        && !ctrl.yielded
    {
        ctrl = cvar.wait(ctrl).unwrap_or_else(|e| e.into_inner());
    }
}

/// Waits out a catalog failure. Pausing, quitting, retrying or changing the settings ends the wait.
fn back_off(shared: &(Mutex<EngineControl>, Condvar), observed_gen: u64) {
    let (lock, cvar) = shared;
    let ctrl = lock.lock().unwrap_or_else(|e| e.into_inner());
    let wait = ctrl.config.error_backoff;
    drop(
        cvar.wait_timeout_while(ctrl, wait, |c| {
            !c.shutdown && !c.paused && !c.clear_failed && c.config_gen == observed_gen
        })
        .unwrap_or_else(|e| e.into_inner()),
    );
}

/// Of `ids`, in order, those that still need one of `kinds` and have a copy in a registered folder.
fn unfinished(
    cat: &Catalog,
    ids: &[AssetId],
    kinds: &[StageKind],
    failed: &HashSet<(AssetId, StageKind)>,
    roots: &[PathBuf],
) -> Vec<AssetId> {
    let mut left: HashSet<AssetId> = HashSet::new();
    for &kind in kinds {
        if let Ok(pending) =
            cat.pending_in_assets_in_roots(kind.as_str(), kind.version(), ids, roots)
        {
            left.extend(
                pending
                    .into_iter()
                    .map(|item| item.asset)
                    .filter(|&id| !failed.contains(&(id, kind))),
            );
        }
    }
    ids.iter().copied().filter(|id| left.contains(id)).collect()
}

/// Puts back, ahead of the queue, the requested photos that a turn did not finish: a pause, a
/// change of settings or an error must not silently drop what the user asked for.
fn requeue(
    cat: &Catalog,
    shared: &(Mutex<EngineControl>, Condvar),
    session: &SessionState,
    taken: Vec<AssetId>,
) {
    let (kinds, roots) = {
        let ctrl = shared.0.lock().unwrap_or_else(|e| e.into_inner());
        let mut kinds = ctrl.config.enabled_stage_kinds();
        // A stage that cannot load stays out, or its photos would come back for ever.
        kinds.retain(|kind| !session.paused_stages.contains_key(kind));
        (kinds, ctrl.config.roots.clone())
    };
    let left = unfinished(cat, &taken, &kinds, &session.failed, &roots);
    let mut ctrl = shared.0.lock().unwrap_or_else(|e| e.into_inner());
    let queued: HashSet<AssetId> = ctrl.priority.iter().copied().collect();
    for id in left.into_iter().rev() {
        if !queued.contains(&id) {
            ctrl.priority.push_front(id);
        }
    }
}

fn count_remaining_work(
    cat: &Catalog,
    active_specs: &[StageSpec],
    cfg: &ReviewConfig,
    shared: &(Mutex<EngineControl>, Condvar),
    session: &SessionState,
    current_chunk_len: usize,
) -> usize {
    if cfg.enabled {
        let pending = cat
            .reviewed_counts(active_specs, Some(&cfg.roots))
            .map(|c| c.pending)
            .unwrap_or(current_chunk_len);
        pending
            .saturating_sub(session.failed_assets_count())
            .max(current_chunk_len)
    } else {
        let queued = shared
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .priority
            .len();
        current_chunk_len.saturating_add(queued)
    }
}

fn run_vision_batch(
    reader: &mut dyn VisionReader,
    decoded: &[(AssetId, Vec<RgbImage>)],
) -> Vec<(AssetId, Result<VisionImageOutput, String>)> {
    let all_single = decoded.iter().all(|(_, f)| f.len() == 1);
    if all_single {
        let img_refs: Vec<&RgbImage> = decoded.iter().map(|(_, f)| &f[0]).collect();
        let batch_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            reader.analyze_images(&img_refs)
        }))
        .unwrap_or_else(|_| Err("error interno en inferencia de visión".into()));

        match batch_res {
            Ok(outputs) if outputs.len() == decoded.len() => decoded
                .iter()
                .zip(outputs)
                .map(|((asset, _), out)| (*asset, Ok(out)))
                .collect(),
            _ => decoded
                .iter()
                .map(|(asset, frames)| {
                    let single = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        reader.analyze_asset_media(frames)
                    }))
                    .unwrap_or_else(|_| Err("error interno en inferencia de visión".into()));
                    (*asset, single)
                })
                .collect(),
        }
    } else {
        decoded
            .iter()
            .map(|(asset, frames)| {
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    reader.analyze_asset_media(frames)
                }))
                .unwrap_or_else(|_| Err("error interno en inferencia de visión".into()));
                (*asset, res)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_chunks_reload_the_interface_at_a_limited_rate_and_at_once_when_idle() {
        let working = EnginePhase::Running {
            done: 1,
            total: 9,
            stage: StageKind::Rules,
            elapsed_secs: 1.0,
        };
        let soon = Duration::from_millis(200);
        assert!(!chunk_reload_due(true, soon, &working));
        assert!(chunk_reload_due(true, CHUNK_RELOAD_INTERVAL, &working));
        assert!(chunk_reload_due(true, soon, &EnginePhase::Idle));
        assert!(chunk_reload_due(true, soon, &EnginePhase::Paused));
        assert!(!chunk_reload_due(
            false,
            CHUNK_RELOAD_INTERVAL,
            &EnginePhase::Idle
        ));
    }
}
