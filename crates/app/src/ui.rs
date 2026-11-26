use crate::analysis_worker::{AnalysisEvent, AnalysisWorker};
use crate::model::{
    CHIP_FILTER_HELP, ChipMode, FilterState, GridLayout, MoveDir, Selection, count_phrase,
    format_bytes, format_chip_label, format_chip_tooltip, format_timestamp_utc, plural_word,
    selected_phrase,
};
use crate::review_flow::ReviewFlow;
use crate::settings::{self, Settings, Theme};
use crate::theme::{self, Palette};
use crate::views;
use crate::worker::{
    IndexEvent, IndexWorker, Notify, PreviewWorker, ThumbPool, ThumbRequest, ThumbnailProvider,
};
use catalog::{
    AssetId, Assignment, Catalog, Counts, DuplicateGroup, Entry, LabelCount, LabelKind, Sort,
    Status,
};
use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Modifiers, RichText,
    ScrollArea, Sense, Stroke, TextureHandle, TextureOptions, Vec2,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// shortcut: fixed cap on GPU thumbnail textures (FIFO eviction); upgrade to LRU if users pan
// back and forth across >1,000 visible items rapidly.
const MAX_THUMB_TEXTURES: usize = 1024;
const SEARCH_INPUT_ID: &str = "photosort_search_input";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveTab {
    Gallery,
    Review,
    Duplicates,
}

pub struct PhotosortApp {
    data_dir: PathBuf,
    settings: Settings,
    catalog: Option<Catalog>,
    active_tab: ActiveTab,
    entries: Vec<Entry>,
    counts: Counts,
    labels_in_use: Vec<LabelCount>,
    pending_review_assets: HashSet<AssetId>,
    review_flow: ReviewFlow,
    duplicate_groups: Vec<DuplicateGroup>,
    filter: FilterState,
    selection: Selection,
    last_columns: usize,
    viewer_open: bool,

    index_worker: Option<IndexWorker>,
    index_progress: Option<String>,
    analysis_worker: Option<AnalysisWorker>,
    analysis_progress: Option<String>,
    status_notice: Option<String>,
    banner_error: Option<String>,

    thumb_pool: ThumbPool,
    thumb_textures: HashMap<String, TextureHandle>,
    thumb_order: VecDeque<String>,
    thumb_failed: HashSet<String>,

    preview_worker: PreviewWorker,
    preview_requested: Option<PathBuf>,
    preview_loaded: Option<(PathBuf, Result<TextureHandle, String>)>,

    inspector_asset: Option<AssetId>,
    inspector_labels: Vec<Assignment>,
    new_label_kind: LabelKind,
    new_label_text: String,
    focus_search_next_frame: bool,
    export_wizard: Option<crate::export_flow::ExportWizard>,
    screenshot: Option<crate::screenshot::ScreenshotController>,
    last_thumbs_ready: bool,
}

