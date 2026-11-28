use crate::face_group_worker::{FaceGroupEvent, FaceGroupWorker};
use crate::model::{ChipMode, FilterState, MoveDir, Selection, count_phrase};
use crate::near_dupes_worker::{NearDupesEvent, NearDupesWorker};
use crate::review_engine::{
    EnginePhase, EngineSnapshot, FolderRecheck, ReviewEngine, ReviewEngineEvent, StageKind,
    chunk_reload_due,
};
use crate::review_flow::ReviewFlow;
use crate::screenshot::CaptureView;
use crate::settings::{self, Settings, Theme};
use crate::theme;
use crate::views;
use crate::views::context_menu::{PhotoMenuAction, PhotoMenuContext, PhotoMenuState};
use crate::views::duplicates::DuplicatesViewState;
use crate::views::gallery::SimilarityView;
use crate::views::inspector::InspectorState;
use crate::views::label_actions::LabelActionModal;
use crate::views::settings::SettingsViewState;
use crate::views::shell::{ActiveView, SidebarState};
use crate::views::tags::{TagsApplyState, TagsViewState};
use crate::views::works::{WorksRunState, WorksViewState};
use crate::vision_worker::{TagsApplyEvent, TagsApplyWorker, VisionRankResult, VisionRankWorker};
use crate::worker::{Notify, PreviewWorker, ThumbPool, ThumbRequest, ThumbnailProvider};
use crate::works_worker::{WorksEvent, WorksWorker};
use catalog::{
    AssetId, Catalog, Counts, DuplicateGroup, Entry, LabelId, LabelNode, MetadataFacets,
    NearDuplicateGroup, NearDuplicateMeta, ReviewedCounts, ReviewedFilter, SCREENSHOT_CATEGORY,
    StageErrorRecord, Status, VisionRule, WorkSummary,
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
    all_labels: Vec<LabelNode>,
    metadata_facets: MetadataFacets,
    works: Vec<WorkSummary>,
    works_view: WorksViewState,
    persons: Vec<catalog::Person>,
    persons_view: views::persons::PersonsViewState,
    person_cards: views::persons::PersonCards,
    vision_rules: Vec<VisionRule>,
    vision_analyzed_count: usize,
    tags_view: TagsViewState,
    similarity_view: Option<SimilarityView>,
    work_order_view: Option<(LabelId, String)>,
    pending_suggestions_assets: HashSet<AssetId>,
    review_flow: ReviewFlow,
    duplicate_groups: Vec<DuplicateGroup>,
    near_duplicate_groups: Vec<NearDuplicateGroup>,
    near_duplicate_meta: Option<NearDuplicateMeta>,
    duplicates_view: DuplicatesViewState,
    stage_errors: Vec<StageErrorRecord>,
    stage_error_assets_count: usize,

    filter: FilterState,
    sidebar_state: SidebarState,
    settings_state: SettingsViewState,
    label_modal: LabelActionModal,
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
    folder_recheck: FolderRecheck,
    /// Photos that could not be found, with the thumbnail key when a thumbnail reported it.
    gone_photos: Vec<(PathBuf, Option<String>)>,

    works_worker: Option<WorksWorker>,
    tags_apply_worker: Option<TagsApplyWorker>,
    vision_rank_worker: Option<VisionRankWorker>,
    near_dupes_worker: Option<NearDupesWorker>,
    face_group_worker: Option<FaceGroupWorker>,
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
    /// Asked of the catalog at startup and after each move or undo, not every frame.
    can_undo_move: bool,
    undo_worker: Option<crate::export_flow::UndoWorker>,
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
        cc.egui_ctx.add_plugin(theme::SpanishNames);

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
            all_labels: Vec::new(),
            metadata_facets: MetadataFacets::default(),
            works: Vec::new(),
            works_view: WorksViewState::default(),
            persons: Vec::new(),
            persons_view: views::persons::PersonsViewState::default(),
            person_cards: Default::default(),
            vision_rules: Vec::new(),
            vision_analyzed_count: 0,
            tags_view: TagsViewState::default(),
            similarity_view: None,
            work_order_view: None,
            pending_suggestions_assets: HashSet::new(),
            review_flow: ReviewFlow::new(),
            duplicate_groups: Vec::new(),
            near_duplicate_groups: Vec::new(),
            near_duplicate_meta: None,
            duplicates_view: DuplicatesViewState::default(),
            stage_errors: Vec::new(),
            stage_error_assets_count: 0,
            filter: FilterState::default(),
            sidebar_state: SidebarState::default(),
            settings_state: SettingsViewState::default(),
            label_modal: LabelActionModal::default(),
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
            folder_recheck: FolderRecheck::new(Instant::now()),
            gone_photos: Vec::new(),
            works_worker: None,
            tags_apply_worker: None,
            vision_rank_worker: None,
            near_dupes_worker: None,
            face_group_worker: None,
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
            can_undo_move: false,
            undo_worker: None,
            screenshot: None,
            last_thumbs_ready: false,
        };
        app.reload_catalog();
        app.refresh_can_undo();
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
            CaptureView::Persons => {
                self.active_view = ActiveView::Persons;
            }
            CaptureView::Tags => {
                self.active_view = ActiveView::Tags;
            }
            CaptureView::Errors => {
                self.active_view = ActiveView::Errors;
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
        match cat.list_labels_in_roots(&roots) {
            Ok(list) => self.all_labels = list,
            Err(e) => self.banner_error = Some(format!("Error al cargar etiquetas: {e}")),
        }
        match cat.metadata_facets_in_roots(&roots) {
            Ok(facets) => self.metadata_facets = facets,
            Err(e) => {
                self.banner_error = Some(format!("Error al cargar filtros de metadatos: {e}"))
            }
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
        match cat.list_proposed_work_groups(Some(&roots)) {
            Ok(groups) => self.works_view.proposed_groups = groups,
            Err(e) => self.banner_error = Some(format!("Error al cargar grupos propuestos: {e}")),
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
        match cat.near_duplicate_groups_in_roots(&roots) {
            Ok(groups) => self.near_duplicate_groups = groups,
            Err(e) => self.banner_error = Some(format!("Error al consultar parecidos: {e}")),
        }
        if let Ok(meta) = cat.near_duplicate_meta() {
            if self.near_duplicate_meta.is_none()
                && self.duplicates_view.running.is_none()
                && let Some(m) = meta
            {
                self.duplicates_view.threshold = m.threshold;
            }
            self.near_duplicate_meta = meta;
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
        match cat.list_persons_in_roots(true, &roots) {
            Ok(p) => {
                // A person no longer shown has no button to clear its filter.
                self.filter
                    .persons
                    .retain(|id| p.iter().any(|person| person.id == *id));
                self.persons = p;
            }
            Err(e) => self.banner_error = Some(format!("Error al cargar personas: {e}")),
        }
        self.reload_person_cards();
        let Some(cat) = &self.catalog else {
            return;
        };
        match cat.list_stage_errors_in_roots(&roots) {
            Ok(list) => self.stage_errors = list,
            Err(e) => self.banner_error = Some(format!("Error al cargar errores de revisión: {e}")),
        }
        if let Ok(n) = cat.count_stage_error_assets_in_roots(&roots) {
            self.stage_error_assets_count = n;
        }
        if let Some((work_id, _)) = self.work_order_view.clone() {
            if let Ok(next) = cat.entries_belonging_to_work(work_id, Some(&roots)) {
                self.selection.reconcile(&self.entries, &next);
                self.entries = next;
                self.sync_inspector(true);
            }
        } else if self.similarity_view.is_none() {
            self.reload_entries();
        } else {
            self.sync_inspector(true);
        }
    }

    fn reload_entries(&mut self) {
        self.similarity_view = None;
        self.work_order_view = None;
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

    fn reload_person_cards(&mut self) {
        if let Some(cat) = &self.catalog {
            self.person_cards = views::persons::PersonCards::load(
                cat,
                &self.settings.library_roots,
                &self.persons_view.postponed_persons,
            );
        }
    }

    /// Face crops are cut from gallery thumbnails: asks for the ones a face view needs, even when
    /// those photos are not on screen in the gallery.
    fn request_face_thumbs<'a>(&self, thumbs: impl IntoIterator<Item = &'a ThumbRequest>) {
        for thumb in thumbs {
            if !self.thumb_textures.contains_key(&thumb.sha256)
                && !self.thumb_failed.contains(&thumb.sha256)
            {
                self.thumb_pool.request(thumb.clone());
            }
        }
    }

    fn reload_inspector_details(&mut self) {
        match (self.inspector.asset, &self.catalog) {
            (Some(asset), Some(cat)) => {
                self.inspector.asset_work = cat.asset_work(asset).unwrap_or(None);
                self.inspector.covers = cat.asset_covers(asset).unwrap_or_default();
                self.inspector.ocr_lines = cat.ocr_lines(asset).unwrap_or_default();
                self.inspector.ocr_done = cat.has_ocr_done(asset).unwrap_or(false);
                let drawn = cat.labels_of(asset).unwrap_or_default().iter().any(|a| {
                    a.kind == catalog::LabelKind::Category
                        && a.status != Status::Rejected
                        && (a.name == catalog::TYPE_MANGA || a.name == catalog::TYPE_ILLUSTRATION)
                });
                let clear = |score, bbox| !drawn && catalog::is_groupable_face(score, bbox);
                self.inspector.faces = cat.asset_faces(asset).unwrap_or_default();
                self.inspector.faces.retain(|f| clear(f.score, f.bbox));
                self.inspector.vision_done = cat.has_vision_done(asset).unwrap_or(false);
                let details = cat.asset_face_details(asset).unwrap_or_default();
                let mut face_details = Vec::with_capacity(details.len());
                for d in details.into_iter().filter(|d| clear(d.score, d.bbox)) {
                    let name = d.person_id.and_then(|pid| {
                        self.persons
                            .iter()
                            .find(|p| p.id == pid)
                            .and_then(|p| p.name.clone())
                    });
                    face_details.push((d, name));
                }
                self.inspector.face_details = face_details;
            }
            _ => {
                self.inspector.asset_work = None;
                self.inspector.covers.clear();
                self.inspector.ocr_lines.clear();
                self.inspector.ocr_done = false;
                self.inspector.faces.clear();
                self.inspector.vision_done = false;
                self.inspector.face_details.clear();
            }
        }
    }

    fn foreground_heavy_task(&self) -> Option<&'static str> {
        if self.undo_worker.is_some() || self.export_wizard.as_ref().is_some_and(|w| w.is_moving())
        {
            Some("el movimiento de fotos")
        } else if self.works_worker.is_some() {
            Some("la búsqueda de obras")
        } else if self.tags_apply_worker.is_some() {
            Some("la aplicación de etiquetas")
        } else if self.near_dupes_worker.is_some() {
            Some("la búsqueda de parecidos")
        } else if self.face_group_worker.is_some() {
            Some("la agrupación de rostros")
        } else {
            None
        }
    }

    fn sync_engine_yield(&self) {
        if let Some(engine) = &self.review_engine {
            engine.set_yielded(self.foreground_heavy_task().is_some());
        }
    }

    /// Lists the registered folders again when it is time to. The engine does it on its own thread.
    fn recheck_folders(&mut self, ctx: &egui::Context) {
        let Some(engine) = &self.review_engine else {
            return;
        };
        let now = Instant::now();
        if self.folder_recheck.due(ctx.input(|i| i.focused), now) {
            engine.request_index();
        }
        ctx.request_repaint_after(self.folder_recheck.remaining(now));
    }

    /// «Recargar»: lists the folders again; the gallery reloads when that ends.
    fn reload_folders(&mut self) {
        let Some(engine) = &self.review_engine else {
            self.reload_catalog();
            return;
        };
        if engine.is_paused() {
            self.status_notice =
                Some("La revisión está en pausa: las carpetas se revisarán al reanudarla".into());
        }
        engine.request_index();
    }

    fn refresh_can_undo(&mut self) {
        self.can_undo_move = self
            .catalog
            .as_ref()
            .is_some_and(|c| c.last_undoable_batch().is_ok_and(|b| b.is_some()));
    }

    fn start_undo(&mut self, ctx: &egui::Context) {
        if self.undo_worker.is_some() {
            return;
        }
        let roots = self.settings.valid_roots();
        match settings::db_dest_path(&self.data_dir, &roots) {
            Ok(db) => {
                let ctx = ctx.clone();
                let notify: Notify = Arc::new(move || ctx.request_repaint());
                self.undo_worker = Some(crate::export_flow::UndoWorker::start(
                    db,
                    roots,
                    self.data_dir.clone(),
                    Some(notify),
                ));
                self.status_notice = Some("Deshaciendo el último movimiento…".into());
                self.sync_engine_yield();
            }
            Err(e) => self.banner_error = Some(e.to_string()),
        }
    }

    fn poll_undo(&mut self, ctx: &egui::Context) {
        let Some(result) = self.undo_worker.as_ref().and_then(|w| w.try_recv()) else {
            return;
        };
        self.undo_worker = None;
        self.sync_engine_yield();
        match result {
            Ok(Some(report)) => {
                let (done, problems) = crate::export_flow::undo_messages(&report);
                self.status_notice = Some(done);
                if problems.is_some() {
                    self.banner_error = problems;
                }
            }
            Ok(None) => self.status_notice = Some("No hay movimientos que deshacer".into()),
            Err(e) => {
                self.status_notice = None;
                self.banner_error = Some(format!("No se pudo deshacer: {e}"));
            }
        }
        // Even a failed undo may have brought some files back.
        self.reload_catalog();
        self.refresh_can_undo();
        ctx.request_repaint();
    }

    /// Flags in the catalog the photos found to be gone, and says whether it flagged any.
    fn mark_gone_photos(&mut self) -> bool {
        if let Some(wizard) = &mut self.export_wizard {
            let gone = wizard.take_gone();
            self.gone_photos
                .extend(gone.into_iter().map(|path| (path, None)));
        }
        // An index holds the database while it reads new files: wait rather than freeze the window.
        if matches!(self.engine_snapshot.phase, EnginePhase::Indexing { .. }) {
            return false;
        }
        let Some(cat) = &self.catalog else {
            return false;
        };
        let mut marked = false;
        for (path, thumb) in self.gone_photos.drain(..) {
            // A failure here is not reported: the next check of the folders flags the photo.
            if cat.mark_location_missing(&path).unwrap_or(false) {
                marked = true;
                // Another copy of the same photo may still give a thumbnail.
                if let Some(sha256) = thumb {
                    self.thumb_failed.remove(&sha256);
                }
            }
        }
        marked
    }

    fn poll_workers(&mut self, ctx: &egui::Context) {
        let mut finished_works = false;
        let mut finished_tags_apply = false;
        let mut finished_rank = false;
        let mut finished_near_dupes = false;
        let mut finished_face_group = false;
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
                        restored,
                        errors: _,
                    } => {
                        // The folders are checked often: with nothing new, nothing is reloaded.
                        if new + changed + missing + restored > 0 {
                            needs_full_reload = true;
                            let mut notice = format!(
                                "Cambios detectados: {}, {}, {}",
                                count_phrase(new, "nueva", "nuevas"),
                                count_phrase(changed, "actualizada", "actualizadas"),
                                count_phrase(missing, "ausente", "ausentes"),
                            );
                            if restored > 0 {
                                notice.push_str(", ");
                                notice.push_str(&count_phrase(
                                    restored,
                                    "recuperada",
                                    "recuperadas",
                                ));
                            }
                            self.status_notice = Some(notice);
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

        if let Some(worker) = &self.near_dupes_worker {
            while let Some(ev) = worker.try_recv() {
                self.duplicates_view.apply_event(&ev);
                match ev {
                    NearDupesEvent::Progress { .. } => {}
                    NearDupesEvent::Finished {
                        stopped,
                        group_count,
                        threshold,
                    } => {
                        finished_near_dupes = true;
                        if stopped {
                            self.status_notice = Some("Búsqueda de parecidos cancelada".into());
                        } else {
                            needs_full_reload = true;
                            self.status_notice = Some(format!(
                                "Búsqueda de parecidos completa: {} con umbral {threshold:.2}",
                                count_phrase(group_count, "grupo", "grupos"),
                            ));
                        }
                    }
                    NearDupesEvent::Failed(err) => {
                        finished_near_dupes = true;
                        self.banner_error = Some(format!("Error al buscar parecidos: {err}"));
                    }
                }
            }
        }

        if let Some(worker) = &self.face_group_worker {
            while let Some(ev) = worker.try_recv() {
                match ev {
                    FaceGroupEvent::Progress { done, total } => {
                        self.settings_state.face_grouping = Some((done, total));
                    }
                    FaceGroupEvent::Finished {
                        stopped,
                        total,
                        grouped_faces,
                        grouped_photos,
                        unreadable_photos,
                    } => {
                        finished_face_group = true;
                        needs_full_reload = true;
                        self.status_notice = Some(if total == 0 {
                            "Todos los rostros detectados ya tienen vector y grupo".into()
                        } else {
                            let prefix = if stopped {
                                "Agrupación cancelada"
                            } else {
                                "Agrupación completada"
                            };
                            let mut notice = format!(
                                "{prefix}: {} agrupados en {}",
                                count_phrase(grouped_faces, "rostro", "rostros"),
                                count_phrase(grouped_photos, "foto", "fotos"),
                            );
                            if unreadable_photos > 0 {
                                notice.push_str(&format!(
                                    " (sin poder leer: {})",
                                    count_phrase(unreadable_photos, "foto", "fotos")
                                ));
                            }
                            notice
                        });
                    }
                    FaceGroupEvent::Failed(err) => {
                        finished_face_group = true;
                        needs_full_reload = true;
                        self.banner_error = Some(format!("Error al agrupar rostros: {err}"));
                    }
                }
            }
        }

        if finished_face_group {
            self.stop_face_grouping();
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
        if finished_near_dupes {
            self.near_dupes_worker = None;
            self.sync_engine_yield();
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
            if let Some(path) = res.gone {
                self.gone_photos.push((path, Some(res.sha256.clone())));
            }
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
            if res.image.is_err() {
                self.gone_photos.push((res.path.clone(), None));
            }
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

        if self.mark_gone_photos() {
            self.reload_catalog();
            ctx.request_repaint();
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
            .set_title("Elegir carpeta con fotos")
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

    fn retry_all_failed(&mut self) {
        if let Some(cat) = &self.catalog {
            let _ = cat.clear_all_stage_errors_in_roots(&self.settings.library_roots);
        }
        if let Some(engine) = &self.review_engine {
            engine.retry_failed();
        } else if self.demo_mode {
            self.status_notice = Some("Modo demo: errores limpiados (sin cargar modelos)".into());
        }
        self.reload_catalog();
    }

    fn retry_single_asset(&mut self, asset: AssetId) {
        if let Some(cat) = &self.catalog {
            let _ = cat.clear_stage_errors_for_asset(asset);
        }
        if let Some(engine) = &self.review_engine {
            engine.retry_asset(asset);
        } else if self.demo_mode {
            self.status_notice =
                Some("Modo demo: foto puesta en cola para reintentar (sin cargar modelos)".into());
        }
        self.reload_catalog();
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

    fn start_face_grouping(&mut self, ctx: &egui::Context) {
        if self.face_group_worker.is_some() {
            return;
        }
        if self.demo_mode {
            self.status_notice = Some("Modo demo: rostros agrupados (sin cargar modelos)".into());
            self.reload_catalog();
            return;
        }
        if let Some(task) = self.foreground_heavy_task() {
            self.status_notice = Some(format!("Espera a que termine {task}"));
            return;
        }
        let Some(models_dir) = self.settings.vision_models_dir.clone() else {
            self.status_notice =
                Some("Falta la carpeta de modelos de visión; configúrala en Configuración".into());
            return;
        };
        let roots = self.settings.valid_roots();
        match settings::db_dest_path(&self.data_dir, &roots) {
            Ok(db) => {
                let ctx_clone = ctx.clone();
                let notify: Notify = Arc::new(move || ctx_clone.request_repaint());
                self.settings_state.face_grouping = Some((0, 0));
                self.face_group_worker = Some(FaceGroupWorker::start(
                    db,
                    models_dir,
                    self.settings.ocr_device,
                    Some(notify),
                ));
                self.sync_engine_yield();
            }
            Err(e) => self.banner_error = Some(e.to_string()),
        }
    }

    /// Drops the grouping worker: cancels it and waits for it to leave the catalog.
    fn stop_face_grouping(&mut self) {
        self.face_group_worker = None;
        self.settings_state.face_grouping = None;
        self.settings_state.face_grouping_cancelling = false;
        self.sync_engine_yield();
    }

    fn start_near_dupes(&mut self, ctx: &egui::Context, threshold: f32) {
        if self.near_dupes_worker.is_some() {
            return;
        }
        if let Some(task) = self.foreground_heavy_task() {
            self.status_notice = Some(format!("Espera a que termine {task}"));
            return;
        }
        let valid_roots = self.settings.valid_roots();
        match settings::db_dest_path(&self.data_dir, &valid_roots) {
            Ok(db) => {
                let ctx_clone = ctx.clone();
                let notify: Notify = Arc::new(move || ctx_clone.request_repaint());
                self.duplicates_view.running = Some((0, 0));
                self.near_dupes_worker = Some(NearDupesWorker::start(
                    db,
                    self.settings.library_roots.clone(),
                    threshold,
                    Some(notify),
                ));
                self.sync_engine_yield();
            }
            Err(e) => self.banner_error = Some(e.to_string()),
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
                self.works_worker = Some(WorksWorker::start_in_roots(
                    db,
                    Some(self.settings.library_roots.clone()),
                    Some(notify),
                ));
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
        if self.photo_menu.is_some()
            || self.label_modal.is_open()
            || self.export_wizard.is_some()
            || egui::Popup::is_any_open(ctx)
        {
            // While a dialog or menu is open, it owns keyboard keys.
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

    /// The «Copiar o mover» window, opened from the selection bar or the photo menu.
    fn draw_export_window(&mut self, ctx: &egui::Context, palette: &theme::Palette) {
        let moving_before = self.export_wizard.as_ref().is_some_and(|w| w.is_moving());
        let undo_requested = views::export::draw_window(
            ctx,
            palette,
            &mut self.export_wizard,
            &self.data_dir,
            &mut self.settings,
            &mut self.banner_error,
        );
        let moving = self.export_wizard.as_ref().is_some_and(|w| w.is_moving());
        if moving != moving_before {
            // The background review waits: it must not read files while they change place.
            self.sync_engine_yield();
        }
        if self
            .export_wizard
            .as_mut()
            .is_some_and(|w| w.take_finished_move())
        {
            self.reload_catalog();
            self.refresh_can_undo();
            ctx.request_repaint();
        }
        if undo_requested {
            self.start_undo(ctx);
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
            if wizard
                .request_preview(&roots, &self.data_dir, Some(notify))
                .is_err()
            {
                self.export_open_failed = true;
            }
        }
        self.export_wizard = Some(wizard);
    }
}

/// A question Windows asks while moving, such as before deleting for good, opens over this window.
fn own_system_dialogs(frame: &eframe::Frame) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    if let Ok(handle) = frame.window_handle()
        && let RawWindowHandle::Win32(window) = handle.as_raw()
    {
        fsguard::set_dialog_owner(window.hwnd.get());
    }
}

impl eframe::App for PhotosortApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        own_system_dialogs(frame);
        theme::scroll_focus_into_view(&ctx);
        self.draw(ui);
        theme::paint_focus_ring(&ctx);
    }
}

impl PhotosortApp {
    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.recheck_folders(&ctx);
        self.poll_workers(&ctx);
        self.poll_undo(&ctx);
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
            self.duplicate_groups.len() + self.near_duplicate_groups.len(),
            self.works.len(),
            self.persons.len(),
            self.vision_rules.len(),
            &self.all_labels,
            &self.persons,
            &self.metadata_facets,
            &mut self.filter,
            &mut self.sidebar_state,
        );
        if let Some(action) = sidebar_out.label_action {
            self.label_modal.open(action, &self.all_labels);
        }
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
            self.stage_error_assets_count,
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
        if top_out.open_errors {
            self.active_view = ActiveView::Errors;
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
        if top_out.retry_failed {
            self.retry_all_failed();
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

                let work_banner = self.work_order_view.as_ref().map(|(_, name)| name.as_str());

                let mut naming_outcome = None;
                if let Some((_, faces)) = &self.person_cards.naming {
                    let thumbs = &self.person_cards.thumbs;
                    self.request_face_thumbs(faces.iter().filter_map(|f| thumbs.get(&f.asset_id)));
                }

                let top_banner = if let (Some(cat), Some((candidate, faces))) =
                    (&self.catalog, &self.person_cards.naming)
                {
                    let pv = &mut self.persons_view;
                    let tt = &self.thumb_textures;
                    let thumbs = &self.person_cards.thumbs;
                    let out_ref = &mut naming_outcome;
                    Some(move |ui: &mut egui::Ui| {
                        *out_ref = Some(views::persons::draw_naming_card(
                            ui, palette, candidate, faces, pv, cat, tt, thumbs,
                        ));
                    })
                } else {
                    None
                };

                let gal_out = views::gallery::draw(
                    ui,
                    palette,
                    &self.entries,
                    &mut self.selection,
                    &mut self.filter.sort,
                    self.similarity_view.as_ref(),
                    work_banner,
                    top_banner,
                    &self.reviewed_assets,
                    &self.pending_suggestions_assets,
                    self.counts.locations > 0,
                    &self.thumb_pool,
                    &self.thumb_textures,
                    &self.thumb_failed,
                    views::gallery::FolderActions {
                        reloading: matches!(
                            self.engine_snapshot.phase,
                            EnginePhase::Indexing { .. }
                        ),
                        // With the window open, its summary is the one that offers to undo.
                        can_undo: self.can_undo_move
                            && self.undo_worker.is_none()
                            && self.export_wizard.is_none(),
                    },
                );
                if gal_out.reload {
                    self.reload_folders();
                }
                if gal_out.undo_move {
                    self.start_undo(&ctx);
                }
                if let Some(out) = naming_outcome {
                    if out.postponed.is_some() {
                        self.reload_person_cards();
                    }
                    let mut catalog_mutated = false;
                    if let Some(cat) = &mut self.catalog {
                        if let Some((pid, name)) = out.renamed {
                            match cat.rename_person(pid, Some(&name)) {
                                Ok(_) => catalog_mutated = true,
                                Err(e) => {
                                    self.banner_error =
                                        Some(format!("No se pudo nombrar la persona: {e}"))
                                }
                            }
                        }
                        if let Some((src, target)) = out.merged {
                            match cat.merge_persons(src, target) {
                                Ok(_) => catalog_mutated = true,
                                Err(e) => {
                                    self.banner_error =
                                        Some(format!("No se pudieron unir las personas: {e}"))
                                }
                            }
                        }
                        if let Some(pid) = out.ignored {
                            match cat.set_person_ignored(pid, true) {
                                Ok(_) => catalog_mutated = true,
                                Err(e) => {
                                    self.banner_error =
                                        Some(format!("No se pudo ignorar la persona: {e}"))
                                }
                            }
                        }
                    }
                    if catalog_mutated {
                        self.reload_catalog();
                    }
                }
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
                // The gallery opens the menu for a focused thumbnail; this covers any other focus.
                if self.photo_menu.is_none()
                    && self.selection.primary().is_some()
                    && !self.viewer_open
                    && !self.label_modal.is_open()
                    && self.export_wizard.is_none()
                    && !ctx.text_edit_focused()
                    && views::context_menu::menu_key_pressed(&ctx)
                {
                    let pos = self
                        .last_primary_cell_pos
                        .unwrap_or_else(|| ctx.content_rect().center());
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
                    if let Some(asset) = insp_out.accept_work {
                        match cat.accept_asset_work(asset) {
                            Ok(_) => catalog_mutated = true,
                            Err(e) => {
                                self.banner_error =
                                    Some(format!("No se pudo confirmar la obra: {e}"))
                            }
                        }
                    }
                    if let Some(asset) = insp_out.reject_work {
                        match cat.reject_asset_work(asset) {
                            Ok(_) => catalog_mutated = true,
                            Err(e) => {
                                self.banner_error =
                                    Some(format!("No se pudo descartar la obra: {e}"))
                            }
                        }
                    }
                    if let Some((asset, cover_idx)) = insp_out.accept_cover {
                        match cat.accept_asset_cover(asset, cover_idx) {
                            Ok(_) => catalog_mutated = true,
                            Err(e) => {
                                self.banner_error =
                                    Some(format!("No se pudo confirmar la portada: {e}"))
                            }
                        }
                    }
                    if let Some((asset, cover_idx)) = insp_out.reject_cover {
                        match cat.reject_asset_cover(asset, cover_idx) {
                            Ok(_) => catalog_mutated = true,
                            Err(e) => {
                                self.banner_error =
                                    Some(format!("No se pudo descartar la portada: {e}"))
                            }
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
                    if let Some((person_id, asset_id)) = insp_out.remove_person_from_asset {
                        match cat.remove_asset_from_person(asset_id, person_id) {
                            Ok(_) => catalog_mutated = true,
                            Err(e) => {
                                self.banner_error =
                                    Some(format!("No se pudo quitar la persona de la foto: {e}"))
                            }
                        }
                    }
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
                if insp_out.open_persons {
                    self.active_view = ActiveView::Persons;
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
                let other_heavy = if self.near_dupes_worker.is_some() {
                    None
                } else {
                    self.foreground_heavy_task()
                };
                let outcome = views::duplicates::draw(
                    ui,
                    palette,
                    &mut self.duplicates_view,
                    &self.duplicate_groups,
                    &self.near_duplicate_groups,
                    self.near_duplicate_meta,
                    self.vision_analyzed_count,
                    other_heavy,
                    &self.thumb_pool,
                    &self.thumb_textures,
                    &self.thumb_failed,
                );
                self.last_thumbs_ready = outcome.thumbs_ready;
                if let Some(err) = outcome.error {
                    self.banner_error = Some(err);
                }
                if outcome.cancel_near_search
                    && let Some(worker) = &self.near_dupes_worker
                {
                    worker.cancel();
                }
                if let Some(threshold) = outcome.start_near_search {
                    self.start_near_dupes(&ctx, threshold);
                }
                if let Some(group_idx) = outcome.open_near_group_in_gallery
                    && let Some(group) = self.near_duplicate_groups.get(group_idx).cloned()
                {
                    let scores: HashMap<AssetId, f32> = group
                        .entries
                        .iter()
                        .map(|e| (e.asset, group.similarity))
                        .collect();
                    self.work_order_view = None;
                    self.selection.reconcile(&self.entries, &group.entries);
                    self.entries = group.entries;
                    self.similarity_view = Some(SimilarityView {
                        title: format!("Parecidos — Grupo {}", group_idx + 1),
                        threshold: self.near_duplicate_meta.map(|m| m.threshold),
                        scores,
                    });
                    self.active_view = ActiveView::Gallery;
                    self.sync_inspector(true);
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
                    &self.settings.library_roots,
                    self.catalog.as_mut(),
                    self.works_worker.is_some(),
                    other_heavy,
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
                if let Some(work_id) = outcome.deleted {
                    self.filter.set_chip(work_id, None);
                    if self
                        .work_order_view
                        .as_ref()
                        .is_some_and(|(id, _)| *id == work_id)
                    {
                        self.work_order_view = None;
                    }
                }
                if let Some((folder, work_id)) = outcome.accept_group
                    && let Some(cat) = &self.catalog
                {
                    match cat.accept_proposed_work_group(&folder, work_id) {
                        Ok(n) => {
                            self.reload_catalog();
                            self.status_notice = Some(format!(
                                "Grupo aceptado: {} de pertenencia confirmadas.",
                                count_phrase(n, "página", "páginas")
                            ));
                        }
                        Err(e) => {
                            self.banner_error = Some(format!("No se pudo aceptar el grupo: {e}"));
                        }
                    }
                }
                if let Some((folder, work_id)) = outcome.dismiss_group
                    && let Some(cat) = &self.catalog
                {
                    match cat.dismiss_proposed_work_group(&folder, work_id) {
                        Ok(_) => {
                            self.reload_catalog();
                            self.status_notice = Some("Grupo propuesto descartado.".into());
                        }
                        Err(e) => {
                            self.banner_error = Some(format!("No se pudo descartar el grupo: {e}"));
                        }
                    }
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
                if let Some((work_id, work_name)) = outcome.open_belongs_in_gallery
                    && let Some(cat) = &self.catalog
                {
                    match cat.entries_belonging_to_work(work_id, Some(&self.settings.library_roots))
                    {
                        Ok(next) => {
                            self.similarity_view = None;
                            self.work_order_view = Some((work_id, work_name));
                            self.selection.reconcile(&self.entries, &next);
                            self.entries = next;
                            self.active_view = ActiveView::Gallery;
                            self.sync_inspector(true);
                        }
                        Err(e) => {
                            self.banner_error =
                                Some(format!("No se pudieron cargar las páginas de la obra: {e}"));
                        }
                    }
                }
            }
            ActiveView::Persons => {
                self.request_face_thumbs(self.person_cards.thumbs.values());
                let (outcome, catalog_mutated) = if let Some(cat) = &mut self.catalog {
                    let outcome = views::persons::draw(
                        ui,
                        palette,
                        &mut self.persons_view,
                        cat,
                        &self.persons,
                        &self.person_cards,
                        &self.thumb_textures,
                    );
                    let mut catalog_mutated = false;
                    if let Some((pid, name)) = &outcome.renamed {
                        match cat.rename_person(*pid, Some(name.as_str())) {
                            Ok(_) => catalog_mutated = true,
                            Err(e) => {
                                self.banner_error =
                                    Some(format!("No se pudo renombrar la persona: {e}"))
                            }
                        }
                    }
                    if let Some((src, target)) = outcome.merged {
                        match cat.merge_persons(src, target) {
                            Ok(_) => catalog_mutated = true,
                            Err(e) => {
                                self.banner_error =
                                    Some(format!("No se pudieron unir las personas: {e}"))
                            }
                        }
                    }
                    if let Some(ids) = &outcome.merged_many {
                        for src in &ids[1..] {
                            if let Err(e) = cat.merge_persons(*src, ids[0]) {
                                self.banner_error =
                                    Some(format!("No se pudieron unir las personas: {e}"));
                                break;
                            }
                            catalog_mutated = true;
                        }
                    }
                    if let Some((pid, hidden)) = outcome.toggled_hidden {
                        match cat.set_person_hidden(pid, hidden) {
                            Ok(_) => catalog_mutated = true,
                            Err(e) => {
                                self.banner_error =
                                    Some(format!("No se pudo cambiar el estado de la persona: {e}"))
                            }
                        }
                    }
                    if let Some(pid) = outcome.ignored {
                        match cat.set_person_ignored(pid, true) {
                            Ok(_) => catalog_mutated = true,
                            Err(e) => {
                                self.banner_error =
                                    Some(format!("No se pudo ignorar la persona: {e}"))
                            }
                        }
                    }
                    if let Some((pid, asset_id)) = outcome.removed_asset {
                        match cat.remove_asset_from_person(asset_id, pid) {
                            Ok(_) => catalog_mutated = true,
                            Err(e) => {
                                self.banner_error =
                                    Some(format!("No se pudo quitar la foto de la persona: {e}"))
                            }
                        }
                    }
                    (outcome, catalog_mutated)
                } else {
                    (views::persons::PersonsOutcome::default(), false)
                };

                if let Some(pid) = outcome.open_in_gallery {
                    self.filter.clear_filters();
                    self.filter.persons.insert(pid);
                    self.active_view = ActiveView::Gallery;
                    self.reload_entries();
                } else if catalog_mutated {
                    self.reload_catalog();
                } else if outcome.postponed.is_some() {
                    self.reload_person_cards();
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
                    &self.all_labels,
                    self.vision_analyzed_count,
                    self.catalog.as_mut(),
                    self.tags_apply_worker.is_some(),
                    self.vision_rank_worker.is_some(),
                    other_heavy,
                );
                if let Some(action) = outcome.label_action {
                    self.label_modal.open(action, &self.all_labels);
                }
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
                    self.stage_error_assets_count,
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
                if outcome.open_errors {
                    self.active_view = ActiveView::Errors;
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
                if outcome.cancel_face_grouping
                    && let Some(worker) = &self.face_group_worker
                {
                    worker.cancel();
                    self.settings_state.face_grouping_cancelling = true;
                }
                if outcome.clear_face_grouping {
                    // A running grouping would recreate the data being deleted.
                    self.stop_face_grouping();
                }
                if outcome.clear_face_grouping
                    && let Some(cat) = &mut self.catalog
                {
                    match cat.clear_face_grouping_data() {
                        Ok(()) => {
                            self.filter.persons.clear();
                            self.persons_view.postponed_persons.clear();
                            self.status_notice = Some(
                                "Se han borrado los datos de rostros, grupos y nombres".into(),
                            );
                        }
                        Err(e) => {
                            self.banner_error =
                                Some(format!("No se pudieron borrar los datos de rostros: {e}"));
                        }
                    }
                    // Also on failure: the stopped grouping may have changed the catalog.
                    self.reload_catalog();
                }
                if outcome.start_face_grouping {
                    self.start_face_grouping(&ctx);
                }
                if outcome.settings_changed {
                    self.save_settings_and_sync_engine();
                    self.reload_catalog();
                }
            }
            ActiveView::Errors => {
                let outcome = views::errors::draw(
                    ui,
                    palette,
                    &self.stage_errors,
                    &self.thumb_pool,
                    &self.thumb_textures,
                    &self.thumb_failed,
                );
                self.last_thumbs_ready = outcome.thumbs_ready;
                if outcome.back {
                    self.active_view = ActiveView::Gallery;
                }
                if outcome.retry_all {
                    self.retry_all_failed();
                }
                if let Some(asset) = outcome.retry_asset {
                    self.retry_single_asset(asset);
                }
            }
        }

        // Label management modal (create subcategory, rename, move, merge, delete)
        let modal_out =
            self.label_modal
                .draw(&ctx, palette, self.catalog.as_mut(), &self.all_labels);
        if let Some(removed) = modal_out.removed_label {
            self.filter.set_chip(removed, None);
        }
        if let Some(notice) = modal_out.status_notice {
            self.status_notice = Some(notice);
        }
        if modal_out.catalog_mutated {
            self.reload_catalog();
        }

        self.draw_export_window(&ctx, palette);

        self.step_screenshot(&ctx);
    }
}
