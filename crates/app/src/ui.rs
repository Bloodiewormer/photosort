use crate::model::{ChipMode, FilterState, MoveDir, Selection, count_phrase};
use crate::review_engine::{
    EnginePhase, EngineSnapshot, ReviewEngine, ReviewEngineEvent, StageKind, chunk_reload_due,
};
use crate::review_flow::ReviewFlow;
use crate::screenshot::CaptureView;
use crate::settings::{self, Settings, Theme};
use crate::theme;
use crate::views;
use crate::views::context_menu::{PhotoMenuAction, PhotoMenuContext, PhotoMenuState};
use crate::views::gallery::SimilarityView;
use crate::views::inspector::InspectorState;
use crate::views::settings::SettingsViewState;
use crate::views::shell::{ActiveView, SidebarState};
use crate::views::tags::{TagsApplyState, TagsViewState};
use crate::views::works::{WorksRunState, WorksViewState};
use crate::vision_worker::{TagsApplyEvent, TagsApplyWorker, VisionRankResult, VisionRankWorker};
use crate::worker::{Notify, PreviewWorker, ThumbPool, ThumbnailProvider};
use crate::works_worker::{WorksEvent, WorksWorker};
use catalog::{
    AssetId, Catalog, Counts, DuplicateGroup, Entry, LabelCount, ReviewedCounts, ReviewedFilter,
    SCREENSHOT_CATEGORY, Status, VisionRule, WorkSummary,
};
use eframe::egui::{self, Key, Modifiers, Pos2, TextureHandle, TextureOptions};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_THUMB_TEXTURES: usize = 1024;
const ENGINE_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

pub struct PhotosortApp {
    data_dir: PathBuf,
    demo_mode: bool,
    force_welcome: bool,
    settings: Settings,
    catalog: Option<Catalog>,
    active_view: ActiveView,
    logo_texture: Option<TextureHandle>,

    entries: Vec<Entry>,
    counts: Counts,
    reviewed_counts: ReviewedCounts,
    reviewed_assets: HashSet<AssetId>,
    root_counts: HashMap<PathBuf, usize>,
    labels_in_use: Vec<LabelCount>,
    works: Vec<WorkSummary>,
    works_view: WorksViewState,
    vision_rules: Vec<VisionRule>,
    vision_analyzed_count: usize,
    tags_view: TagsViewState,
    similarity_view: Option<SimilarityView>,
    pending_suggestions_assets: HashSet<AssetId>,
    review_flow: ReviewFlow,
    duplicate_groups: Vec<DuplicateGroup>,

    filter: FilterState,
    sidebar_state: SidebarState,
    settings_state: SettingsViewState,
    selection: Selection,
    last_columns: usize,
    last_primary_cell_pos: Option<Pos2>,
    viewer_open: bool,
    photo_menu: Option<PhotoMenuState>,
    inspector: InspectorState,

    review_engine: Option<ReviewEngine>,
    engine_snapshot: EngineSnapshot,
    demo_paused: bool,
    last_engine_refresh: Instant,
    pending_engine_refresh: bool,
    /// A chunk finished and the full reload it calls for has not happened yet.
    chunk_reload_pending: bool,
    last_full_reload: Instant,

    works_worker: Option<WorksWorker>,
    tags_apply_worker: Option<TagsApplyWorker>,
    vision_rank_worker: Option<VisionRankWorker>,
    status_notice: Option<String>,
    banner_error: Option<String>,

    thumb_pool: ThumbPool,
    thumb_textures: HashMap<String, TextureHandle>,
    thumb_order: VecDeque<String>,
    thumb_failed: HashSet<String>,

    preview_worker: PreviewWorker,
    preview_requested: Option<PathBuf>,
    preview_loaded: Option<(PathBuf, Result<TextureHandle, String>)>,

