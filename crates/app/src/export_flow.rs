use crate::model::{Selection, count_phrase, plural_word};
use crate::settings::{self, Settings, SettingsError};
use crate::worker::Notify;
use catalog::{AssetId, Assignment, Catalog, CatalogError, Entry};
use export::{
    Control, ExportItem, ExportOptions, ExportProgress, ExportReport, Layout, MoveContext,
    Operation, Outcome, Preview, UndoReport, export_with_options, move_with_options,
    preview_with_options, undo_last_move,
};
use fsguard::{
    DestPath, GuardError, LibraryRoot, MoveSource, ensure_export_dir, ensure_move_dir, io_reason,
};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

pub const ALL_LAYOUTS: [Layout; 5] = [
    Layout::Flat,
    Layout::ByCategory,
    Layout::ByEntity,
    Layout::ByYear,
    Layout::PreserveFolders,
];

pub fn layout_label(layout: Layout) -> &'static str {
    match layout {
        Layout::Flat => "Plana",
        Layout::ByCategory => "Por categoría",
        Layout::ByEntity => "Por obra",
        Layout::ByYear => "Por año",
        Layout::PreserveFolders => "Conservar carpetas",
    }
}

pub fn layout_description(layout: Layout) -> &'static str {
    match layout {
        Layout::Flat => "Todas las copias directamente en la carpeta de destino.",
        Layout::ByCategory => {
            "Una subcarpeta por la primera categoría de cada imagen (o «Sin categoría»)."
        }
        Layout::ByEntity => {
            "Una subcarpeta por la primera obra o entidad de cada imagen (o «Sin obra»)."
        }
        Layout::ByYear => "Una subcarpeta por el año de modificación del archivo (o «Sin fecha»).",
        Layout::PreserveFolders => {
            "Replica la estructura de carpetas original de cada foto respecto a su biblioteca."
        }
    }
}

/// Returns the currently selected entries, or all visible filtered entries when nothing is selected.
pub fn select_entries<'a>(entries: &'a [Entry], selection: &Selection) -> Vec<&'a Entry> {
    if selection.is_empty() {
        entries.iter().collect()
    } else {
        selection
            .selected_indices()
            .into_iter()
            .filter_map(|idx| entries.get(idx))
            .collect()
    }
}

#[derive(Clone)]
struct CachedAssetInfo {
    labels: Vec<Assignment>,
    belongs_to: Option<catalog::AssetWork>,
    persons: Vec<String>,
    hierarchical_categories: Vec<String>,
}

/// Builds export items by pairing each catalog entry with its catalog label assignments.
pub fn build_export_items(
    catalog: &Catalog,
    entries: &[&Entry],
) -> Result<Vec<ExportItem>, CatalogError> {
    let mut cache: HashMap<AssetId, CachedAssetInfo> = HashMap::new();
    let mut items = Vec::with_capacity(entries.len());
    for entry in entries {
        let info = match cache.get(&entry.asset) {
            Some(cached) => cached.clone(),
            None => {
                let loaded = CachedAssetInfo {
                    labels: catalog.labels_of(entry.asset)?,
                    belongs_to: catalog.asset_work(entry.asset)?,
                    persons: catalog.named_persons_of(entry.asset)?,
                    hierarchical_categories: catalog.hierarchical_categories_of(entry.asset)?,
                };
                cache.insert(entry.asset, loaded.clone());
                loaded
            }
        };
        let root = catalog.root_of_path(&entry.path)?;
        items.push(ExportItem {
            source: entry.path.clone(),
            labels: info.labels,
            belongs_to: info.belongs_to,
            persons: info.persons,
            hierarchical_categories: info.hierarchical_categories,
            root,
        });
    }
    Ok(items)
}