impl PhotosortApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        data_dir: PathBuf,
        open_thumbnails: impl FnOnce(
            &Path,
            &[fsguard::LibraryRoot],
        ) -> (Arc<dyn ThumbnailProvider>, Option<String>),
    ) -> Self {
        let loaded = settings::load(&data_dir);
        let settings = loaded.settings;
        let (thumbs, thumbs_warning) = open_thumbnails(&data_dir, &settings.valid_roots());
        theme::configure_styles(&cc.egui_ctx, settings.theme);

        let ctx_for_notify = cc.egui_ctx.clone();
        let notify: Notify = Arc::new(move || ctx_for_notify.request_repaint());

        let mut banner_error = loaded
            .warning
            .or(thumbs_warning.map(|why| format!("Miniaturas desactivadas: {why}")));
        let catalog = match settings::db_dest_path(&data_dir, &settings.valid_roots())
            .map_err(|e| e.to_string())
            .and_then(|db| Catalog::open(&db).map_err(|e| e.to_string()))
        {
            Ok(c) => Some(c),
            Err(e) => {
                banner_error = Some(format!("No se pudo abrir el catálogo: {e}"));
                None
            }
        };

        let mut app = Self {
            data_dir,
            settings,
            catalog,
            active_tab: ActiveTab::Gallery,
            entries: Vec::new(),
            counts: Counts {
                assets: 0,
                locations: 0,
                missing: 0,
            },
            labels_in_use: Vec::new(),
            pending_review_assets: HashSet::new(),
            review_flow: ReviewFlow::new(),
            duplicate_groups: Vec::new(),
            filter: FilterState::default(),
            selection: Selection::default(),
            last_columns: 1,
            viewer_open: false,
            index_worker: None,
            index_progress: None,
            analysis_worker: None,
            analysis_progress: None,
            status_notice: None,
            banner_error,
            thumb_pool: ThumbPool::new(3, thumbs, Some(Arc::clone(&notify))),
            thumb_textures: HashMap::new(),
            thumb_order: VecDeque::new(),
            thumb_failed: HashSet::new(),
            preview_worker: PreviewWorker::new(theme::PREVIEW_MAX_EDGE, Some(notify)),
            preview_requested: None,
            preview_loaded: None,
            inspector_asset: None,
            inspector_labels: Vec::new(),
            new_label_kind: LabelKind::Tag,
            new_label_text: String::new(),
            focus_search_next_frame: false,
            export_wizard: None,
            screenshot: None,
            last_thumbs_ready: false,
        };
        app.reload_catalog();
        app
    }

    pub fn with_theme_override(mut self, ctx: &egui::Context, theme: Theme) -> Self {
        self.settings.theme = theme;
        theme::apply_theme_preference(ctx, theme);
        self
    }

    pub fn with_screenshot(mut self, controller: crate::screenshot::ScreenshotController) -> Self {
        self.screenshot = Some(controller);
        self
    }

    fn reload_catalog(&mut self) {
        let Some(cat) = &self.catalog else {
            return;
        };
        match cat.counts() {
            Ok(c) => self.counts = c,
            Err(e) => self.banner_error = Some(format!("Error al consultar contadores: {e}")),
        }
        match cat.labels_in_use() {
            Ok(list) => self.labels_in_use = list,
            Err(e) => self.banner_error = Some(format!("Error al cargar etiquetas: {e}")),
        }
        // Confidence is bounded in [0, 1], so 1.1 lists every undecided suggestion.
        if let Ok(queue) = cat.review_queue(1.1) {
            self.pending_review_assets = queue.into_iter().map(|i| i.asset).collect();
        }
        if let Err(e) = self.review_flow.reload(cat) {
            self.banner_error = Some(format!("Error al cargar cola de revisión: {e}"));
        }
        match cat.duplicate_groups() {
            Ok(groups) => self.duplicate_groups = groups,
            Err(e) => self.banner_error = Some(format!("Error al consultar duplicados: {e}")),
        }
        self.reload_entries();
    }

    fn reload_entries(&mut self) {
        let Some(cat) = &self.catalog else {
            return;
        };
        let cf = self.filter.to_catalog_filter();
        match cat.entries(&cf, self.filter.sort) {
            Ok(next) => {
                self.selection.reconcile(&self.entries, &next);
                self.entries = next;
                self.sync_inspector(true);
            }
            Err(e) => self.banner_error = Some(format!("Error al filtrar imágenes: {e}")),
        }
    }

    fn sync_inspector(&mut self, force_labels: bool) {
        let primary = self.selection.primary().and_then(|i| self.entries.get(i));
        let next_asset = primary.map(|e| e.asset);
        let next_path = primary.map(|e| e.path.clone());

        if force_labels || self.inspector_asset != next_asset {
            self.inspector_asset = next_asset;
            self.inspector_labels = match (next_asset, &self.catalog) {
                (Some(asset), Some(cat)) => cat.labels_of(asset).unwrap_or_default(),
                _ => Vec::new(),
            };
        }

        if self.preview_requested != next_path {
            self.preview_requested = next_path.clone();
            match next_path {
                Some(path) => self.preview_worker.request(path),
                None => {
                    self.preview_worker.clear();
                    self.preview_loaded = None;
                }
            }
        }
    }

    fn poll_workers(&mut self, ctx: &egui::Context) {
        let mut finished_index = false;
        let mut index_completed_ok = false;
        let mut finished_analysis = false;
        let mut needs_reload = false;
        if let Some(worker) = &self.index_worker {
            while let Some(ev) = worker.try_recv() {
                match ev {
                    IndexEvent::Progress {
                        root_index,
                        root_count,
                        done,
                        total,
                        ..
                    } => {
                        self.index_progress = Some(if root_count > 1 {
                            format!(
                                "Indexando carpeta {}/{}: {done} / {total}",
                                root_index + 1,
                                root_count
                            )
                        } else {
                            format!("Indexando {done} / {total}")
                        });
                    }
                    IndexEvent::RootDone { .. } => {
                        needs_reload = true;
                    }
                    IndexEvent::Finished {
                        stopped,
                        new,
                        changed,
                        unchanged,
                        missing,
                        errors,
                    } => {
                        finished_index = true;
                        index_completed_ok = !stopped;
                        needs_reload = true;
                        self.index_progress = None;
                        self.status_notice = Some(if stopped {
                            format!(
                                "Indexación detenida: {}, {}.",
                                count_phrase(new, "nueva", "nuevas"),
                                count_phrase(changed, "actualizada", "actualizadas"),
                            )
                        } else if errors > 0 {
                            format!(
                                "Indexación completa: {}, {}, {unchanged} sin cambios, {}, {}.",
                                count_phrase(new, "nueva", "nuevas"),
                                count_phrase(changed, "actualizada", "actualizadas"),
                                count_phrase(missing, "ausente", "ausentes"),
                                count_phrase(errors, "error", "errores"),
                            )
                        } else {
                            format!(
                                "Indexación completa: {}, {}, {unchanged} sin cambios, {}.",
                                count_phrase(new, "nueva", "nuevas"),
                                count_phrase(changed, "actualizada", "actualizadas"),
                                count_phrase(missing, "ausente", "ausentes"),
                            )
                        });
                    }
                    IndexEvent::Failed(err) => {
                        finished_index = true;
                        self.index_progress = None;
                        self.banner_error = Some(format!("Error durante la indexación: {err}"));
                    }
                }
            }
        }
        if let Some(worker) = &self.analysis_worker {
            while let Some(ev) = worker.try_recv() {
                match ev {
                    AnalysisEvent::Progress { analyzed, pending } => {
                        let total = analyzed.saturating_add(pending);
                        self.analysis_progress = Some(format!("Analizando {analyzed} / {total}"));
                    }
                    AnalysisEvent::Finished {
                        stopped,
                        analyzed,
                        suggestions,
                        errors,
                    } => {
                        finished_analysis = true;
                        needs_reload = true;
                        self.analysis_progress = None;
                        self.status_notice = Some(if stopped {
                            format!(
                                "Análisis detenido: {}, {}.",
                                count_phrase(analyzed, "analizada", "analizadas"),
                                count_phrase(suggestions, "sugerencia", "sugerencias"),
                            )
                        } else if errors > 0 {
                            format!(
                                "Análisis completo: {}, {}, {}.",
                                count_phrase(analyzed, "analizada", "analizadas"),
                                count_phrase(suggestions, "sugerencia", "sugerencias"),
                                count_phrase(errors, "error", "errores"),
                            )
                        } else {
                            format!(
                                "Análisis completo: {}, {}.",
                                count_phrase(analyzed, "analizada", "analizadas"),
                                count_phrase(suggestions, "sugerencia", "sugerencias"),
                            )
                        });
                    }
                    AnalysisEvent::Failed(err) => {
                        finished_analysis = true;
                        self.analysis_progress = None;
                        self.banner_error = Some(format!("Error durante el análisis: {err}"));
                    }
                }
            }
        }
        if finished_index {
            self.index_worker = None;
        }
        if finished_analysis {
            self.analysis_worker = None;
        }
        if needs_reload {
            self.reload_catalog();
        }
        if index_completed_ok && self.settings.auto_analyze {
            self.start_analysis(ctx);
        }

        while let Some(res) = self.thumb_pool.try_recv() {
            match res.image {
                Ok(img) => {
                    if self.thumb_textures.len() >= MAX_THUMB_TEXTURES
                        && let Some(oldest) = self.thumb_order.pop_front()
                    {
                        self.thumb_textures.remove(&oldest);
                    }
                    let tex = ctx.load_texture(
                        format!("thumb_{}", res.sha256),
                        img,
                        TextureOptions::LINEAR,
                    );
                    self.thumb_failed.remove(&res.sha256);
                    self.thumb_textures.insert(res.sha256.clone(), tex);
                    self.thumb_order.push_back(res.sha256);
                }
                Err(_) => {
                    self.thumb_failed.insert(res.sha256);
                }
            }
        }

        while let Some(res) = self.preview_worker.try_recv() {
            if self.preview_requested.as_ref() == Some(&res.path) {
                let loaded = res.image.map(|img| {
                    ctx.load_texture(
                        format!("preview_{}", res.path.display()),
                        img,
                        TextureOptions::LINEAR,
                    )
                });
                self.preview_loaded = Some((res.path, loaded));
            }
        }
    }

    fn start_indexing(&mut self, ctx: &egui::Context) {
        if self.index_worker.is_some() {
            return;
        }
        let roots = self.settings.valid_roots();
        if roots.is_empty() {
            self.banner_error = Some("No hay carpetas válidas para indexar.".into());
            return;
        }
        match settings::db_dest_path(&self.data_dir, &roots) {
            Ok(db) => {
                let ctx_clone = ctx.clone();
                let notify: Notify = Arc::new(move || ctx_clone.request_repaint());
                self.status_notice = None;
                self.index_progress = Some("Preparando escaneo...".into());
                self.index_worker = Some(IndexWorker::start(db, roots, Some(notify)));
            }
            Err(e) => self.banner_error = Some(e.to_string()),
        }
    }

    fn start_analysis(&mut self, ctx: &egui::Context) {
        if self.analysis_worker.is_some() {
            return;
        }
        let roots = self.settings.valid_roots();
        match settings::db_dest_path(&self.data_dir, &roots) {
            Ok(db) => {
                let ctx_clone = ctx.clone();
                let notify: Notify = Arc::new(move || ctx_clone.request_repaint());
                self.analysis_progress = Some("Preparando análisis...".into());
                self.analysis_worker = Some(AnalysisWorker::start(db, Some(notify)));
            }
            Err(e) => self.banner_error = Some(e.to_string()),
        }
    }

    fn pick_and_add_library(&mut self, ctx: &egui::Context) {
        if let Some(folder) = rfd::FileDialog::new()
            .set_title("Elegir carpeta de biblioteca (solo lectura)")
            .pick_folder()
        {
            match self.settings.add_library_root(&folder, &self.data_dir) {
                Ok(_) => {
                    if let Err(e) = settings::save(&self.data_dir, &self.settings) {
                        self.banner_error = Some(e.to_string());
                        return;
                    }
                    self.banner_error = None;
                    if self.catalog.is_none()
                        && let Ok(db) =
                            settings::db_dest_path(&self.data_dir, &self.settings.valid_roots())
                        && let Ok(cat) = Catalog::open(&db)
                    {
                        self.catalog = Some(cat);
                    }
                    self.start_indexing(ctx);
                }
                Err(e) => self.banner_error = Some(e.to_string()),
            }
        }
    }

    fn remove_library(&mut self, path: &Path) {
        if self.settings.remove_library_root(path) {
            if let Err(e) = settings::save(&self.data_dir, &self.settings) {
                self.banner_error = Some(e.to_string());
            } else {
                self.status_notice =
                    Some(format!("Carpeta quitada de la lista: {}", path.display()));
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.active_tab != ActiveTab::Gallery {
            return;
        }
        let wants_text = ctx.egui_wants_keyboard_input();
        let mut selection_changed = false;

        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::COMMAND, Key::F) {
                self.focus_search_next_frame = true;
            }
            if !wants_text && i.consume_key(Modifiers::NONE, Key::Slash) {
                self.focus_search_next_frame = true;
            }
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                if self.viewer_open {
                    self.viewer_open = false;
                } else if !wants_text && !self.selection.is_empty() {
                    self.selection.clear();
                    selection_changed = true;
                }
            }
            if wants_text {
                return;
            }

            if i.consume_key(Modifiers::COMMAND, Key::A) {
                self.selection.select_all(self.entries.len());
                selection_changed = true;
            }

            let extend = i.modifiers.shift;
            let arrow_mod = if extend {
                Modifiers::SHIFT
            } else {
                Modifiers::NONE
            };
            let moved = if i.consume_key(arrow_mod, Key::ArrowLeft) {
                Some(MoveDir::Left)
            } else if i.consume_key(arrow_mod, Key::ArrowRight) {
                Some(MoveDir::Right)
            } else if i.consume_key(arrow_mod, Key::ArrowUp) {
                Some(MoveDir::Up)
            } else if i.consume_key(arrow_mod, Key::ArrowDown) {
                Some(MoveDir::Down)
            } else {
                None
            };
            if let Some(dir) = moved {
                let cols = if self.viewer_open {
                    1
                } else {
                    self.last_columns
                };
                self.selection.move_cursor(
                    dir,
                    cols,
                    self.entries.len(),
                    extend && !self.viewer_open,
                );
                selection_changed = true;
            }

            if (i.consume_key(Modifiers::NONE, Key::Enter)
                || i.consume_key(Modifiers::NONE, Key::Space))
                && self.selection.primary().is_some()
            {
                self.viewer_open = !self.viewer_open;
            }
        });

        if selection_changed {
            self.sync_inspector(false);
        }
    }

    fn draw_top_bar(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        egui::Panel::top("top_bar")
            .exact_size(theme::TOP_BAR_HEIGHT)
            .frame(
                Frame::new()
                    .fill(palette.bg_panel.color32())
                    .inner_margin(Margin::symmetric(
                        theme::SPACE_MD as i8,
                        theme::SPACE_SM as i8,
                    ))
                    .stroke(Stroke::new(1.0, palette.border.color32())),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(
                        RichText::new("photosort")
                            .strong()
                            .size(theme::FONT_HEADING),
                    );
                    ui.separator();

                    let review_label = if self.review_flow.total_items() > 0 {
                        format!("Revisión ({})", self.review_flow.total_items())
                    } else {
                        "Revisión".to_owned()
                    };
                    let dupes_label = if !self.duplicate_groups.is_empty() {
                        format!("Duplicados ({})", self.duplicate_groups.len())
                    } else {
                        "Duplicados".to_owned()
                    };
                    for (tab, label) in [
                        (ActiveTab::Gallery, "Galería"),
                        (ActiveTab::Review, review_label.as_str()),
                        (ActiveTab::Duplicates, dupes_label.as_str()),
                    ] {
                        let selected = self.active_tab == tab;
                        let btn = egui::Button::new(
                            RichText::new(label).color(palette.text_primary.color32()),
                        )
                        .fill(if selected {
                            palette.accent_bg.color32()
                        } else {
                            palette.bg_card.color32()
                        })
                        .stroke(if selected {
                            Stroke::new(theme::FOCUS_STROKE_WIDTH, palette.accent.color32())
                        } else {
                            Stroke::new(1.0, palette.border.color32())
                        });
                        if ui.add(btn).clicked() {
                            self.active_tab = tab;
                            if tab != ActiveTab::Gallery {
                                self.viewer_open = false;
                            }
                        }
                    }
                    ui.separator();

                    let search_id = Id::unique(SEARCH_INPUT_ID);
                    let search_resp = ui.add(
                        egui::TextEdit::singleline(&mut self.filter.search)
                            .id(search_id)
                            .desired_width(240.0)
                            .hint_text("Buscar por ruta o nombre (/ o Ctrl+F)..."),
                    );
                    if self.focus_search_next_frame {
                        search_resp.request_focus();
                        self.focus_search_next_frame = false;
                    }
                    if search_resp.changed() {
                        self.reload_entries();
                    }
                    if !self.filter.search.is_empty() && ui.button("Limpiar").clicked() {
                        self.filter.search.clear();
                        self.reload_entries();
                    }

                    ui.separator();
                    let prev_sort = self.filter.sort;
                    egui::ComboBox::from_id_salt("sort_combo")
                        .selected_text(sort_label(self.filter.sort))
                        .show_ui(ui, |ui| {
                            for option in [
                                Sort::PathAsc,
                                Sort::PathDesc,
                                Sort::SizeAsc,
                                Sort::SizeDesc,
                                Sort::ModifiedAsc,
                                Sort::ModifiedDesc,
                            ] {
                                ui.selectable_value(
                                    &mut self.filter.sort,
                                    option,
                                    sort_label(option),
                                );
                            }
                        });
                    if self.filter.sort != prev_sort {
                        self.reload_entries();
                    }

                    ui.separator();
                    if let Some(worker) = &self.index_worker {
                        if let Some(msg) = &self.index_progress {
                            ui.label(RichText::new(msg).color(palette.text_secondary.color32()));
                        }
                        if ui.button("Cancelar").clicked() {
                            worker.cancel();
                        }
                    } else {
                        let can_index = !self.settings.library_roots.is_empty();
                        if ui
                            .add_enabled(can_index, egui::Button::new("Indexar"))
                            .clicked()
                        {
                            self.start_indexing(ui.ctx());
                        }
                    }

                    let can_export = !self.entries.is_empty() && self.catalog.is_some();
                    if ui
                        .add_enabled(can_export, egui::Button::new("Exportar…"))
                        .clicked()
                        && let Some(cat) = &self.catalog
                    {
                        self.export_wizard = Some(crate::export_flow::ExportWizard::open(
                            cat,
                            &self.entries,
                            &self.selection,
                            self.settings.last_export_dir.clone(),
                        ));
                    }

                    if let Some(worker) = &self.analysis_worker {
                        if let Some(msg) = &self.analysis_progress {
                            ui.label(RichText::new(msg).color(palette.text_secondary.color32()));
                        }
                        if ui.button("Cancelar análisis").clicked() {
                            worker.cancel();
                        }
                    } else {
                        let can_analyze = self.catalog.is_some() && self.counts.locations > 0;
                        if ui
                            .add_enabled(can_analyze, egui::Button::new("Analizar"))
                            .clicked()
                        {
                            self.start_analysis(ui.ctx());
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let prev_theme = self.settings.theme;
                        egui::ComboBox::from_id_salt("theme_combo")
                            .selected_text(format!("Tema: {}", theme_label(self.settings.theme)))
                            .show_ui(ui, |ui| {
                                for option in [Theme::Dark, Theme::Light, Theme::System] {
                                    ui.selectable_value(
                                        &mut self.settings.theme,
                                        option,
                                        theme_label(option),
                                    );
                                }
                            });
                        if self.settings.theme != prev_theme {
                            theme::apply_theme_preference(ui.ctx(), self.settings.theme);
                            if let Err(e) = settings::save(&self.data_dir, &self.settings) {
                                self.banner_error = Some(e.to_string());
                            }
                        }
                    });
                });
            });

        if let Some(err) = self.banner_error.clone() {
            egui::Panel::top("error_banner")
                .frame(
                    Frame::new()
                        .fill(palette.chip_exclude_bg.color32())
                        .inner_margin(Margin::symmetric(
                            theme::SPACE_MD as i8,
                            theme::SPACE_SM as i8,
                        )),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&err).color(palette.chip_exclude_fg.color32()));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Cerrar aviso").clicked() {
                                self.banner_error = None;
                            }
                        });
                    });
                });
        } else if let Some(notice) = self.status_notice.clone() {
            egui::Panel::top("status_banner")
                .frame(
                    Frame::new()
                        .fill(palette.bg_card.color32())
                        .inner_margin(Margin::symmetric(
                            theme::SPACE_MD as i8,
                            theme::SPACE_XS as i8,
                        ))
                        .stroke(Stroke::new(1.0, palette.border.color32())),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&notice).color(palette.text_secondary.color32()));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Ocultar").clicked() {
                                self.status_notice = None;
                            }
                        });
                    });
                });
        }
    }

    fn draw_welcome(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        egui::CentralPanel::default()
            .frame(Frame::new().fill(palette.bg_canvas.color32()))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space((ui.available_height() * 0.28).max(theme::SPACE_XL));
                    Frame::new()
                        .fill(palette.bg_panel.color32())
                        .stroke(Stroke::new(1.0, palette.border.color32()))
                        .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                        .inner_margin(Margin::same(theme::SPACE_XL as i8))
                        .show(ui, |ui| {
                            ui.set_max_width(480.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new("Bienvenido a photosort")
                                        .size(theme::FONT_TITLE)
                                        .strong(),
                                );
                                ui.add_space(theme::SPACE_MD);
                                ui.label(
                                    RichText::new(
                                        "Elige las carpetas de tu biblioteca. photosort solo las lee, nunca las modifica.",
                                    )
                                    .size(theme::FONT_BODY)
                                    .color(palette.text_secondary.color32()),
                                );
                                ui.add_space(theme::SPACE_LG);
                                if ui.button("Añadir carpeta").clicked() {
                                    self.pick_and_add_library(ui.ctx());
                                }
                            });
                        });
                });
            });
    }

    fn draw_sidebar(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let mut to_remove: Option<PathBuf> = None;
        let mut add_clicked = false;
        let mut filter_changed = false;

        egui::Panel::left("sidebar")
            .default_size(theme::SIDEBAR_WIDTH)
            .size_range(210.0..=380.0)
            .resizable(true)
            .frame(
                Frame::new()
                    .fill(palette.bg_panel.color32())
                    .inner_margin(Margin::same(theme::SPACE_MD as i8))
                    .stroke(Stroke::new(1.0, palette.border.color32())),
            )
            .show(ui, |ui| {
                ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Bibliotecas").strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Añadir carpeta").clicked() {
                                add_clicked = true;
                            }
                        });
                    });
                    ui.add_space(theme::SPACE_XS);
                    for root in &self.settings.library_roots {
                        let name = root
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| root.display().to_string());
                        ui.horizontal(|ui| {
                            ui.label(&name).on_hover_text(root.display().to_string());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Quitar").clicked() {
                                    to_remove = Some(root.clone());
                                }
                            });
                        });
                    }

                    ui.add_space(theme::SPACE_MD);
                    ui.separator();
                    ui.add_space(theme::SPACE_SM);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Etiquetas").strong());
                        if self.filter.has_active_filters() {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Limpiar filtros").clicked() {
                                    self.filter.clear_filters();
                                    filter_changed = true;
                                }
                            });
                        }
                    });
                    ui.label(
                        RichText::new(CHIP_FILTER_HELP)
                            .size(theme::FONT_SMALL)
                            .color(palette.text_secondary.color32()),
                    );
                    ui.add_space(theme::SPACE_XS);

                    if self.labels_in_use.is_empty() {
                        ui.label(
                            RichText::new("Sin etiquetas todavía.")
                                .color(palette.text_secondary.color32()),
                        );
                    } else {
                        for (kind, heading) in [
                            (LabelKind::Category, "Categorías"),
                            (LabelKind::Tag, "Etiquetas"),
                            (LabelKind::Entity, "Entidades"),
                        ] {
                            let group: Vec<&LabelCount> = self
                                .labels_in_use
                                .iter()
                                .filter(|lc| lc.kind == kind)
                                .collect();
                            if group.is_empty() {
                                continue;
                            }
                            ui.add_space(theme::SPACE_XS);
                            ui.label(
                                RichText::new(heading)
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                            ui.horizontal_wrapped(|ui| {
                                for lc in group {
                                    let mode = self.filter.chip_mode(lc.label);
                                    let count = lc.images.max(0) as usize;
                                    let (bg, fg, stroke) = match mode {
                                        None => (
                                            palette.bg_card.color32(),
                                            palette.text_primary.color32(),
                                            Stroke::new(1.0, palette.border.color32()),
                                        ),
                                        Some(ChipMode::Require) => {
                                            let c = palette.chip_require_fg.color32();
                                            (
                                                palette.chip_require_bg.color32(),
                                                c,
                                                Stroke::new(1.5, c),
                                            )
                                        }
                                        Some(ChipMode::Any) => {
                                            let c = palette.chip_any_fg.color32();
                                            (palette.chip_any_bg.color32(), c, Stroke::new(1.5, c))
                                        }
                                        Some(ChipMode::Exclude) => {
                                            let c = palette.chip_exclude_fg.color32();
                                            (
                                                palette.chip_exclude_bg.color32(),
                                                c,
                                                Stroke::new(1.5, c),
                                            )
                                        }
                                    };
                                    let text = format_chip_label(mode, &lc.name, count);
                                    let mut rt = RichText::new(text).color(fg);
                                    if mode.is_some() {
                                        rt = rt.strong();
                                    }
                                    let btn = egui::Button::new(rt).fill(bg).stroke(stroke);
                                    let resp = ui
                                        .add(btn)
                                        .on_hover_text(format_chip_tooltip(mode, &lc.name, count));
                                    if resp.clicked() {
                                        self.filter.cycle_chip(lc.label);
                                        filter_changed = true;
                                    }
                                }
                            });
                        }
                    }

                    ui.add_space(theme::SPACE_MD);
                    ui.separator();
                    ui.add_space(theme::SPACE_SM);
                    ui.label(RichText::new("Resumen").strong());
                    ui.add_space(theme::SPACE_XS);
                    let assets = self.counts.assets.max(0) as usize;
                    let locations = self.counts.locations.max(0) as usize;
                    let missing = self.counts.missing.max(0) as usize;
                    let selected = self.selection.len();
                    let stats = [
                        (
                            plural_word(assets, "Imagen única", "Imágenes únicas"),
                            assets.to_string(),
                        ),
                        (
                            plural_word(locations, "Ubicación", "Ubicaciones"),
                            locations.to_string(),
                        ),
                        (
                            plural_word(missing, "Ausente", "Ausentes"),
                            missing.to_string(),
                        ),
                        ("En vista", self.entries.len().to_string()),
                        (
                            plural_word(selected, "Seleccionada", "Seleccionadas"),
                            selected.to_string(),
                        ),
                    ];
                    for (label, value) in stats {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(label).color(palette.text_secondary.color32()));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(RichText::new(value).strong());
                            });
                        });
                    }
                });
            });

        if add_clicked {
            self.pick_and_add_library(ui.ctx());
        }
        if let Some(p) = to_remove {
            self.remove_library(&p);
        }
        if filter_changed {
            self.reload_entries();
        }
    }

    fn draw_gallery(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let mut selection_changed = false;
        let mut visible_hashes: HashSet<String> = HashSet::new();
        let mut to_request: Vec<ThumbRequest> = Vec::new();

        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(palette.bg_canvas.color32())
                    .inner_margin(Margin::same(theme::SPACE_MD as i8)),
            )
            .show(ui, |ui| {
                // Selection bar when items exist
                if !self.entries.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} ({})",
                                count_phrase(self.entries.len(), "imagen", "imágenes"),
                                selected_phrase(self.selection.len()),
                            ))
                            .color(palette.text_secondary.color32()),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if !self.selection.is_empty()
                                && ui.button("Limpiar selección").clicked()
                            {
                                self.selection.clear();
                                selection_changed = true;
                            }
                            if ui.button("Invertir").clicked() {
                                self.selection.invert(self.entries.len());
                                selection_changed = true;
                            }
                            if ui.button("Seleccionar todo").clicked() {
                                self.selection.select_all(self.entries.len());
                                selection_changed = true;
                            }
                        });
                    });
                    ui.add_space(theme::SPACE_SM);
                }

                if self.entries.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(theme::SPACE_XL * 2.0);
                        let msg = if self.counts.locations == 0 {
                            "Aún no hay imágenes indexadas. Pulsa \"Indexar\" para escanear tus carpetas."
                        } else {
                            "Ninguna imagen coincide con los filtros actuales."
                        };
                        ui.label(RichText::new(msg).color(palette.text_secondary.color32()));
                    });
                    return;
                }

                let avail_w = ui.available_width();
                let layout = GridLayout::compute(
                    avail_w,
                    theme::CELL_WIDTH,
                    theme::CELL_HEIGHT,
                    theme::CELL_SPACING,
                    self.entries.len(),
                );
                self.last_columns = layout.columns;

                ui.spacing_mut().item_spacing = Vec2::splat(theme::CELL_SPACING);
                ScrollArea::vertical().auto_shrink([false, false]).show_rows(
                    ui,
                    theme::CELL_HEIGHT,
                    layout.row_count,
                    |ui, row_range| {
                        for row in row_range {
                            let indices = layout.row_indices(row);
                            ui.horizontal(|ui| {
                                for idx in indices {
                                    let entry = &self.entries[idx];
                                    visible_hashes.insert(entry.sha256.clone());
                                    if !self.thumb_textures.contains_key(&entry.sha256)
                                        && !self.thumb_failed.contains(&entry.sha256)
                                    {
                                        to_request.push(ThumbRequest {
                                            source: entry.path.clone(),
                                            sha256: entry.sha256.clone(),
                                        });
                                    }

                                    let selected = self.selection.contains(idx);
                                    let is_primary = self.selection.primary() == Some(idx);
                                    let pending_review =
                                        self.pending_review_assets.contains(&entry.asset);

                                    let (rect, resp) = ui.allocate_exact_size(
                                        Vec2::new(theme::CELL_WIDTH, theme::CELL_HEIGHT),
                                        Sense::click(),
                                    );
                                    if resp.clicked() {
                                        let mods = ui.input(|i| i.modifiers);
                                        if mods.shift {
                                            self.selection.shift_click(idx, self.entries.len());
                                        } else if mods.command {
                                            self.selection.ctrl_click(idx, self.entries.len());
                                        } else {
                                            self.selection.click(idx, self.entries.len());
                                        }
                                        selection_changed = true;
                                    }
                                    if resp.double_clicked() {
                                        self.selection.click(idx, self.entries.len());
                                        self.viewer_open = true;
                                        selection_changed = true;
                                    }

                                    let bg = if selected {
                                        palette.accent_bg.color32()
                                    } else if resp.hovered() {
                                        palette.bg_elevated.color32()
                                    } else {
                                        palette.bg_card.color32()
                                    };
                                    let border = if is_primary || resp.has_focus() {
                                        Stroke::new(
                                            theme::FOCUS_STROKE_WIDTH,
                                            palette.accent.color32(),
                                        )
                                    } else if selected {
                                        Stroke::new(1.5, palette.accent.color32())
                                    } else {
                                        Stroke::new(1.0, palette.border.color32())
                                    };
                                    let painter = ui.painter();
                                    painter.rect(
                                        rect,
                                        CornerRadius::same(theme::RADIUS_MD),
                                        bg,
                                        border,
                                        egui::StrokeKind::Inside,
                                    );

                                    let thumb_rect = egui::Rect::from_min_size(
                                        rect.min + Vec2::splat(theme::SPACE_SM),
                                        Vec2::new(
                                            theme::CELL_WIDTH - theme::SPACE_SM * 2.0,
                                            theme::CELL_THUMB_HEIGHT,
                                        ),
                                    );
                                    painter.rect_filled(
                                        thumb_rect,
                                        CornerRadius::same(theme::RADIUS_SM),
                                        palette.bg_canvas.color32(),
                                    );

                                    if let Some(tex) = self.thumb_textures.get(&entry.sha256) {
                                        let fit = fit_rect(thumb_rect, tex.size_vec2());
                                        painter.image(
                                            tex.id(),
                                            fit,
                                            egui::Rect::from_min_max(
                                                egui::pos2(0.0, 0.0),
                                                egui::pos2(1.0, 1.0),
                                            ),
                                            Color32::WHITE,
                                        );
                                    } else {
                                        let placeholder = if self.thumb_failed.contains(&entry.sha256)
                                        {
                                            "Sin miniatura"
                                        } else {
                                            "Cargando..."
                                        };
                                        painter.text(
                                            thumb_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            placeholder,
                                            egui::FontId::proportional(theme::FONT_SMALL),
                                            palette.text_secondary.color32(),
                                        );
                                    }

                                    if pending_review {
                                        let badge_rect = egui::Rect::from_min_size(
                                            thumb_rect.min + Vec2::splat(theme::SPACE_XS),
                                            Vec2::new(66.0, 20.0),
                                        );
                                        painter.rect_filled(
                                            badge_rect,
                                            CornerRadius::same(theme::RADIUS_SM),
                                            palette.review_badge_bg.color32(),
                                        );
                                        painter.text(
                                            badge_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "Revisar",
                                            egui::FontId::proportional(theme::FONT_SMALL),
                                            palette.review_badge_fg.color32(),
                                        );
                                    }

                                    let filename = entry
                                        .path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| entry.path.display().to_string());
                                    let label_rect = egui::Rect::from_min_max(
                                        egui::pos2(
                                            rect.min.x + theme::SPACE_SM,
                                            thumb_rect.max.y + theme::SPACE_XS,
                                        ),
                                        egui::pos2(
                                            rect.max.x - theme::SPACE_SM,
                                            rect.max.y - theme::SPACE_XS,
                                        ),
                                    );
                                    let mut child_ui = ui.new_child(
                                        egui::UiBuilder::new().max_rect(label_rect),
                                    );
                                    child_ui.add(
                                        egui::Label::new(
                                            RichText::new(filename)
                                                .size(theme::FONT_SMALL)
                                                .color(palette.text_primary.color32()),
                                        )
                                        .truncate(),
                                    );
                                }
                            });
                        }
                    },
                );
            });

        self.last_thumbs_ready = !visible_hashes.is_empty() && to_request.is_empty();
        self.thumb_pool.set_visible(&visible_hashes);
        for req in to_request {
            self.thumb_pool.request(req);
        }
        if selection_changed {
            self.sync_inspector(false);
        }
    }

    fn draw_inspector(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        let selected_entry = self
            .selection
            .primary()
            .and_then(|i| self.entries.get(i))
            .cloned();

        let mut accept_action: Option<(AssetId, catalog::LabelId)> = None;
        let mut reject_action: Option<(AssetId, catalog::LabelId)> = None;
        let mut add_user_label: Option<(AssetId, LabelKind, String)> = None;

        egui::Panel::right("inspector")
            .default_size(theme::INSPECTOR_WIDTH)
            .size_range(280.0..=520.0)
            .resizable(true)
            .frame(
                Frame::new()
                    .fill(palette.bg_panel.color32())
                    .inner_margin(Margin::same(theme::SPACE_MD as i8))
                    .stroke(Stroke::new(1.0, palette.border.color32())),
            )
            .show(ui, |ui| {
                let Some(entry) = selected_entry else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(theme::SPACE_XL * 2.0);
                        ui.label(
                            RichText::new("Selecciona una imagen para inspeccionarla.")
                                .color(palette.text_secondary.color32()),
                        );
                    });
                    return;
                };

                ScrollArea::vertical().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Inspector").strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Abrir visor").clicked() {
                                self.viewer_open = true;
                            }
                        });
                    });
                    ui.add_space(theme::SPACE_SM);

                    // Preview box
                    let preview_w = ui.available_width().max(120.0);
                    let preview_h = 220.0;
                    let (rect, _) =
                        ui.allocate_exact_size(Vec2::new(preview_w, preview_h), Sense::hover());
                    ui.painter().rect(
                        rect,
                        CornerRadius::same(theme::RADIUS_MD),
                        palette.bg_canvas.color32(),
                        Stroke::new(1.0, palette.border.color32()),
                        egui::StrokeKind::Inside,
                    );
                    match &self.preview_loaded {
                        Some((p, Ok(tex))) if p == &entry.path => {
                            let fit = fit_rect(rect.shrink(theme::SPACE_XS), tex.size_vec2());
                            ui.painter().image(
                                tex.id(),
                                fit,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                Color32::WHITE,
                            );
                        }
                        Some((p, Err(e))) if p == &entry.path => {
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                format!("No se pudo decodificar: {e}"),
                                egui::FontId::proportional(theme::FONT_SMALL),
                                palette.text_secondary.color32(),
                            );
                        }
                        _ => {
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "Cargando vista previa...",
                                egui::FontId::proportional(theme::FONT_SMALL),
                                palette.text_secondary.color32(),
                            );
                        }
                    }

                    ui.add_space(theme::SPACE_MD);
                    ui.label(RichText::new("Metadatos").strong());
                    ui.add_space(theme::SPACE_XS);
                    meta_row(ui, palette, "Ruta", &entry.path.display().to_string());
                    meta_row(ui, palette, "Tamaño", &format_bytes(entry.size));
                    let date_str = entry
                        .modified_ns
                        .map(format_timestamp_utc)
                        .unwrap_or_else(|| "Desconocida".into());
                    meta_row(ui, palette, "Modificado", &date_str);
                    let short_sha = if entry.sha256.len() > 16 {
                        format!("{}...", &entry.sha256[..16])
                    } else {
                        entry.sha256.clone()
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("SHA-256:").color(palette.text_secondary.color32()));
                        ui.label(RichText::new(&short_sha).monospace())
                            .on_hover_text(&entry.sha256);
                    });

                    ui.add_space(theme::SPACE_MD);
                    ui.separator();
                    ui.add_space(theme::SPACE_SM);

                    ui.label(RichText::new("Etiquetas de la imagen").strong());
                    ui.add_space(theme::SPACE_XS);
                    if self.inspector_labels.is_empty() {
                        ui.label(
                            RichText::new("Sin etiquetas asignadas.")
                                .color(palette.text_secondary.color32()),
                        );
                    } else {
                        for assign in &self.inspector_labels {
                            Frame::new()
                                .fill(palette.bg_card.color32())
                                .stroke(Stroke::new(1.0, palette.border.color32()))
                                .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                                .inner_margin(Margin::same(theme::SPACE_SM as i8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!(
                                                "{}: {}",
                                                kind_label(assign.kind),
                                                assign.name
                                            ))
                                            .strong(),
                                        );
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                let (st_text, st_color) = match assign.status {
                                                    Status::Accepted => (
                                                        "Aceptada",
                                                        palette.status_accepted_fg.color32(),
                                                    ),
                                                    Status::Rejected => (
                                                        "Rechazada",
                                                        palette.status_rejected_fg.color32(),
                                                    ),
                                                    Status::Suggested => (
                                                        "Sugerida",
                                                        palette.status_suggested_fg.color32(),
                                                    ),
                                                };
                                                ui.label(
                                                    RichText::new(st_text)
                                                        .size(theme::FONT_SMALL)
                                                        .color(st_color),
                                                );
                                            },
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(format!(
                                                "Fuente: {} ({:.0}%)",
                                                source_label(assign.source),
                                                assign.confidence * 100.0
                                            ))
                                            .size(theme::FONT_SMALL)
                                            .color(palette.text_secondary.color32()),
                                        );
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if assign.status != Status::Rejected
                                                    && ui.button("Rechazar").clicked()
                                                {
                                                    reject_action =
                                                        Some((entry.asset, assign.label));
                                                }
                                                if assign.status != Status::Accepted
                                                    && ui.button("Aceptar").clicked()
                                                {
                                                    accept_action =
                                                        Some((entry.asset, assign.label));
                                                }
                                            },
                                        );
                                    });
                                });
                            ui.add_space(theme::SPACE_XS);
                        }
                    }

                    ui.add_space(theme::SPACE_SM);
                    ui.label(RichText::new("Añadir etiqueta").strong());
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("new_label_kind")
                            .selected_text(kind_label(self.new_label_kind))
                            .width(96.0)
                            .show_ui(ui, |ui| {
                                for k in [LabelKind::Tag, LabelKind::Category, LabelKind::Entity] {
                                    ui.selectable_value(&mut self.new_label_kind, k, kind_label(k));
                                }
                            });
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut self.new_label_text)
                                .desired_width((ui.available_width() - 78.0).max(80.0))
                                .hint_text("Nombre..."),
                        );
                        let submit = ui.button("Añadir").clicked()
                            || (edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                        if submit {
                            let name = self.new_label_text.trim().to_owned();
                            if !name.is_empty() {
                                add_user_label = Some((entry.asset, self.new_label_kind, name));
                                self.new_label_text.clear();
                            }
                        }
                    });
                });
            });

        let mut catalog_mutated = false;
        if let Some(cat) = &self.catalog {
            if let Some((asset, label)) = accept_action {
                match cat.accept(asset, label) {
                    Ok(()) => catalog_mutated = true,
                    Err(e) => self.banner_error = Some(format!("No se pudo aceptar: {e}")),
                }
            }
            if let Some((asset, label)) = reject_action {
                match cat.reject(asset, label) {
                    Ok(()) => catalog_mutated = true,
                    Err(e) => self.banner_error = Some(format!("No se pudo rechazar: {e}")),
                }
            }
            if let Some((asset, kind, name)) = add_user_label {
                match cat.label(kind, &name).and_then(|id| cat.accept(asset, id)) {
                    Ok(()) => catalog_mutated = true,
                    Err(e) => self.banner_error = Some(format!("No se pudo añadir etiqueta: {e}")),
                }
            }
        }
        if catalog_mutated {
            self.reload_catalog();
        }
    }

    fn draw_viewer_overlay(&mut self, ctx: &egui::Context, palette: &Palette) {
        if !self.viewer_open {
            return;
        }
        let Some(idx) = self.selection.primary() else {
            self.viewer_open = false;
            return;
        };
        let Some(entry) = self.entries.get(idx).cloned() else {
            self.viewer_open = false;
            return;
        };

        let screen = ctx.content_rect();
        egui::Area::new(Id::unique("image_viewer_overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(12, 14, 17, 240))
                    .inner_margin(Margin::same(theme::SPACE_LG as i8))
                    .show(ui, |ui| {
                        ui.set_min_size(screen.size() - Vec2::splat(theme::SPACE_LG * 2.0));
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} / {} — {}",
                                    idx + 1,
                                    self.entries.len(),
                                    entry.path.display()
                                ))
                                .color(theme::DARK_PALETTE.text_primary.color32()),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Cerrar").clicked() {
                                    self.viewer_open = false;
                                }
                                if ui.button("Siguiente").clicked() {
                                    self.selection.move_cursor(
                                        MoveDir::Right,
                                        1,
                                        self.entries.len(),
                                        false,
                                    );
                                    self.sync_inspector(false);
                                }
                                if ui.button("Anterior").clicked() {
                                    self.selection.move_cursor(
                                        MoveDir::Left,
                                        1,
                                        self.entries.len(),
                                        false,
                                    );
                                    self.sync_inspector(false);
                                }
                            });
                        });
                        ui.add_space(theme::SPACE_MD);

                        let avail = ui.available_size();
                        let (rect, _) = ui.allocate_exact_size(avail, Sense::hover());
                        match &self.preview_loaded {
                            Some((p, Ok(tex))) if p == &entry.path => {
                                let fit = fit_rect(rect, tex.size_vec2());
                                ui.painter().image(
                                    tex.id(),
                                    fit,
                                    egui::Rect::from_min_max(
                                        egui::pos2(0.0, 0.0),
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    Color32::WHITE,
                                );
                            }
                            Some((p, Err(e))) if p == &entry.path => {
                                ui.painter().text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    format!("Error al cargar imagen: {e}"),
                                    egui::FontId::proportional(theme::FONT_BODY),
                                    palette.text_secondary.color32(),
                                );
                            }
                            _ => {
                                ui.painter().text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "Cargando imagen...",
                                    egui::FontId::proportional(theme::FONT_BODY),
                                    theme::DARK_PALETTE.text_secondary.color32(),
                                );
                            }
                        }
                    });
            });
    }

    fn step_screenshot(&mut self, ctx: &egui::Context) {
        let has_entries = !self.entries.is_empty();
        let has_selection = !self.selection.is_empty();
        let thumbs_ready = self.last_thumbs_ready;
        let preview_ready = match &self.preview_requested {
            None => true,
            Some(path) => matches!(&self.preview_loaded, Some((p, _)) if p == path),
        };
        let select_first = self.screenshot.as_mut().is_some_and(|ctrl| {
            ctrl.step(ctx, has_entries, has_selection, thumbs_ready, preview_ready)
        });
        if select_first && has_entries {
            self.selection.click(0, self.entries.len());
            self.sync_inspector(false);
        }
    }
}

