use crate::model::Selection;
use crate::settings::{self, Settings, SettingsError};
use crate::worker::Notify;
use catalog::{AssetId, Assignment, Catalog, CatalogError, Entry};
use export::{
    Control, ExportItem, ExportOptions, ExportProgress, ExportReport, Layout, Outcome, Preview,
    export_with_options, preview_with_options,
};
use fsguard::{DestPath, GuardError, LibraryRoot, ensure_export_dir};
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

/// Validates a destination folder with `fsguard::ensure_export_dir`, returning clear Spanish errors
/// when the path is empty, invalid, inside a library, or contains a library.
pub fn validate_destination(path: &Path, roots: &[LibraryRoot]) -> Result<DestPath, String> {
    if path.as_os_str().is_empty() {
        return Err("Elige una carpeta de destino para exportar.".into());
    }
    match ensure_export_dir(path, roots) {
        Ok(dest) => Ok(dest),
        Err(GuardError::InsideLibrary { root, .. }) => Err(format!(
            "No se puede exportar dentro de una biblioteca ({}). Elige una carpeta fuera de todas las bibliotecas.",
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
    pub dest: PathBuf,
    pub copied: usize,
    pub already_present: usize,
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
        let mut already_present = 0usize;
        let mut failed = Vec::new();
        for (src, outcome) in report.outcomes {
            match outcome {
                Outcome::Copied(_) => copied += 1,
                Outcome::AlreadyPresent(_) => already_present += 1,
                Outcome::Failed(reason) => failed.push((src, reason)),
            }
        }
        Self {
            dest,
            copied,
            already_present,
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
    Running { done: usize, total: usize },
    Done(ExportSummary),
    Error(String),
}

enum ExportWorkerEvent {
    Progress(ExportProgress),
    Finished(ExportReport),
    Failed(String),
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
        notify: Option<Notify>,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_bg = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();
        let join = thread::spawn(move || {
            let res = export_with_options(&items, &dest, options, &mut |p| {
                let _ = tx.send(ExportWorkerEvent::Progress(*p));
                if let Some(cb) = &notify {
                    cb();
                }
                if cancel_bg.load(Ordering::Relaxed) {
                    Control::Stop
                } else {
                    Control::Continue
                }
            });
            match res {
                Ok(report) => {
                    let _ = tx.send(ExportWorkerEvent::Finished(report));
                }
                Err(e) => {
                    let _ = tx.send(ExportWorkerEvent::Failed(e.to_string()));
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

/// State machine for the export wizard: `Choose -> Preview -> Running -> Done / Error`.
/// Computing previews and copying files both happen on background threads; nothing is copied
/// until [`Self::start_export`] is called explicitly from a ready preview that fits on disk.
pub struct ExportWizard {
    items: Vec<ExportItem>,
    from_selection: bool,
    dest_input: PathBuf,
    validated_dest: Option<DestPath>,
    layout: Layout,
    write_xmp: bool,
    embed_jpeg_tags: bool,
    phase: ExportPhase,
    validation_error: Option<String>,
    cancel_requested: bool,
    preview_gen: u64,
    preview_rx: Option<mpsc::Receiver<(u64, Result<Preview, String>)>>,
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
            export_worker: None,
            focus_dest: true,
        }
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
        matches!(self.phase, ExportPhase::Preview(PreviewState::Ready(ref p)) if p.fits())
            && self.validated_dest.is_some()
            && !self.items.is_empty()
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
        if self.is_running() || self.embed_jpeg_tags == val {
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
        notify: Option<Notify>,
    ) -> Result<DestPath, String> {
        if self.is_running() {
            return Err("Ya hay una exportación en curso.".into());
        }
        if self.items.is_empty() {
            let msg = "No hay imágenes para exportar.".to_string();
            self.validation_error = Some(msg.clone());
            return Err(msg);
        }
        let dest = match validate_destination(&self.dest_input, roots) {
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
            let res = preview_with_options(&items, &dest_bg, options).map_err(|e| e.to_string());
            let _ = tx.send((gen_id, res));
            if let Some(cb) = &notify {
                cb();
            }
        });

        self.preview_rx = Some(rx);
        self.phase = ExportPhase::Preview(PreviewState::Computing);
        Ok(dest)
    }

    /// Starts the actual copy on a background thread. Only succeeds after a ready preview that fits.
    pub fn start_export(&mut self, notify: Option<Notify>) -> Result<(), String> {
        let preview = match &self.phase {
            ExportPhase::Preview(PreviewState::Ready(p)) => p,
            _ => return Err("Calcula la vista previa antes de exportar.".into()),
        };
        if !preview.fits() {
            return Err("No hay espacio libre suficiente en la carpeta de destino.".into());
        }
        let Some(dest) = self.validated_dest.clone() else {
            return Err("Valida la carpeta de destino antes de exportar.".into());
        };
        if self.items.is_empty() {
            return Err("No hay imágenes para exportar.".into());
        }

        self.cancel_requested = false;
        let total = self.items.len();
        self.export_worker = Some(ExportWorker::start(
            self.items.clone(),
            dest,
            self.options(),
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
                Ok((gen_id, res)) => {
                    self.preview_rx = None;
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
                    ExportWorkerEvent::Finished(report) => {
                        let dest = self
                            .validated_dest
                            .as_ref()
                            .map(|d| d.as_path().to_path_buf())
                            .unwrap_or_else(|| self.dest_input.clone());
                        self.phase = ExportPhase::Done(ExportSummary::from_report(dest, report));
                        finish_worker = true;
                    }
                    ExportWorkerEvent::Failed(e) => {
                        self.phase =
                            ExportPhase::Error(format!("Error durante la exportación: {e}"));
                        finish_worker = true;
                    }
                }
            }
        }
        if finish_worker {
            self.export_worker = None;
            self.cancel_requested = false;
        }

        changed
    }
}