/// Validates a destination folder, returning clear Spanish errors. Copies must go outside every
/// library, with `fsguard::ensure_export_dir`; moves may sort inside them, but never into `app_data`.
pub fn validate_destination(
    path: &Path,
    roots: &[LibraryRoot],
    operation: Operation,
    app_data: &Path,
) -> Result<DestPath, String> {
    if path.as_os_str().is_empty() {
        return Err("Elige una carpeta de destino.".into());
    }
    let checked = match operation {
        Operation::Copy => ensure_export_dir(path, roots),
        Operation::Move => ensure_move_dir(path, app_data),
    };
    match checked {
        Ok(dest) => Ok(dest),
        Err(GuardError::InsideLibrary { root, .. }) => Err(format!(
            "No se puede copiar dentro de una biblioteca ({}). Elige una carpeta fuera de todas las bibliotecas.",
            root.display()
        )),
        Err(GuardError::ContainsLibrary { root, .. }) => Err(format!(
            "La carpeta elegida contiene la biblioteca {}. Elige una carpeta independiente fuera de tus bibliotecas.",
            root.display()
        )),
        Err(GuardError::ParentDirComponent(_)) => {
            Err("La ruta de destino no puede contener «..». Elige una carpeta válida.".into())
        }
        Err(e) => Err(format!("No se pudo validar la carpeta de destino: {e}")),
    }
}

/// What the second confirmation of a move says: `photos` leave their folders, and `recycled` of
/// them go to another disk.
pub fn move_confirmation_text(photos: usize, recycled: usize) -> String {
    let bin = if recycled == 0 {
        String::new()
    } else {
        format!(
            " {recycled} {} a la Papelera después de copiarse y verificarse; si Windows fuera a \
             borrar alguna para siempre, te preguntará antes. Las de memorias USB o de red \
             conservan el original.",
            plural_word(recycled, "irá", "irán"),
        )
    };
    format!(
        "{} {} fuera de {}.{bin} Puedes deshacerlo.",
        plural_word(photos, "Se moverá", "Se moverán"),
        count_phrase(photos, "foto", "fotos"),
        plural_word(photos, "su carpeta", "sus carpetas"),
    )
}

/// True when every photo is directly in `dir` already.
fn all_in(items: &[ExportItem], dir: &Path) -> bool {
    let dir = dir.to_string_lossy().to_lowercase();
    items.iter().all(|item| {
        item.source
            .parent()
            .is_some_and(|p| p.to_string_lossy().to_lowercase() == dir)
    })
}

/// Remembers `last_export_dir` using the existing settings persistence API.
pub fn remember_export_dir(
    data_dir: &Path,
    settings: &mut Settings,
    dest: &DestPath,
) -> Result<(), SettingsError> {
    let chosen = dest.as_path().to_path_buf();
    if settings.last_export_dir.as_ref() != Some(&chosen) {
        settings.last_export_dir = Some(chosen);
        settings::save(data_dir, settings)?;
    }
    Ok(())
}

/// Opens the destination folder in Windows Explorer.
#[cfg(windows)]
pub fn open_in_explorer(dir: &Path) -> io::Result<()> {
    std::process::Command::new("explorer.exe")
        .arg(dir)
        .spawn()
        .map(|_| ())
}