    focus_search_next_frame: bool,
    export_wizard: Option<crate::export_flow::ExportWizard>,
    preset_export_dir: Option<PathBuf>,
    export_open_failed: bool,
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
        Self::new_with_mode(cc, data_dir, false, open_thumbnails)
    }

    pub fn new_with_mode(
        cc: &eframe::CreationContext<'_>,
        data_dir: PathBuf,
        demo_mode: bool,
        open_thumbnails: impl FnOnce(
            &Path,
            &[fsguard::LibraryRoot],
        ) -> (Arc<dyn ThumbnailProvider>, Option<String>),
    ) -> Self {
        let loaded = settings::load(&data_dir);
        let settings = loaded.settings;
        let (thumbs, thumbs_warning) = open_thumbnails(&data_dir, &settings.valid_roots());
        theme::configure_styles(&cc.egui_ctx, settings.theme);

        let logo_texture = theme::logo_color_image().map(|img| {
            cc.egui_ctx
                .load_texture("photosort_logo", img, TextureOptions::LINEAR)
        });

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

        // In --demo mode, never start the real background review engine or load ONNX models.
        let review_engine = if demo_mode {
            None
        } else {
            settings::db_dest_path(&data_dir, &settings.valid_roots())
                .ok()
                .map(|db| {
                    ReviewEngine::start(db, settings.to_review_config(), Some(Arc::clone(&notify)))
                })
        };

        let initial_snapshot = if demo_mode {
            EngineSnapshot {
                phase: EnginePhase::Running {
                    done: 48,
                    total: 160,
                    stage: StageKind::Rules,
                    elapsed_secs: 9.6,
                },
                ..Default::default()
            }
        } else {
            EngineSnapshot::default()
        };

        let mut app = Self {
            data_dir,
            demo_mode,
            force_welcome: false,
            settings,
            catalog,
            active_view: ActiveView::Gallery,
            logo_texture,
            entries: Vec::new(),
            counts: Counts {
                assets: 0,
                locations: 0,
                missing: 0,
            },
            reviewed_counts: ReviewedCounts::default(),
            reviewed_assets: HashSet::new(),
            root_counts: HashMap::new(),
            labels_in_use: Vec::new(),
            works: Vec::new(),
            works_view: WorksViewState::default(),
            vision_rules: Vec::new(),
            vision_analyzed_count: 0,
            tags_view: TagsViewState::default(),
            similarity_view: None,
            pending_suggestions_assets: HashSet::new(),
            review_flow: ReviewFlow::new(),
            duplicate_groups: Vec::new(),
            filter: FilterState::default(),
            sidebar_state: SidebarState::default(),
            settings_state: SettingsViewState::default(),
            selection: Selection::default(),
            last_columns: 1,
            last_primary_cell_pos: None,
            viewer_open: false,
            photo_menu: None,
            inspector: InspectorState::default(),
            review_engine,
            engine_snapshot: initial_snapshot,
            demo_paused: false,
            last_engine_refresh: Instant::now(),
            pending_engine_refresh: false,
            chunk_reload_pending: false,
            last_full_reload: Instant::now(),
            works_worker: None,
            tags_apply_worker: None,
            vision_rank_worker: None,
            status_notice: None,
            banner_error,
            thumb_pool: ThumbPool::new(3, thumbs, Some(Arc::clone(&notify))),
            thumb_textures: HashMap::new(),
            thumb_order: VecDeque::new(),
            thumb_failed: HashSet::new(),
            preview_worker: PreviewWorker::new(theme::PREVIEW_MAX_EDGE, Some(notify)),
            preview_requested: None,
            preview_loaded: None,
            focus_search_next_frame: false,
            export_wizard: None,
            preset_export_dir: None,
            export_open_failed: false,
            screenshot: None,
            last_thumbs_ready: false,
        };
        app.reload_catalog();
        if app.demo_mode {
            app.engine_snapshot.phase = EnginePhase::Running {
                done: app.reviewed_counts.reviewed,
                total: app.reviewed_counts.all.max(1),
                stage: StageKind::Rules,
                elapsed_secs: 9.6,
            };
        }
        app
    }

    pub fn with_theme_override(mut self, ctx: &egui::Context, theme: Theme) -> Self {
        self.settings.theme = theme;
        theme::apply_theme_preference(ctx, theme);
        self
    }

    pub fn with_initial_view(
        mut self,
        view: CaptureView,
        preset_export_dir: Option<PathBuf>,
    ) -> Self {
        match view {
            CaptureView::Gallery | CaptureView::Export | CaptureView::Menu => {
                self.active_view = ActiveView::Gallery;
            }
            CaptureView::Welcome => {
                self.force_welcome = true;
                self.active_view = ActiveView::Gallery;
            }
            CaptureView::Settings => {
                self.active_view = ActiveView::Settings;
            }
            CaptureView::Suggestions => {
                self.active_view = ActiveView::Suggestions;
            }
            CaptureView::Duplicates => {
                self.active_view = ActiveView::Duplicates;
            }
            CaptureView::Works => {
                self.active_view = ActiveView::Works;
            }
            CaptureView::Tags => {
                self.active_view = ActiveView::Tags;
            }
        }
        self.preset_export_dir = preset_export_dir;
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
        let roots = self.settings.library_roots.clone();
        let stages = self.settings.enabled_stage_specs();

        match cat.counts_in_roots(&roots) {
            Ok(c) => self.counts = c,
            Err(e) => self.banner_error = Some(format!("Error al consultar contadores: {e}")),
        }
        match cat.reviewed_counts(&stages, Some(&roots)) {
            Ok(rc) => self.reviewed_counts = rc,
            Err(e) => self.banner_error = Some(format!("Error al contar revisadas: {e}")),
        }
        match cat.reviewed_assets(&stages, Some(&roots)) {
            Ok(set) => self.reviewed_assets = set,
            Err(e) => self.banner_error = Some(format!("Error al consultar revisadas: {e}")),
        }
        self.root_counts.clear();
        for root in &roots {
            if let Ok(n) = cat.count_images_in_root(root) {
                self.root_counts.insert(root.clone(), n);
            }
        }
        match cat.labels_in_use_in_roots(&roots) {
            Ok(list) => self.labels_in_use = list,
            Err(e) => self.banner_error = Some(format!("Error al cargar etiquetas: {e}")),
        }
        match cat.list_works_in_roots(&roots) {
            Ok(list) => {
                self.works = list;
                self.works_view.sync_selection(&self.works, Some(cat));
                if self
                    .inspector
                    .alias_target
                    .is_none_or(|id| !self.works.iter().any(|w| w.label == id))
                {
                    self.inspector.alias_target = self.works.first().map(|w| w.label);
                }
            }
            Err(e) => self.banner_error = Some(format!("Error al cargar obras: {e}")),
        }
        if let Ok(queue) = cat.review_queue_in_roots(1.1, &roots) {
            self.pending_suggestions_assets = queue.into_iter().map(|i| i.asset).collect();
        }
        if let Err(e) = self.review_flow.reload_in_roots(cat, &roots) {
            self.banner_error = Some(format!("Error al cargar sugerencias: {e}"));
        }
        match cat.duplicate_groups_in_roots(&roots) {
            Ok(groups) => self.duplicate_groups = groups,
            Err(e) => self.banner_error = Some(format!("Error al consultar duplicados: {e}")),
        }
        match cat.list_vision_rules_in_roots(&roots) {
            Ok(rules) => {
                self.vision_rules = rules;
                self.tags_view.sync_selection(&self.vision_rules);
            }
            Err(e) => self.banner_error = Some(format!("Error al cargar reglas de visión: {e}")),
        }
        if let Ok(n) = cat.count_assets_with_vision_in_roots(&roots) {
            self.vision_analyzed_count = n;
        }
        if self.similarity_view.is_none() {
            self.reload_entries();
        } else {
            self.sync_inspector(true);
        }
    }

    fn reload_entries(&mut self) {
        self.similarity_view = None;
        let Some(cat) = &self.catalog else {
            return;
        };
        let stages = self.settings.enabled_stage_specs();
        let cf = self
            .filter
            .to_catalog_filter_scoped(&self.settings.library_roots, &stages);
        match cat.entries(&cf, self.filter.sort) {
            Ok(next) => {
                self.selection.reconcile(&self.entries, &next);
                self.entries = next;
                self.sync_inspector(true);
            }
            Err(e) => self.banner_error = Some(format!("Error al filtrar fotos: {e}")),
        }
    }

    fn refresh_reviewed_counts_light(&mut self) {
        let Some(cat) = &self.catalog else {
            return;
        };
        let stages = self.settings.enabled_stage_specs();
        if let Ok(rc) = cat.reviewed_counts(&stages, Some(&self.settings.library_roots)) {
            self.reviewed_counts = rc;
        }
        if self.filter.reviewed != ReviewedFilter::All && self.similarity_view.is_none() {
            self.reload_entries();
        }
    }

    fn sync_inspector(&mut self, force_labels: bool) {
        let primary = self.selection.primary().and_then(|i| self.entries.get(i));
        let next_asset = primary.map(|e| e.asset);
        let next_path = primary.map(|e| e.path.clone());

        if force_labels || self.inspector.asset != next_asset {
            if self.inspector.asset != next_asset {
                self.inspector.ocr_checked.clear();
                self.inspector.ocr_draft.clear();
            }
            self.inspector.asset = next_asset;
            self.inspector.labels = match (next_asset, &self.catalog) {
                (Some(asset), Some(cat)) => cat.labels_of(asset).unwrap_or_default(),
                _ => Vec::new(),
            };
            self.reload_inspector_details();
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

    fn reload_inspector_details(&mut self) {
        match (self.inspector.asset, &self.catalog) {
            (Some(asset), Some(cat)) => {
                self.inspector.ocr_lines = cat.ocr_lines(asset).unwrap_or_default();
                self.inspector.ocr_done = cat.has_ocr_done(asset).unwrap_or(false);
                self.inspector.faces = cat.asset_faces(asset).unwrap_or_default();
                self.inspector.vision_done = cat.has_vision_done(asset).unwrap_or(false);
            }
            _ => {
                self.inspector.ocr_lines.clear();
                self.inspector.ocr_done = false;
                self.inspector.faces.clear();
                self.inspector.vision_done = false;
            }
        }
    }

    fn foreground_heavy_task(&self) -> Option<&'static str> {
        if self.works_worker.is_some() {
            Some("la búsqueda de obras")
        } else if self.tags_apply_worker.is_some() {
            Some("la aplicación de etiquetas")
        } else {
            None
        }
    }

    fn sync_engine_yield(&self) {
        if let Some(engine) = &self.review_engine {
            engine.set_yielded(self.foreground_heavy_task().is_some());
        }
    }

    fn poll_workers(&mut self, ctx: &egui::Context) {
        let mut finished_works = false;
        let mut finished_tags_apply = false;
        let mut finished_rank = false;
        let mut needs_full_reload = false;
        let mut inspector_asset_updated = false;

        // 1. Poll background review engine
        if let Some(engine) = &self.review_engine {
            while let Some(ev) = engine.try_recv() {
                match ev {
                    ReviewEngineEvent::Status(snap) => {
                        self.engine_snapshot = snap;
                    }
                    ReviewEngineEvent::IndexFinished {
                        new,
                        changed,
                        missing,
                        errors,
                    } => {
                        needs_full_reload = true;
                        if new > 0 || changed > 0 || missing > 0 || errors > 0 {
                            self.status_notice = Some(format!(
                                "Cambios detectados: {}, {}, {}",
                                count_phrase(new, "nueva", "nuevas"),
                                count_phrase(changed, "actualizada", "actualizadas"),
                                count_phrase(missing, "ausente", "ausentes"),
                            ));
                        }
                    }
                    ReviewEngineEvent::AssetProgress {
                        asset,
                        now_reviewed,
                    } => {
                        if now_reviewed {
                            self.reviewed_assets.insert(asset);
                        } else {
                            self.reviewed_assets.remove(&asset);
                        }
                        if self.inspector.asset == Some(asset) {
                            inspector_asset_updated = true;
                        }
                        self.pending_engine_refresh = true;
                    }
                    ReviewEngineEvent::ChunkFinished => {
                        self.chunk_reload_pending = true;
                    }
                }
            }
        }

        // 2. Poll foreground workers (Works, TagsApply, VisionRank)
        if let Some(worker) = &self.works_worker {
            while let Some(ev) = worker.try_recv() {
                self.works_view.apply_event(&ev);
                match ev {
                    WorksEvent::Progress { .. } => {}
                    WorksEvent::Finished {
                        stopped,
                        processed,
                        matched_assets,
                        suggestions,
                    } => {
                        finished_works = true;
                        needs_full_reload = true;
                        let prefix = if stopped {
                            "Búsqueda de obras detenida"
                        } else {
                            "Búsqueda de obras completa"
                        };
                        self.status_notice = Some(format!(
                            "{prefix}: {} revisadas, {} con coincidencias ({})",
                            count_phrase(processed, "foto", "fotos"),
                            count_phrase(matched_assets, "foto", "fotos"),
                            count_phrase(suggestions, "sugerencia", "sugerencias"),
                        ));
                    }
                    WorksEvent::Failed(err) => {
                        finished_works = true;
                        self.banner_error = Some(format!("Error al buscar obras: {err}"));
                    }
                }
            }
        }

        if let Some(worker) = &self.tags_apply_worker {
            while let Some(ev) = worker.try_recv() {
                self.tags_view.apply_event(&ev);
                match ev {
                    TagsApplyEvent::LoadingModel | TagsApplyEvent::Progress { .. } => {}
                    TagsApplyEvent::Finished {
                        stopped,
                        processed,
                        matched_assets,
                        suggestions,
                    } => {
                        finished_tags_apply = true;
                        needs_full_reload = true;
                        let prefix = if stopped {
                            "Aplicación de etiquetas detenida"
                        } else {
                            "Etiquetas aplicadas"
                        };
                        self.status_notice = Some(format!(
                            "{prefix}: {} evaluadas, {} con sugerencias ({})",
                            count_phrase(processed, "foto", "fotos"),
                            count_phrase(matched_assets, "foto", "fotos"),
                            count_phrase(suggestions, "sugerencia", "sugerencias"),
                        ));
                    }
                    TagsApplyEvent::Failed(err) => {
                        finished_tags_apply = true;
                        self.banner_error = Some(format!("Error al aplicar etiquetas: {err}"));
                    }
                }
            }
        }

        if let Some(worker) = &self.vision_rank_worker
            && let Some(res) = worker.try_recv()
        {
            finished_rank = true;
            match res {
                VisionRankResult::Ready {
                    title,
                    threshold,
                    ranked,
                } => {
                    self.tags_view.rank_error = None;
                    let scores: HashMap<AssetId, f32> =
                        ranked.iter().map(|r| (r.entry.asset, r.score)).collect();
                    let next_entries: Vec<Entry> = ranked.into_iter().map(|r| r.entry).collect();
                    self.selection.reconcile(&self.entries, &next_entries);
                    self.entries = next_entries;
                    self.similarity_view = Some(SimilarityView {
                        title,
                        threshold,
                        scores,
                    });
                    self.active_view = ActiveView::Gallery;
                    self.sync_inspector(true);
                }
                VisionRankResult::Failed(err) => {
                    self.tags_view.rank_error = Some(err);
                }
            }
        }

        if finished_works {
            self.works_worker = None;
            self.sync_engine_yield();
        }
        if finished_tags_apply {
            self.tags_apply_worker = None;
            self.sync_engine_yield();
        }
        if finished_rank {
            self.vision_rank_worker = None;
        }

        needs_full_reload |= chunk_reload_due(
            self.chunk_reload_pending,
            self.last_full_reload.elapsed(),
            &self.engine_snapshot.phase,
        );
        if needs_full_reload {
            self.reload_catalog();
            self.chunk_reload_pending = false;
            self.last_full_reload = Instant::now();
            self.last_engine_refresh = Instant::now();
            self.pending_engine_refresh = false;
        } else if self.pending_engine_refresh
            && self.last_engine_refresh.elapsed() >= ENGINE_REFRESH_INTERVAL
        {
            self.refresh_reviewed_counts_light();
            if inspector_asset_updated {
                self.sync_inspector(true);
            }
            self.last_engine_refresh = Instant::now();
            self.pending_engine_refresh = false;
        } else if inspector_asset_updated {
            self.sync_inspector(true);
        }

        // 3. Thumbnails & preview
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

    fn save_settings_and_sync_engine(&mut self) {
        if let Err(e) = settings::save(&self.data_dir, &self.settings) {
            self.banner_error = Some(e.to_string());
        }
        if let Some(engine) = &self.review_engine {
            engine.update_config(self.settings.to_review_config());
        }
    }

    fn pick_and_add_library(&mut self) {
        if let Some(folder) = rfd::FileDialog::new()
            .set_title("Elegir carpeta con fotos (solo lectura)")
            .pick_folder()
        {
            match self.settings.add_library_root(&folder, &self.data_dir) {
                Ok(_) => {
                    self.force_welcome = false;
                    self.banner_error = None;
                    if self.catalog.is_none()
                        && let Ok(db) =
                            settings::db_dest_path(&self.data_dir, &self.settings.valid_roots())
                        && let Ok(cat) = Catalog::open(&db)
                    {
                        self.catalog = Some(cat);
                    }
                    self.save_settings_and_sync_engine();
                    if let Some(engine) = &self.review_engine {
                        engine.trigger_index();
                    }
                    self.reload_catalog();
                }
                Err(e) => self.banner_error = Some(e.to_string()),
            }
        }
    }

    fn remove_library(&mut self, path: &Path) {
        if self.settings.remove_library_root(path) {
            self.save_settings_and_sync_engine();
            self.reload_catalog();
            self.status_notice = Some(format!(
                "Carpeta quitada de la aplicación: {} (ningún archivo fue modificado)",
                path.display()
            ));
        }
    }

    fn request_review_assets(&mut self, assets: Vec<AssetId>) {
        if assets.is_empty() {
            return;
        }
        let stages = self.settings.enabled_stage_specs();
        if stages.is_empty() {
            self.status_notice =
                Some("Activa al menos una etapa en Configuración para revisar fotos".into());
            return;
        }
        if self.settings.stage_vision && self.settings.vision_models_dir.is_none() {
            self.status_notice =
                Some("Falta la carpeta de modelos de visión; configúrala en Configuración".into());
            return;
        }
        if self.settings.stage_ocr && self.settings.ocr_models_dir.is_none() {
            self.status_notice =
                Some("Falta la carpeta de modelos OCR; configúrala en Configuración".into());
            return;
        }
        if self.demo_mode {
            self.status_notice = Some(format!(
                "Modo demo: {} en cola de revisión (sin cargar modelos)",
                count_phrase(assets.len(), "foto", "fotos")
            ));
            return;
        }

        let stage_names: Vec<&str> = stages.iter().map(|s| s.stage.as_str()).collect();
        if let Some(cat) = &mut self.catalog
            && let Err(e) = cat.reset_stages(&assets, &stage_names)
        {
            self.banner_error = Some(format!("No se pudo reiniciar la revisión: {e}"));
            return;
        }
        for id in &assets {
            self.reviewed_assets.remove(id);
        }
        self.refresh_reviewed_counts_light();

        if let Some(engine) = &self.review_engine {
            engine.prioritize(assets);
        }
    }

    fn find_similar_to(&mut self, entry: &Entry) {
        let Some(cat) = &self.catalog else {
            return;
        };
        let emb = match cat.asset_embedding(entry.asset) {
            Ok(Some((model, vec))) => (model, vec),
            Ok(None) => {
                self.status_notice =
                    Some("Esta foto aún no tiene vector de reconocimiento visual".into());
                return;
            }
            Err(e) => {
                self.banner_error = Some(format!("No se pudo leer el vector visual: {e}"));
                return;
            }
        };
        match cat.rank_by_embeddings_in_roots(
            &emb.0,
            &[emb.1.as_slice()],
            &self.settings.library_roots,
            250,
        ) {
            Ok(ranked) => {
                let filename = entry
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| entry.path.display().to_string());
                let scores: HashMap<AssetId, f32> =
                    ranked.iter().map(|r| (r.entry.asset, r.score)).collect();
                let next_entries: Vec<Entry> = ranked.into_iter().map(|r| r.entry).collect();
                self.selection.reconcile(&self.entries, &next_entries);
                self.entries = next_entries;
                self.similarity_view = Some(SimilarityView {
                    title: format!("Parecidas a «{filename}»"),
                    threshold: None,
                    scores,
                });
                self.active_view = ActiveView::Gallery;
                self.sync_inspector(true);
            }
            Err(e) => {
                self.banner_error = Some(format!("No se pudieron buscar fotos parecidas: {e}"));
            }
        }
    }

    fn start_works_match(&mut self, ctx: &egui::Context) {
        if self.works_worker.is_some() {
            return;
        }
        if let Some(task) = self.foreground_heavy_task() {
            self.status_notice = Some(format!("Espera a que termine {task}"));
            return;
        }
        let roots = self.settings.valid_roots();
        match settings::db_dest_path(&self.data_dir, &roots) {
            Ok(db) => {
                let ctx_clone = ctx.clone();
                let notify: Notify = Arc::new(move || ctx_clone.request_repaint());
                self.works_view.run_state = WorksRunState::Running {
                    done: 0,
                    total: 0,
                    matched_assets: 0,
                    suggestions: 0,
                };
                self.works_worker = Some(WorksWorker::start(db, Some(notify)));
                self.sync_engine_yield();
            }
            Err(e) => self.banner_error = Some(e.to_string()),
        }
    }

    fn start_tags_apply(&mut self, ctx: &egui::Context) {
        if self.tags_apply_worker.is_some() {
            return;
        }
        if let Some(task) = self.foreground_heavy_task() {
            self.status_notice = Some(format!("Espera a que termine {task}"));
            return;
        }
        let roots = self.settings.valid_roots();
        match settings::db_dest_path(&self.data_dir, &roots) {
            Ok(db) => {
                let ctx_clone = ctx.clone();
                let notify: Notify = Arc::new(move || ctx_clone.request_repaint());
                self.tags_view.apply_state = TagsApplyState::Running {
                    done: 0,
                    total: self.vision_analyzed_count,
                    matched_assets: 0,
                    suggestions: 0,
                };
                self.tags_apply_worker = Some(TagsApplyWorker::start(
                    db,
                    self.settings.vision_models_dir.clone(),
                    self.settings.ocr_device,
                    Some(notify),
                ));
                self.sync_engine_yield();
            }
            Err(e) => self.banner_error = Some(e.to_string()),
        }
    }

    fn start_vision_rank(
        &mut self,
        ctx: &egui::Context,
        title: String,
        texts: Vec<String>,
        threshold: Option<f32>,
    ) {
        if self.vision_rank_worker.is_some() {
            return;
        }
        let roots = self.settings.valid_roots();
        match settings::db_dest_path(&self.data_dir, &roots) {
            Ok(db) => {
                let ctx_clone = ctx.clone();
                let notify: Notify = Arc::new(move || ctx_clone.request_repaint());
                self.tags_view.rank_error = None;
                self.vision_rank_worker = Some(VisionRankWorker::start(
                    db,
                    self.settings.vision_models_dir.clone(),
                    self.settings.ocr_device,
                    self.settings.library_roots.clone(),
                    title,
                    texts,
                    threshold,
                    250,
                    Some(notify),
                ));
            }
            Err(e) => self.tags_view.rank_error = Some(e.to_string()),
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.photo_menu.is_some() {
            // While the photo context menu is open, it owns Arrow/Enter/Escape keys.
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
            if self.active_view != ActiveView::Gallery {
                return;
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

            // Shift+F10 opens the photo context menu for the current photo.
            // Note: egui 0.37's Key enum does not expose the Windows Application/Menu key;
            // Shift+F10 is the standard Windows keyboard equivalent supported here.
            if i.consume_key(Modifiers::SHIFT, Key::F10)
                && self.selection.primary().is_some()
                && !self.viewer_open
            {
                let pos = self
                    .last_primary_cell_pos
                    .unwrap_or_else(|| ctx.content_rect().center());
                self.photo_menu = Some(PhotoMenuState::at(pos));
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

    fn execute_photo_menu_action(&mut self, ctx: &egui::Context, action: PhotoMenuAction) {
        let Some(primary_entry) = self
            .selection
            .primary()
            .and_then(|i| self.entries.get(i))
            .cloned()
        else {
            return;
        };

        match action {
            PhotoMenuAction::OpenViewer => {
                self.viewer_open = true;
            }
            PhotoMenuAction::RevealInExplorer => {
                if let Err(e) = views::duplicates::reveal_in_explorer(&primary_entry.path) {
                    self.banner_error = Some(format!("No se pudo abrir el Explorador: {e}"));
                }
            }
            PhotoMenuAction::CopyPath => {
                ctx.copy_text(primary_entry.path.display().to_string());
                self.status_notice = Some("Ruta copiada al portapapeles".into());
            }
            PhotoMenuAction::ReviewSelected => {
                let assets: Vec<AssetId> = self
                    .selection
                    .selected_indices()
                    .into_iter()
                    .filter_map(|i| self.entries.get(i).map(|e| e.asset))
                    .collect();
                self.request_review_assets(assets);
            }
            PhotoMenuAction::AddTag => {
                self.inspector.focus_new_label_next_frame = true;
            }
            PhotoMenuAction::CopyOcrText => {
                let is_screenshot = self.inspector.labels.iter().any(|a| {
                    a.kind == catalog::LabelKind::Category
                        && a.name.eq_ignore_ascii_case(SCREENSHOT_CATEGORY)
                        && a.status != Status::Rejected
                });
                let joined: String = self
                    .inspector
                    .ocr_lines
                    .iter()
                    .filter(|l| !(l.top_strip && is_screenshot))
                    .map(|l| l.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !joined.is_empty() {
                    ctx.copy_text(joined);
                    self.status_notice = Some("Texto leído copiado al portapapeles".into());
                }
            }
            PhotoMenuAction::FindSimilar => {
                self.find_similar_to(&primary_entry);
            }
            PhotoMenuAction::Export => {
                if let Some(cat) = &self.catalog {
                    self.export_wizard = Some(crate::export_flow::ExportWizard::open(
                        cat,
                        &self.entries,
                        &self.selection,
                        self.settings.last_export_dir.clone(),
                    ));
                }
            }
            PhotoMenuAction::ClearSelection => {
                self.selection.clear();
                self.sync_inspector(false);
            }
        }
    }

    fn step_screenshot(&mut self, ctx: &egui::Context) {
        let has_entries = !self.entries.is_empty();
        let has_selection = !self.selection.is_empty();
        let thumbs_ready = self.last_thumbs_ready;
        let preview_ready = match &self.preview_requested {
            None => true,
            Some(path) => matches!(&self.preview_loaded, Some((p, _)) if p == path),
        };
        let export_open = self.export_wizard.is_some();
        let export_ready = self.export_wizard.as_ref().is_some_and(|w| {
            matches!(
                w.phase(),
                crate::export_flow::ExportPhase::Preview(crate::export_flow::PreviewState::Ready(
                    _
                ))
            )
        });
        let export_failed = self.export_open_failed
            || self.export_wizard.as_ref().is_some_and(|w| {
                w.validation_error().is_some()
                    || matches!(w.phase(), crate::export_flow::ExportPhase::Error(_))
            });
        let menu_open = self.photo_menu.is_some();

        let step = self
            .screenshot
            .as_mut()
            .map_or(crate::screenshot::CaptureStep::None, |ctrl| {
                ctrl.step(
                    ctx,
                    crate::screenshot::FrameSignals {
                        has_entries,
                        has_selection,
                        thumbs_ready,
                        preview_ready,
                        export_open,
                        export_ready,
                        export_failed,
                        menu_open,
                        image_received: false,
                    },
                )
            });
        match step {
            crate::screenshot::CaptureStep::SelectFirst if has_entries => {
                self.selection.click(0, self.entries.len());
                self.sync_inspector(false);
            }
            crate::screenshot::CaptureStep::OpenExport if has_entries => {
                self.open_export_for_capture(ctx);
            }
            crate::screenshot::CaptureStep::OpenMenu if has_entries => {
                self.selection.click(0, self.entries.len());
                self.sync_inspector(false);
                let pos = self
                    .last_primary_cell_pos
                    .unwrap_or_else(|| Pos2::new(380.0, 240.0));
                self.photo_menu = Some(PhotoMenuState::at(pos));
            }
            _ => {}
        }
    }

    fn open_export_for_capture(&mut self, ctx: &egui::Context) {
        let Some(cat) = self.catalog.as_ref() else {
            self.export_open_failed = true;
            return;
        };
        let dest = self
            .preset_export_dir
            .clone()
            .or_else(|| self.settings.last_export_dir.clone());
        let mut wizard =
            crate::export_flow::ExportWizard::open(cat, &self.entries, &Selection::default(), dest);
        if matches!(wizard.phase(), crate::export_flow::ExportPhase::Error(_)) {
            self.export_open_failed = true;
        } else {
            let roots = self.settings.valid_roots();
            let ctx_clone = ctx.clone();
            let notify: Notify = Arc::new(move || ctx_clone.request_repaint());
            if wizard.request_preview(&roots, Some(notify)).is_err() {
                self.export_open_failed = true;
            }
        }
        self.export_wizard = Some(wizard);
    }
}

impl eframe::App for PhotosortApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_workers(&ctx);
        self.handle_shortcuts(&ctx);

        let palette = theme::palette_for(&ctx);

        if self.force_welcome || self.settings.library_roots.is_empty() {
            if views::shell::draw_welcome(ui, palette, self.logo_texture.as_ref()) {
                self.pick_and_add_library();
            }
            self.step_screenshot(&ctx);
            return;
        }

        // 1. Left sidebar (always present when folders exist)
        let sidebar_out = views::shell::draw_sidebar(
            ui,
            palette,
            self.logo_texture.as_ref(),
            self.active_view,
            self.reviewed_counts,
            self.review_flow.total_items(),
            self.duplicate_groups.len(),
            self.works.len(),
            self.vision_rules.len(),
            &self.labels_in_use,
            &mut self.filter,
            &mut self.sidebar_state,
        );
        if let Some(view) = sidebar_out.switch_view {
            if view != ActiveView::Gallery {
                self.viewer_open = false;
                self.photo_menu = None;
            }
            self.active_view = view;
        }
        if let Some(rev) = sidebar_out.set_reviewed_filter {
            self.filter.reviewed = rev;
            self.reload_entries();
        } else if sidebar_out.filter_changed {
            self.reload_entries();
        }

        // 2. Minimal top bar (search + background review status)
        let top_out = views::shell::draw_top_bar(
            ui,
            palette,
            &mut self.filter.search,
            &mut self.focus_search_next_frame,
            &self.engine_snapshot,
            self.banner_error.as_deref(),
            self.status_notice.as_deref(),
        );
        if top_out.search_changed {
            self.active_view = ActiveView::Gallery;
            self.reload_entries();
        }
        if top_out.open_settings {
            self.active_view = ActiveView::Settings;
        }
        if top_out.toggle_pause {
            if self.demo_mode {
                self.demo_paused = !self.demo_paused;
                self.engine_snapshot.phase = if self.demo_paused {
                    EnginePhase::Paused
                } else {
                    EnginePhase::Running {
                        done: self.reviewed_counts.reviewed,
                        total: self.reviewed_counts.all.max(1),
                        stage: StageKind::Rules,
                        elapsed_secs: 9.6,
                    }
                };
            } else if let Some(engine) = &self.review_engine {
                let next = !engine.is_paused();
                engine.set_paused(next);
            }
        }
        if top_out.retry_failed
            && let Some(engine) = &self.review_engine
        {
            engine.retry_failed();
        }
        if top_out.dismiss_error {
            self.banner_error = None;
        }
        if top_out.dismiss_notice {
            self.status_notice = None;
        }

        // 3. Main view
        match self.active_view {
            ActiveView::Gallery => {
                // Inspector hidden when nothing is selected!
                let primary_entry = self
                    .selection
                    .primary()
                    .and_then(|i| self.entries.get(i))
                    .cloned();
                let primary_reviewed = primary_entry
                    .as_ref()
                    .is_some_and(|e| self.reviewed_assets.contains(&e.asset));
                let insp_out = views::inspector::draw(
                    ui,
                    palette,
                    primary_entry.as_ref(),
                    primary_reviewed,
                    &mut self.inspector,
                    &self.works,
                    self.preview_loaded.as_ref(),
                );

                let gal_out = views::gallery::draw(
                    ui,
                    palette,
                    &self.entries,
                    &mut self.selection,
                    &mut self.filter.sort,
                    self.similarity_view.as_ref(),
                    &self.reviewed_assets,
                    &self.pending_suggestions_assets,
                    self.counts.locations > 0,
                    &self.thumb_pool,
                    &self.thumb_textures,
                    &self.thumb_failed,
                );
                self.last_thumbs_ready = gal_out.thumbs_ready;
                self.last_columns = gal_out.last_columns;
                if gal_out.primary_cell_pos.is_some() {
                    self.last_primary_cell_pos = gal_out.primary_cell_pos;
                }
                if gal_out.open_viewer || insp_out.open_viewer {
                    self.viewer_open = true;
                }
                if gal_out.sort_changed || gal_out.exit_similarity {
                    self.reload_entries();
                } else if gal_out.selection_changed {
                    self.sync_inspector(false);
                }
                if let Some((_idx, pos)) = gal_out.open_photo_menu {
                    self.photo_menu = Some(PhotoMenuState::at(pos));
                }
                if gal_out.review_selected {
                    let assets: Vec<AssetId> = self
                        .selection
                        .selected_indices()
                        .into_iter()
                        .filter_map(|i| self.entries.get(i).map(|e| e.asset))
                        .collect();
                    self.request_review_assets(assets);
                }
                if let Some(asset) = insp_out.review_asset {
                    self.request_review_assets(vec![asset]);
                }
                if gal_out.open_export
                    && let Some(cat) = &self.catalog
                {
                    self.export_wizard = Some(crate::export_flow::ExportWizard::open(
                        cat,
                        &self.entries,
                        &self.selection,
                        self.settings.last_export_dir.clone(),
                    ));
                }

                // Apply inspector catalog mutations
                let mut catalog_mutated = false;
                if let Some(cat) = &self.catalog {
                    if let Some((asset, label)) = insp_out.accept_action {
                        match cat.accept(asset, label) {
                            Ok(()) => catalog_mutated = true,
                            Err(e) => self.banner_error = Some(format!("No se pudo aceptar: {e}")),
                        }
                    }
                    if let Some((asset, label)) = insp_out.reject_action {
                        match cat.reject(asset, label) {
                            Ok(()) => catalog_mutated = true,
                            Err(e) => self.banner_error = Some(format!("No se pudo rechazar: {e}")),
                        }
                    }
                    if let Some((asset, kind, name)) = insp_out.add_user_label {
                        match cat.label(kind, &name).and_then(|id| cat.accept(asset, id)) {
                            Ok(()) => catalog_mutated = true,
                            Err(e) => {
                                self.banner_error = Some(format!("No se pudo añadir etiqueta: {e}"))
                            }
                        }
                    }
                }
                if let Some(cat) = &mut self.catalog {
                    if let Some((asset, name)) = insp_out.create_work_from_ocr {
                        match cat
                            .create_work(&name)
                            .and_then(|_| cat.load_work_matcher())
                            .and_then(|m| cat.apply_work_matches(asset, &m))
                        {
                            Ok(_) => {
                                self.inspector.ocr_checked.clear();
                                self.inspector.ocr_draft.clear();
                                self.status_notice = Some(
                                    "Obra creada. En Obras puedes buscarla en las demás fotos"
                                        .into(),
                                );
                                catalog_mutated = true;
                            }
                            Err(e) => {
                                self.banner_error = Some(format!("No se pudo crear la obra: {e}"))
                            }
                        }
                    }
                    if let Some((asset, target, alias)) = insp_out.add_alias_from_ocr {
                        match cat.add_work_alias(target, &alias) {
                            Ok(true) => {
                                if let Ok(m) = cat.load_work_matcher() {
                                    let _ = cat.apply_work_matches(asset, &m);
                                }
                                self.inspector.ocr_checked.clear();
                                self.inspector.ocr_draft.clear();
                                self.status_notice = Some(
                                    "Alias añadido. En Obras puedes aplicarlo a las demás fotos"
                                        .into(),
                                );
                                catalog_mutated = true;
                            }
                            Ok(false) => {
                                self.status_notice = Some(
                                    "Ese alias ya existe o tiene menos de 2 caracteres alfanuméricos"
                                        .into(),
                                );
                            }
                            Err(e) => {
                                self.banner_error = Some(format!("No se pudo añadir el alias: {e}"))
                            }
                        }
                    }
                }
                if catalog_mutated {
                    self.reload_catalog();
                }

                // Viewer overlay
                if views::gallery::draw_viewer_overlay(
                    &ctx,
                    palette,
                    &mut self.viewer_open,
                    &self.entries,
                    &mut self.selection,
                    self.preview_loaded.as_ref(),
                ) {
                    self.sync_inspector(false);
                }

                // Photo context menu
                if self.photo_menu.is_some() {
                    let all_reviewed = self
                        .selection
                        .selected_indices()
                        .into_iter()
                        .filter_map(|i| self.entries.get(i))
                        .all(|e| self.reviewed_assets.contains(&e.asset));
                    let has_ocr_text = !self.inspector.ocr_lines.is_empty();
                    let has_embedding = self
                        .inspector
                        .asset
                        .and_then(|a| self.catalog.as_ref()?.asset_embedding(a).ok().flatten())
                        .is_some();
                    let menu_ctx = PhotoMenuContext {
                        selected_count: self.selection.len().max(1),
                        all_reviewed,
                        has_ocr_text,
                        has_embedding,
                    };
                    if let Some(action) = views::context_menu::draw_photo_menu(
                        &ctx,
                        palette,
                        &mut self.photo_menu,
                        menu_ctx,
                    ) {
                        self.execute_photo_menu_action(&ctx, action);
                    }
                }
            }
            ActiveView::Suggestions => {
                let outcome = views::review::draw(
                    ui,
                    palette,
                    &mut self.review_flow,
                    self.catalog.as_ref(),
                    &self.thumb_pool,
                    &self.thumb_textures,
                    &self.thumb_failed,
                );
                self.last_thumbs_ready = outcome.thumbs_ready;
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
            ActiveView::Duplicates => {
                let outcome = views::duplicates::draw(
                    ui,
                    palette,
                    &self.duplicate_groups,
                    &self.thumb_pool,
                    &self.thumb_textures,
                    &self.thumb_failed,
                );
                self.last_thumbs_ready = outcome.thumbs_ready;
                if let Some(err) = outcome.error {
                    self.banner_error = Some(err);
                }
            }
            ActiveView::Works => {
                let other_heavy = if self.works_worker.is_some() {
                    None
                } else {
                    self.foreground_heavy_task()
                };
                let outcome = views::works::draw(
                    ui,
                    palette,
                    &mut self.works_view,
                    &self.works,
                    self.catalog.as_mut(),
                    self.works_worker.is_some(),
                    other_heavy,
                );
                if let Some(err) = outcome.error {
                    self.banner_error = Some(err);
                }
                if let Some(notice) = outcome.status_notice {
                    self.status_notice = Some(notice);
                }
                if let Some(work_id) = outcome.deleted {
                    self.filter.set_chip(work_id, None);
                }
                if outcome.catalog_mutated {
                    self.reload_catalog();
                }
                if outcome.cancel_match
                    && let Some(worker) = &self.works_worker
                {
                    worker.cancel();
                }
                if outcome.start_match {
                    self.start_works_match(&ctx);
                }
                if let Some(work_id) = outcome.open_in_gallery {
                    self.filter.clear_filters();
                    self.filter.set_chip(work_id, Some(ChipMode::Require));
                    self.active_view = ActiveView::Gallery;
                    self.reload_entries();
                }
            }
            ActiveView::Tags => {
                let other_heavy = if self.tags_apply_worker.is_some() {
                    None
                } else {
                    self.foreground_heavy_task()
                };
                let outcome = views::tags::draw(
                    ui,
                    palette,
                    &mut self.tags_view,
                    &self.vision_rules,
                    self.vision_analyzed_count,
                    self.catalog.as_mut(),
                    self.tags_apply_worker.is_some(),
                    self.vision_rank_worker.is_some(),
                    other_heavy,
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
                if outcome.cancel_apply
                    && let Some(worker) = &self.tags_apply_worker
                {
                    worker.cancel();
                }
                if outcome.start_apply {
                    self.start_tags_apply(&ctx);
                }
                if let Some((title, texts, threshold)) = outcome.rank_request {
                    self.start_vision_rank(&ctx, title, texts, threshold);
                }
            }
            ActiveView::Settings => {
                let outcome = views::settings::draw(
                    ui,
                    palette,
                    &mut self.settings,
                    &mut self.settings_state,
                    &self.root_counts,
                    &self.engine_snapshot,
                    &self.data_dir,
                    self.demo_mode,
                );
                if let Some(err) = outcome.error {
                    self.banner_error = Some(err);
                }
                if outcome.add_folder {
                    self.pick_and_add_library();
                }
                if let Some(root) = outcome.remove_folder {
                    self.remove_library(&root);
                }
                if outcome.scan_changes_now {
                    if let Some(task) = self.foreground_heavy_task() {
                        self.status_notice = Some(format!("Espera a que termine {task}"));
                    } else if let Some(engine) = &self.review_engine {
                        engine.trigger_index();
                    }
                }
                if outcome.theme_changed {
                    theme::apply_theme_preference(&ctx, self.settings.theme);
                }
                if outcome.settings_changed {
                    self.save_settings_and_sync_engine();
                    self.reload_catalog();
                }
            }
        }

        // Export wizard modal (when opened from selection bar or photo context menu)
        views::export::draw_window(
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