impl eframe::App for PhotosortApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_workers(&ctx);
        self.handle_shortcuts(&ctx);

        let palette = theme::palette_for(&ctx);
        self.draw_top_bar(ui, palette);

        if self.settings.library_roots.is_empty() {
            self.draw_welcome(ui, palette);
        } else {
            match self.active_tab {
                ActiveTab::Gallery => {
                    self.draw_sidebar(ui, palette);
                    self.draw_inspector(ui, palette);
                    self.draw_gallery(ui, palette);
                    self.draw_viewer_overlay(&ctx, palette);
                }
                ActiveTab::Review => {
                    let outcome = views::review::draw(
                        ui,
                        palette,
                        &mut self.review_flow,
                        self.catalog.as_ref(),
                        &self.thumb_pool,
                        &self.thumb_textures,
                        &self.thumb_failed,
                    );
                    if let Some(err) = outcome.error {
                        self.banner_error = Some(err);
                    }
                    if let Some(notice) = outcome.status_notice {
                        self.status_notice = Some(notice);
                    }
                    if outcome.catalog_mutated {
                        self.reload_catalog();
                    }
                }
                ActiveTab::Duplicates => {
                    if let Some(err) = views::duplicates::draw(
                        ui,
                        palette,
                        &self.duplicate_groups,
                        &self.thumb_pool,
                        &self.thumb_textures,
                        &self.thumb_failed,
                    ) {
                        self.banner_error = Some(err);
                    }
                }
            }
        }
        crate::views::export::draw_window(
            &ctx,
            palette,
            &mut self.export_wizard,
            &self.data_dir,
            &mut self.settings,
            &mut self.banner_error,
        );

        self.step_screenshot(&ctx);
    }
}