#[cfg(not(windows))]
pub fn open_in_explorer(_dir: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Abrir carpeta solo está disponible en Windows.",
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewState {
    Computing,
    Ready(Preview),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSummary {
    pub operation: Operation,
    pub dest: PathBuf,
    pub copied: usize,
    pub moved: usize,
    /// Copied by a move that left the original where it was, with the reason.
    pub kept_original: Vec<(PathBuf, String)>,
    pub already_present: usize,
    /// Not exported because the guard refused to follow a link in the destination.
    pub blocked: Vec<(PathBuf, String)>,
    pub failed: Vec<(PathBuf, String)>,
    /// Exported files where something optional (XMP sidecar, embedded tags) did not happen.
    pub warnings: Vec<(PathBuf, String)>,
    pub stopped: bool,
    pub manifest_json: PathBuf,
    pub manifest_csv: PathBuf,
}

impl ExportSummary {
    pub fn from_report(dest: PathBuf, report: ExportReport) -> Self {
        let mut copied = 0usize;
        let mut moved = 0usize;
        let mut kept_original = Vec::new();
        let mut already_present = 0usize;
        let mut blocked = Vec::new();
        let mut failed = Vec::new();
        for (src, outcome) in report.outcomes {
            match outcome {
                Outcome::Copied(_) => copied += 1,
                Outcome::AlreadyPresent(_) => already_present += 1,
                Outcome::Blocked(reason) => blocked.push((src, reason)),
                Outcome::Failed(reason) => failed.push((src, reason)),
                Outcome::Moved(_) => moved += 1,
                Outcome::KeptOriginal { reason, .. } => kept_original.push((src, reason)),
            }
        }
        Self {
            // A report does not say which it was: the wizard sets it after a move.
            operation: Operation::Copy,
            dest,
            copied,
            moved,
            kept_original,
            already_present,
            blocked,
            failed,
            warnings: report.warnings,
            stopped: report.stopped,
            manifest_json: report.manifest_json,
            manifest_csv: report.manifest_csv,
        }
    }

    pub fn manifest_json_name(&self) -> String {
        file_name_or_full(&self.manifest_json)
    }

    pub fn manifest_csv_name(&self) -> String {
        file_name_or_full(&self.manifest_csv)
    }
}

fn file_name_or_full(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportPhase {
    Choose,
    Preview(PreviewState),
    /// The second confirmation a move asks for, with the preview it is about.
    ConfirmMove(Preview),
    Running {
        done: usize,
        total: usize,
    },
    Done(ExportSummary),
    Error(String),
}

/// Of `paths`, those that are no longer there. Runs on the background threads.
fn gone_among<'a>(paths: impl Iterator<Item = &'a PathBuf>) -> Vec<PathBuf> {
    paths
        .filter(|p| catalog::file_is_gone(p))
        .cloned()
        .collect()
}

type PreviewMessage = (u64, Result<Preview, String>, Vec<PathBuf>);

enum ExportWorkerEvent {
    Progress(ExportProgress),
    Finished(ExportReport, Vec<PathBuf>),
    Failed(String),
}

/// `fsguard::recycle` outside tests.
pub type Retire = fn(&MoveSource) -> io::Result<bool>;

/// What the background thread does with the items.
enum Job {
    Copy,
    /// Moving needs its own connection to the catalog and the libraries.
    Move {
        db: DestPath,
        roots: Vec<LibraryRoot>,
        retire: Retire,
    },
}

struct ExportWorker {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<ExportWorkerEvent>,
    join: Option<JoinHandle<()>>,
}

impl ExportWorker {
    fn start(
        items: Vec<ExportItem>,
        dest: DestPath,
        options: ExportOptions,
        job: Job,
        notify: Option<Notify>,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_bg = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let join = thread::spawn(move || {
            let mut progress = |p: &ExportProgress| {
                let _ = tx.send(ExportWorkerEvent::Progress(*p));
                if let Some(cb) = &notify {
                    cb();
                }
                if cancel_bg.load(Ordering::Relaxed) {
                    Control::Stop
                } else {
                    Control::Continue
                }
            };
            let res = match job {
                Job::Copy => export_with_options(&items, &dest, options, &mut progress),
                Job::Move { db, roots, retire } => Catalog::open(&db)
                    .map_err(|e| io::Error::other(e.to_string()))
                    .and_then(|catalog| {
                        let context = MoveContext {
                            catalog: &catalog,
                            roots: &roots,
                            retire: &retire,
                            force_cross_volume: false,
                            before_retire: None,
                        };
                        move_with_options(&items, &dest, options, &context, &mut progress)
                    }),
            };
            match res {
                Ok(report) => {
                    let failed = report
                        .outcomes
                        .iter()
                        .filter(|(_, outcome)| matches!(outcome, Outcome::Failed(_)));
                    let gone = gone_among(failed.map(|(source, _)| source));
                    let _ = tx.send(ExportWorkerEvent::Finished(report, gone));
                }
                Err(e) => {
                    let _ = tx.send(ExportWorkerEvent::Failed(io_reason(&e)));
                }
            }
            if let Some(cb) = &notify {
                cb();
            }
        });
        Self {
            cancel,
            rx,
            join: Some(join),
        }
    }

    fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn try_recv(&self) -> Option<ExportWorkerEvent> {
        self.rx.try_recv().ok()
    }
}

impl Drop for ExportWorker {
    fn drop(&mut self) {
        self.cancel();
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

/// Brings the last move back on a background thread. Dropping it waits for it to finish.
pub struct UndoWorker {
    rx: mpsc::Receiver<Result<Option<UndoReport>, String>>,
    join: Option<JoinHandle<()>>,
}

impl UndoWorker {
    pub fn start(
        db: DestPath,
        roots: Vec<LibraryRoot>,
        app_data: PathBuf,
        retire: Retire,
        notify: Option<Notify>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let join = thread::spawn(move || {
            let result = Catalog::open(&db)
                .map_err(|e| e.to_string())
                .and_then(|catalog| {
                    let context = MoveContext {
                        catalog: &catalog,
                        roots: &roots,
                        retire: &retire,
                        force_cross_volume: false,
                        before_retire: None,
                    };
                    undo_last_move(&context, &app_data).map_err(|e| io_reason(&e))
                });
            let _ = tx.send(result);
            if let Some(cb) = &notify {
                cb();
            }
        });
        Self {
            rx,
            join: Some(join),
        }
    }

    /// The result, once the undo is over.
    pub fn try_recv(&self) -> Option<Result<Option<UndoReport>, String>> {
        match self.rx.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("se detuvo inesperadamente".into())),
        }
    }
}

impl Drop for UndoWorker {
    fn drop(&mut self) {
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

/// What to tell after an undo: how many came back and, apart, anything that needs attention.
pub fn undo_messages(report: &UndoReport) -> (String, Option<String>) {
    const SHOWN: usize = 3;
    let n = report.restored.len();
    let done = format!(
        "{} {}",
        plural_word(n, "Se restauró", "Se restauraron"),
        count_phrase(n, "foto", "fotos")
    );
    let list = |files: &[(PathBuf, String)]| {
        let mut text = files
            .iter()
            .take(SHOWN)
            .map(|(path, why)| format!("{} ({why})", file_name_or_full(path)))
            .collect::<Vec<_>>()
            .join("; ");
        if files.len() > SHOWN {
            text.push_str(&format!(" y {} más", files.len() - SHOWN));
        }
        text
    };
    let mut problems = Vec::new();
    if !report.skipped.is_empty() {
        let k = report.skipped.len();
        problems.push(format!(
            "{k} no se {} restaurar: {}",
            plural_word(k, "pudo", "pudieron"),
            list(&report.skipped)
        ));
    }
    if !report.lost.is_empty() {
        let k = report.lost.len();
        problems.push(format!(
            "{k} ya no se {} deshacer: {}",
            plural_word(k, "puede", "pueden"),
            list(&report.lost)
        ));
    }
    if !report.warnings.is_empty() {
        problems.push(format!("Aviso: {}", list(&report.warnings)));
    }
    (done, (!problems.is_empty()).then(|| problems.join(". ")))
}

/// State machine for the «Copiar o mover» wizard: `Choose -> Preview -> Running -> Done / Error`,
/// with `ConfirmMove` before `Running` when moving. Computing previews, copying and moving happen on
/// background threads; nothing is touched until [`Self::start_export`] is called explicitly from a
/// preview that fits on disk and, to move, after that second confirmation.
pub struct ExportWizard {
    items: Vec<ExportItem>,
    from_selection: bool,
    operation: Operation,
    retire: Retire,
    /// A move ran and the application has not reloaded the gallery yet.
    finished_move: bool,
    dest_input: PathBuf,
    validated_dest: Option<DestPath>,
    layout: Layout,
    write_xmp: bool,
    embed_jpeg_tags: bool,
    phase: ExportPhase,
    validation_error: Option<String>,
    cancel_requested: bool,
    preview_gen: u64,
    preview_rx: Option<mpsc::Receiver<PreviewMessage>>,
    /// Sources found to be gone, until the application takes them.
    gone: Vec<PathBuf>,
    export_worker: Option<ExportWorker>,
    focus_dest: bool,
}

impl ExportWizard {
    /// True once, in the first frame of the window: the keyboard focus starts inside it.
    pub fn take_focus_dest(&mut self) -> bool {
        std::mem::take(&mut self.focus_dest)
    }

    pub fn open(
        catalog: &Catalog,
        entries: &[Entry],
        selection: &Selection,
        last_export_dir: Option<PathBuf>,
    ) -> Self {
        let from_selection = !selection.is_empty();
        let chosen = select_entries(entries, selection);
        match build_export_items(catalog, &chosen) {
            Ok(items) => Self::from_items(items, from_selection, last_export_dir),
            Err(e) => {
                let mut w = Self::from_items(Vec::new(), from_selection, last_export_dir);
                w.phase = ExportPhase::Error(format!(
                    "No se pudieron leer las etiquetas del catálogo: {e}"
                ));
                w
            }
        }
    }

    pub fn from_items(
        items: Vec<ExportItem>,
        from_selection: bool,
        last_export_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            items,
            from_selection,
            operation: Operation::Copy,
            retire: fsguard::recycle,
            finished_move: false,
            dest_input: last_export_dir.unwrap_or_default(),
            validated_dest: None,
            layout: Layout::Flat,
            write_xmp: false,
            embed_jpeg_tags: false,
            phase: ExportPhase::Choose,
            validation_error: None,
            cancel_requested: false,
            preview_gen: 0,
            preview_rx: None,
            gone: Vec::new(),
            export_worker: None,
            focus_dest: true,
        }
    }

    pub fn operation(&self) -> Operation {
        self.operation
    }

    pub fn set_operation(&mut self, operation: Operation) {
        if self.is_running() || self.operation == operation {
            return;
        }
        self.operation = operation;
        if operation == Operation::Move {
            // It would change the bytes of the only copy.
            self.embed_jpeg_tags = false;
        }
        self.validation_error = None;
        self.invalidate_preview();
    }

    /// Replaces the Recycle Bin. For tests: they must never reach the real one.
    pub fn set_retire(&mut self, retire: Retire) {
        self.retire = retire;
    }

    /// True once after each move ends, however it ended: the gallery has to be read again.
    pub fn take_finished_move(&mut self) -> bool {
        std::mem::take(&mut self.finished_move)
    }

    pub fn is_moving(&self) -> bool {
        self.is_running() && self.operation == Operation::Move
    }

    pub fn can_confirm_move(&self) -> bool {
        self.operation == Operation::Move
            && matches!(self.phase, ExportPhase::Preview(PreviewState::Ready(ref p))
                if p.fits() && p.files > 0)
    }

    pub fn request_move_confirmation(&mut self) -> Result<(), String> {
        match &self.phase {
            ExportPhase::Preview(PreviewState::Ready(p)) if self.can_confirm_move() => {
                self.phase = ExportPhase::ConfirmMove(p.clone());
                Ok(())
            }
            _ => Err("No hay nada que mover con esta vista previa.".into()),
        }
    }

    pub fn back_to_preview(&mut self) {
        if let ExportPhase::ConfirmMove(p) = &self.phase {
            self.phase = ExportPhase::Preview(PreviewState::Ready(p.clone()));
        }
    }

    /// Sources that the preview or the export did not find, so the catalog can flag them.
    pub fn take_gone(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.gone)
    }

    pub fn phase(&self) -> &ExportPhase {
        &self.phase
    }

    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    pub fn from_selection(&self) -> bool {
        self.from_selection
    }

    pub fn dest_input(&self) -> &Path {
        &self.dest_input
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    pub fn write_xmp(&self) -> bool {
        self.write_xmp
    }

    pub fn embed_jpeg_tags(&self) -> bool {
        self.embed_jpeg_tags
    }

    pub fn options(&self) -> ExportOptions {
        ExportOptions {
            layout: self.layout,
            write_xmp_sidecars: self.write_xmp,
            embed_jpeg_tags: self.embed_jpeg_tags,
            operation: self.operation,
        }
    }

    pub fn validation_error(&self) -> Option<&str> {
        self.validation_error.as_deref()
    }

    pub fn is_cancel_requested(&self) -> bool {
        self.cancel_requested
    }

    pub fn is_running(&self) -> bool {
        matches!(self.phase, ExportPhase::Running { .. })
    }

    pub fn can_start_export(&self) -> bool {
        self.confirmed_preview().is_some_and(Preview::fits)
            && self.validated_dest.is_some()
            && !self.items.is_empty()
    }

    /// The preview the user has said yes to: a ready one to copy, a confirmed one to move.
    fn confirmed_preview(&self) -> Option<&Preview> {
        match (&self.phase, self.operation) {
            (ExportPhase::Preview(PreviewState::Ready(p)), Operation::Copy) => Some(p),
            (ExportPhase::ConfirmMove(p), Operation::Move) => Some(p),
            _ => None,
        }
    }

    pub fn set_dest(&mut self, path: PathBuf) {
        if self.is_running() {
            return;
        }
        self.dest_input = path;
        self.validation_error = None;
        self.invalidate_preview();
    }

    pub fn set_layout(&mut self, layout: Layout) {
        if self.is_running() || self.layout == layout {
            return;
        }
        self.layout = layout;
        self.invalidate_preview();
    }

    pub fn set_write_xmp(&mut self, val: bool) {
        if self.is_running() || self.write_xmp == val {
            return;
        }
        self.write_xmp = val;
        self.invalidate_preview();
    }

    pub fn set_embed_jpeg_tags(&mut self, val: bool) {
        if self.is_running() || self.operation == Operation::Move || self.embed_jpeg_tags == val {
            return;
        }
        self.embed_jpeg_tags = val;
        self.invalidate_preview();
    }

    pub fn back_to_choose(&mut self) {
        if self.is_running() {
            return;
        }
        self.invalidate_preview();
    }

    fn invalidate_preview(&mut self) {
        self.preview_gen = self.preview_gen.wrapping_add(1);
        self.preview_rx = None;
        self.validated_dest = None;
        if !matches!(self.phase, ExportPhase::Choose) {
            self.phase = ExportPhase::Choose;
        }
    }

    /// Validates the destination and computes the preview off the UI thread.
    pub fn request_preview(
        &mut self,
        roots: &[LibraryRoot],
        app_data: &Path,
        notify: Option<Notify>,
    ) -> Result<DestPath, String> {
        if self.is_running() {
            return Err("Ya hay una copia o un movimiento en curso.".into());
        }
        if self.items.is_empty() {
            let msg = "No hay imágenes seleccionadas.".to_string();
            self.validation_error = Some(msg.clone());
            return Err(msg);
        }
        let checked = validate_destination(&self.dest_input, roots, self.operation, app_data)
            .and_then(|dest| {
                if self.operation == Operation::Move && all_in(&self.items, dest.as_path()) {
                    Err("Las fotos ya están en esa carpeta.".to_string())
                } else {
                    Ok(dest)
                }
            });
        let dest = match checked {
            Ok(d) => d,
            Err(err) => {
                self.validation_error = Some(err.clone());
                self.phase = ExportPhase::Choose;
                return Err(err);
            }
        };

        self.validation_error = None;
        self.validated_dest = Some(dest.clone());
        self.preview_gen = self.preview_gen.wrapping_add(1);
        let gen_id = self.preview_gen;
        let items = self.items.clone();
        let options = self.options();
        let dest_bg = dest.clone();
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let res = preview_with_options(&items, &dest_bg, options).map_err(|e| io_reason(&e));
            let gone = if res.as_ref().is_ok_and(|p| p.unreadable > 0) {
                gone_among(items.iter().map(|item| &item.source))
            } else {
                Vec::new()
            };
            let _ = tx.send((gen_id, res, gone));
            if let Some(cb) = &notify {
                cb();
            }
        });

        self.preview_rx = Some(rx);
        self.phase = ExportPhase::Preview(PreviewState::Computing);
        Ok(dest)
    }

    /// Starts copying or moving on a background thread. Only succeeds after a ready preview that
    /// fits and, to move, its second confirmation. `db`, `roots` and `app_data` are for moving.
    pub fn start_export(
        &mut self,
        notify: Option<Notify>,
        db: DestPath,
        roots: Vec<LibraryRoot>,
        app_data: PathBuf,
    ) -> Result<(), String> {
        let Some(preview) = self.confirmed_preview() else {
            return Err("Calcula la vista previa y confírmala antes de empezar.".into());
        };
        if !preview.fits() {
            return Err("No hay espacio libre suficiente en la carpeta de destino.".into());
        }
        let Some(mut dest) = self.validated_dest.clone() else {
            return Err("Valida la carpeta de destino antes de empezar.".into());
        };
        if self.items.is_empty() {
            return Err("No hay imágenes seleccionadas.".into());
        }
        let job = match self.operation {
            Operation::Copy => Job::Copy,
            Operation::Move => {
                // The folder may have become a link since the preview.
                dest = validate_destination(dest.as_path(), &[], Operation::Move, &app_data)?;
                Job::Move {
                    db,
                    roots,
                    retire: self.retire,
                }
            }
        };

        self.cancel_requested = false;
        let total = self.items.len();
        self.export_worker = Some(ExportWorker::start(
            self.items.clone(),
            dest,
            self.options(),
            job,
            notify,
        ));
        self.phase = ExportPhase::Running { done: 0, total };
        Ok(())
    }

    pub fn cancel_export(&mut self) {
        if let Some(worker) = &self.export_worker {
            self.cancel_requested = true;
            worker.cancel();
        }
    }

    /// Polls background preview and export threads without blocking. Returns true if state changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;

        if let Some(rx) = &self.preview_rx {
            match rx.try_recv() {
                Ok((gen_id, res, gone)) => {
                    self.preview_rx = None;
                    self.gone.extend(gone);
                    if gen_id == self.preview_gen {
                        self.phase = match res {
                            Ok(preview) => ExportPhase::Preview(PreviewState::Ready(preview)),
                            Err(e) => ExportPhase::Error(format!(
                                "No se pudo calcular la vista previa: {e}"
                            )),
                        };
                        changed = true;
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.preview_rx = None;
                    if matches!(self.phase, ExportPhase::Preview(PreviewState::Computing)) {
                        self.phase = ExportPhase::Error(
                            "El cálculo de la vista previa se detuvo inesperadamente.".into(),
                        );
                        changed = true;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        let mut finish_worker = false;
        if let Some(worker) = &self.export_worker {
            while let Some(ev) = worker.try_recv() {
                changed = true;
                match ev {
                    ExportWorkerEvent::Progress(p) => {
                        self.phase = ExportPhase::Running {
                            done: p.done,
                            total: p.total,
                        };
                    }
                    ExportWorkerEvent::Finished(report, gone) => {
                        self.gone.extend(gone);
                        let dest = self
                            .validated_dest
                            .as_ref()
                            .map(|d| d.as_path().to_path_buf())
                            .unwrap_or_else(|| self.dest_input.clone());
                        let mut summary = ExportSummary::from_report(dest, report);
                        summary.operation = self.operation;
                        self.phase = ExportPhase::Done(summary);
                        finish_worker = true;
                    }
                    ExportWorkerEvent::Failed(e) => {
                        let what = match self.operation {
                            Operation::Copy => "la copia",
                            Operation::Move => "el movimiento",
                        };
                        self.phase = ExportPhase::Error(format!("Error durante {what}: {e}"));
                        finish_worker = true;
                    }
                }
            }
        }
        if finish_worker {
            // Even a failed move may have moved some files.
            self.finished_move |= self.operation == Operation::Move;
            self.export_worker = None;
            self.cancel_requested = false;
        }

        changed
    }
}