fn fit_rect(bounds: egui::Rect, content: Vec2) -> egui::Rect {
    if content.x <= 0.0 || content.y <= 0.0 || bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return bounds;
    }
    let scale = (bounds.width() / content.x)
        .min(bounds.height() / content.y)
        .min(1.0);
    let size = content * scale;
    egui::Rect::from_center_size(bounds.center(), size)
}

fn meta_row(ui: &mut egui::Ui, palette: &Palette, key: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{key}:")).color(palette.text_secondary.color32()));
        ui.label(value);
    });
}

fn sort_label(sort: Sort) -> &'static str {
    match sort {
        Sort::PathAsc => "Ruta (A-Z)",
        Sort::PathDesc => "Ruta (Z-A)",
        Sort::SizeAsc => "Tamaño (menor)",
        Sort::SizeDesc => "Tamaño (mayor)",
        Sort::ModifiedAsc => "Fecha (antigua)",
        Sort::ModifiedDesc => "Fecha (reciente)",
    }
}

fn theme_label(theme: Theme) -> &'static str {
    match theme {
        Theme::Dark => "Oscuro",
        Theme::Light => "Claro",
        Theme::System => "Sistema",
    }
}

fn kind_label(kind: LabelKind) -> &'static str {
    match kind {
        LabelKind::Category => "Categoría",
        LabelKind::Tag => "Etiqueta",
        LabelKind::Entity => "Entidad",
    }
}

fn source_label(source: catalog::Source) -> &'static str {
    match source {
        catalog::Source::Ai => "IA",
        catalog::Source::Ocr => "OCR",
        catalog::Source::Rules => "Reglas",
        catalog::Source::User => "Usuario",
    }
}
