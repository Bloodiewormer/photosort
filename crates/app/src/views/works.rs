use crate::model::count_phrase;
use crate::theme::{self, Palette};
use crate::worker::{ThumbPool, ThumbRequest};
use crate::works_worker::WorksEvent;
use catalog::{
    Catalog, LabelId, ProposedWorkGroup, WorkAlias, WorkProposal, WorkSummary, work_match_key,
};
use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Key, Layout, Margin, ProgressBar, RichText,
    ScrollArea, Sense, Stroke, TextureHandle, Vec2,
};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

const ROW_HEIGHT: f32 = 42.0;
const PROPOSALS_WINDOW_WIDTH: f32 = 540.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorksRunState {
    Idle,
    Running {
        done: usize,
        total: usize,
        matched_assets: usize,
        suggestions: usize,
    },
    Done {
        stopped: bool,
        processed: usize,
        matched_assets: usize,
        suggestions: usize,
    },
    Failed(String),
}

pub struct WorksViewState {
    pub filter: String,
    pub selected: Option<LabelId>,
    pub selected_aliases: Vec<WorkAlias>,
    pub new_work_input: String,
    pub new_alias_input: String,
    pub proposals_open: bool,
    pub proposals: Vec<WorkProposal>,
    /// Checked proposal titles (none checked by default).
    pub checked_proposals: BTreeSet<String>,
    pub run_state: WorksRunState,
    /// Work whose deletion is waiting for a second click.
    pub confirm_delete: Option<LabelId>,
    /// Work whose bulk ownership confirmation is waiting for a second click.
    pub confirm_accept_all: Option<LabelId>,
    pub proposed_groups: Vec<ProposedWorkGroup>,
}

impl Default for WorksViewState {
    fn default() -> Self {
        Self {
            filter: String::new(),
            selected: None,
            selected_aliases: Vec::new(),
            new_work_input: String::new(),
            new_alias_input: String::new(),
            proposals_open: false,
            proposals: Vec::new(),
            checked_proposals: BTreeSet::new(),
            run_state: WorksRunState::Idle,
            confirm_delete: None,
            confirm_accept_all: None,
            proposed_groups: Vec::new(),
        }
    }
}

impl WorksViewState {
    pub fn apply_event(&mut self, ev: &WorksEvent) {
        match ev {
            WorksEvent::Progress {
                done,
                total,
                matched_assets,
                suggestions,
            } => {
                self.run_state = WorksRunState::Running {
                    done: *done,
                    total: *total,
                    matched_assets: *matched_assets,
                    suggestions: *suggestions,
                };
            }
            WorksEvent::Finished {
                stopped,
                processed,
                matched_assets,
                suggestions,
            } => {
                self.run_state = WorksRunState::Done {
                    stopped: *stopped,
                    processed: *processed,
                    matched_assets: *matched_assets,
                    suggestions: *suggestions,
                };
            }
            WorksEvent::Failed(msg) => {
                self.run_state = WorksRunState::Failed(msg.clone());
            }
        }
    }

    pub fn sync_selection(&mut self, works: &[WorkSummary], cat: Option<&Catalog>) {
        let valid = self
            .selected
            .filter(|id| works.iter().any(|w| w.label == *id));
        self.selected = valid.or_else(|| works.first().map(|w| w.label));
        self.reload_aliases(cat);
    }

    pub fn reload_aliases(&mut self, cat: Option<&Catalog>) {
        self.selected_aliases = match (self.selected, cat) {
            (Some(id), Some(c)) => c.work_aliases(id).unwrap_or_default(),
            _ => Vec::new(),
        };
    }
}

#[derive(Default)]
pub struct WorksViewOutcome {
    pub open_in_gallery: Option<LabelId>,
    pub open_belongs_in_gallery: Option<(LabelId, String)>,
    pub start_match: bool,
    pub cancel_match: bool,
    pub catalog_mutated: bool,
    /// A work that no longer exists, so that no filter keeps pointing at it.
    pub deleted: Option<LabelId>,
    pub status_notice: Option<String>,
    pub error: Option<String>,
    pub accept_group: Option<(PathBuf, LabelId)>,
    pub dismiss_group: Option<(PathBuf, LabelId)>,
    pub thumbs_ready: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut WorksViewState,
    works: &[WorkSummary],
    roots: &[PathBuf],
    cat: Option<&mut Catalog>,
    worker_running: bool,
    other_heavy_task: Option<&str>,
    thumb_pool: &ThumbPool,
    thumb_textures: &HashMap<String, TextureHandle>,
    thumb_failed: &HashSet<String>,
) -> WorksViewOutcome {
    let mut outcome = WorksViewOutcome::default();
    let mut visible_hashes: HashSet<String> = HashSet::new();
    let mut to_request: Vec<ThumbRequest> = Vec::new();

    let mut create_work_name: Option<String> = None;
    let mut add_alias: Option<(LabelId, String)> = None;
    let mut remove_alias: Option<(LabelId, String)> = None;
    let mut open_proposals = false;
    let mut create_proposals: Vec<String> = Vec::new();
    let mut select_changed = false;
    let mut delete_work: Option<LabelId> = None;
    let mut accept_all_work: Option<LabelId> = None;
    // A running worker would write against the deleted work.
    let busy = worker_running || other_heavy_task.is_some();

    // Right panel: selected work details & aliases.
    egui::Panel::right("works_detail")
        .default_size(360.0)
        .size_range(280.0..=500.0)
        .resizable(true)
        .frame(
            Frame::new()
                .fill(palette.bg_panel.color32())
                .inner_margin(Margin::same(theme::SPACE_MD as i8))
                .stroke(Stroke::new(1.0, palette.border.color32())),
        )
        .show(ui, |ui| {
            let selected_work = state
                .selected
                .and_then(|id| works.iter().find(|w| w.label == id));

            let Some(work) = selected_work else {
                ui.vertical_centered(|ui| {
                    ui.add_space(theme::SPACE_XL * 2.0);
                    ui.label(
                        RichText::new("Selecciona una obra para gestionar sus alias.")
                            .color(palette.text_secondary.color32()),
                    );
                });
                return;
            };

            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                ui.label(RichText::new(&work.name).size(theme::FONT_HEADING).strong());
                ui.add_space(theme::SPACE_XS);
                ui.label(
                    RichText::new(format!(
                        "Pertenecen: {} · Menciones: {} · {}",
                        work.belongs_to,
                        work.images,
                        count_phrase(work.aliases, "alias adicional", "alias adicionales"),
                    ))
                    .size(theme::FONT_SMALL)
                    .color(palette.text_secondary.color32()),
                );
                ui.add_space(theme::SPACE_SM);

                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            work.belongs_to > 0,
                            egui::Button::new("Ver páginas por capítulo"),
                        )
                        .on_hover_text(
                            "Mostrar en la galería las imágenes que pertenecen a esta obra ordenadas por capítulo y página",
                        )
                        .clicked()
                    {
                        outcome.open_belongs_in_gallery = Some((work.label, work.name.clone()));
                    }
                    if ui
                        .button("Ver menciones")
                        .on_hover_text("Mostrar en la galería las imágenes que mencionan esta obra")
                        .clicked()
                    {
                        outcome.open_in_gallery = Some(work.label);
                    }
                });

                if work.belongs_to_suggested > 0 {
                    ui.add_space(theme::SPACE_SM);
                    if state.confirm_accept_all == Some(work.label) {
                        ui.label(
                            RichText::new(format!(
                                "¿Confirmar las {} pendientes de pertenencia a «{}»?",
                                count_phrase(
                                    work.belongs_to_suggested,
                                    "sugerencia",
                                    "sugerencias",
                                ),
                                work.name
                            ))
                            .size(theme::FONT_SMALL),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    !busy,
                                    theme::primary_button(palette, "Sí, confirmar todas"),
                                )
                                .clicked()
                            {
                                accept_all_work = Some(work.label);
                            }
                            if ui.button("Cancelar").clicked() {
                                state.confirm_accept_all = None;
                            }
                        });
                    } else if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(format!(
                                "Confirmar sugerencias ({})",
                                work.belongs_to_suggested
                            )),
                        )
                        .on_hover_text(
                            "Confirma todas las sugerencias pendientes de pertenencia de esta obra",
                        )
                        .clicked()
                    {
                        state.confirm_accept_all = Some(work.label);
                    }
                }

                ui.add_space(theme::SPACE_MD);
                ui.separator();
                ui.add_space(theme::SPACE_SM);

                ui.label(RichText::new("Alias de la obra").strong());
                ui.add_space(theme::SPACE_XS);
                ui.label(
                    RichText::new(
                        "El nombre principal de la obra siempre se usa automáticamente al buscar coincidencias en el texto OCR.",
                    )
                    .size(theme::FONT_SMALL)
                    .color(palette.text_secondary.color32()),
                );
                ui.add_space(theme::SPACE_SM);

                // Add alias row
                ui.horizontal(|ui| {
                    let edit = theme::named(
                        ui.add(
                            egui::TextEdit::singleline(&mut state.new_alias_input)
                                .desired_width((ui.available_width() - 108.0).max(120.0))
                                .hint_text(theme::hint_text(palette, "Añadir alias…")),
                        ),
                        "Añadir alias",
                    );
                    let submit = ui.button("Añadir alias").clicked()
                        || (edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                    if submit {
                        let trimmed = state.new_alias_input.trim().to_string();
                        if !trimmed.is_empty() {
                            add_alias = Some((work.label, trimmed));
                            state.new_alias_input.clear();
                        }
                    }
                });

                ui.add_space(theme::SPACE_SM);

                if state.selected_aliases.is_empty() {
                    ui.label(
                        RichText::new("Sin alias adicionales todavía.")
                            .size(theme::FONT_SMALL)
                            .color(palette.text_secondary.color32()),
                    );
                } else {
                    for item in &state.selected_aliases {
                        Frame::new()
                            .fill(palette.bg_card.color32())
                            .stroke(Stroke::new(1.0, palette.border.color32()))
                            .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                            .inner_margin(Margin::symmetric(
                                theme::SPACE_SM as i8,
                                theme::SPACE_XS as i8,
                            ))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.button("Quitar").clicked() {
                                            remove_alias = Some((work.label, item.key.clone()));
                                        }
                                        ui.with_layout(
                                            Layout::left_to_right(Align::Center),
                                            |ui| {
                                                ui.add(egui::Label::new(&item.alias).truncate());
                                            },
                                        );
                                    });
                                });
                            });
                        ui.add_space(theme::SPACE_XS);
                    }
                }

                ui.add_space(theme::SPACE_MD);
                ui.separator();
                ui.add_space(theme::SPACE_SM);
                if state.confirm_delete == Some(work.label) {
                    ui.label(format!(
                        "¿Eliminar «{}»? Se borran sus alias y se quita de todas las imágenes, \
                         también donde la habías aceptado o rechazado. No se puede deshacer. \
                         Las fotos no cambian.",
                        work.name
                    ));
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("Sí, eliminar"))
                            .clicked()
                        {
                            delete_work = Some(work.label);
                        }
                        if ui.button("Conservar").clicked() {
                            state.confirm_delete = None;
                        }
                    });
                } else if ui
                    .add_enabled(!busy, egui::Button::new("Eliminar obra…"))
                    .on_disabled_hover_text("Espera a que termine la tarea en curso.")
                    .clicked()
                {
                    state.confirm_delete = Some(work.label);
                }
            });
        });

    // Central panel: toolbar, create work, filter, and virtualized works list.
    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(palette.bg_canvas.color32())
                .inner_margin(Margin::same(theme::SPACE_MD as i8)),
        )
        .show(ui, |ui| {
            // Header & primary actions
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(
                        "Obras ({})",
                        works.len()
                    ))
                    .size(theme::FONT_HEADING)
                    .strong(),
                );
                ui.add_space(theme::SPACE_SM);

                let can_propose = cat.is_some();
                if ui
                    .add_enabled(
                        can_propose,
                        egui::Button::new("Proponer desde nombres de archivo…"),
                    )
                    .clicked()
                {
                    open_proposals = true;
                }

                if worker_running {
                    if ui.button("Cancelar búsqueda").clicked() {
                        outcome.cancel_match = true;
                    }
                } else {
                    let can_match = cat.is_some() && other_heavy_task.is_none();
                    let disabled_tip = if let Some(task) = other_heavy_task {
                        format!("Espera a que termine {task} antes de buscar obras.")
                    } else {
                        "No hay catálogo abierto.".into()
                    };
                    if ui
                        .add_enabled(
                            can_match,
                            egui::Button::new("Buscar obras en el texto leído"),
                        )
                        .on_disabled_hover_text(disabled_tip)
                        .clicked()
                    {
                        outcome.start_match = true;
                    }
                }
            });

            // Match progress / status banner inside the Works view
            match &state.run_state {
                WorksRunState::Idle => {}
                WorksRunState::Running {
                    done,
                    total,
                    matched_assets,
                    suggestions,
                } => {
                    ui.add_space(theme::SPACE_SM);
                    let frac = if *total == 0 {
                        0.0
                    } else {
                        *done as f32 / *total as f32
                    };
                    ui.add(ProgressBar::new(frac).desired_width(ui.available_width()));
                    ui.label(
                        RichText::new(format!(
                            "Buscando obras: {done} de {total} · {} con coincidencias · {}",
                            count_phrase(*matched_assets, "imagen", "imágenes"),
                            count_phrase(*suggestions, "sugerencia", "sugerencias"),
                        ))
                        .size(theme::FONT_SMALL)
                        .color(palette.text_secondary.color32()),
                    );
                }
                WorksRunState::Done {
                    stopped,
                    processed,
                    matched_assets,
                    suggestions,
                } => {
                    ui.add_space(theme::SPACE_SM);
                    let prefix = if *stopped {
                        "Búsqueda detenida"
                    } else {
                        "Búsqueda completada"
                    };
                    Frame::new()
                        .fill(palette.chip_require_bg.color32())
                        .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                        .inner_margin(Margin::symmetric(
                            theme::SPACE_SM as i8,
                            theme::SPACE_XS as i8,
                        ))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{prefix}: {} revisadas, {} con coincidencias ({}).",
                                    count_phrase(*processed, "imagen", "imágenes"),
                                    count_phrase(*matched_assets, "imagen", "imágenes"),
                                    count_phrase(*suggestions, "sugerencia", "sugerencias"),
                                ))
                                .size(theme::FONT_SMALL)
                                .color(palette.chip_require_fg.color32()),
                            );
                        });
                }
                WorksRunState::Failed(err) => {
                    ui.add_space(theme::SPACE_SM);
                    Frame::new()
                        .fill(palette.chip_exclude_bg.color32())
                        .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                        .inner_margin(Margin::symmetric(
                            theme::SPACE_SM as i8,
                            theme::SPACE_XS as i8,
                        ))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!("Error al buscar obras: {err}"))
                                    .size(theme::FONT_SMALL)
                                    .color(palette.chip_exclude_fg.color32()),
                            );
                        });
                }
            }

            // Proposed groups section
            if !state.proposed_groups.is_empty() {
                ui.add_space(theme::SPACE_MD);
                ui.label(
                    RichText::new(format!(
                        "Grupos propuestos ({})",
                        state.proposed_groups.len()
                    ))
                    .size(theme::FONT_HEADING)
                    .strong(),
                );
                ui.add_space(theme::SPACE_XS);
                ui.label(
                    RichText::new(
                        "Carpetas cuyas páginas pertenecen a una obra según el nombre o la mayoría detectada.",
                    )
                    .size(theme::FONT_SMALL)
                    .color(palette.text_secondary.color32()),
                );
                ui.add_space(theme::SPACE_SM);

                for group in &state.proposed_groups {
                    Frame::new()
                        .fill(palette.bg_card.color32())
                        .stroke(Stroke::new(1.0, palette.border.color32()))
                        .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                        .inner_margin(Margin::same(theme::SPACE_MD as i8))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let folder_name = group
                                    .folder
                                    .file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or_else(|| group.folder.to_str().unwrap_or(""));
                                let full_folder = group.folder.display().to_string();
                                ui.label(RichText::new(folder_name).strong())
                                    .on_hover_text(&full_folder);
                                ui.label(
                                    RichText::new("→")
                                        .color(palette.text_secondary.color32()),
                                );
                                ui.label(
                                    RichText::new(&group.work_name)
                                        .color(palette.accent.color32())
                                        .strong(),
                                );

                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .add_enabled(
                                            !busy,
                                            egui::Button::new("Descartar grupo"),
                                        )
                                        .clicked()
                                    {
                                        outcome.dismiss_group =
                                            Some((group.folder.clone(), group.work));
                                    }
                                    if ui
                                        .add_enabled(
                                            !busy,
                                            theme::primary_button(palette, "Aceptar grupo"),
                                        )
                                        .clicked()
                                    {
                                        outcome.accept_group =
                                            Some((group.folder.clone(), group.work));
                                    }
                                });
                            });

                            let ch_part = if group.chapter.is_empty() {
                                String::new()
                            } else {
                                format!(" · Capítulo {}", group.chapter)
                            };
                            ui.label(
                                RichText::new(format!(
                                    "{}{ch_part}",
                                    count_phrase(group.page_count, "página", "páginas")
                                ))
                                .size(theme::FONT_SMALL)
                                .color(palette.text_secondary.color32()),
                            );

                            if !group.sample_entries.is_empty() {
                                ui.add_space(theme::SPACE_SM);
                                ui.horizontal(|ui| {
                                    for entry in &group.sample_entries {
                                        visible_hashes.insert(entry.sha256.clone());
                                        if !thumb_textures.contains_key(&entry.sha256)
                                            && !thumb_failed.contains(&entry.sha256)
                                        {
                                            to_request.push(ThumbRequest {
                                                source: entry.path.clone(),
                                                sha256: entry.sha256.clone(),
                                            });
                                        }
                                        let (rect, _) = ui.allocate_exact_size(
                                            Vec2::new(56.0, 56.0),
                                            Sense::hover(),
                                        );
                                        draw_thumb_box(
                                            ui,
                                            palette,
                                            rect,
                                            thumb_textures.get(&entry.sha256),
                                            thumb_failed.contains(&entry.sha256),
                                        );
                                    }
                                });
                            }
                        });
                    ui.add_space(theme::SPACE_SM);
                }
            }

            ui.add_space(theme::SPACE_MD);

            // Create new work + filter bar
            ui.horizontal(|ui| {
                let edit = theme::named(
                    ui.add(
                        egui::TextEdit::singleline(&mut state.new_work_input)
                            .desired_width(240.0)
                            .hint_text(theme::hint_text(palette, "Nueva obra o grupo…")),
                    ),
                    "Nueva obra o grupo",
                );
                let submit = ui.button("Crear obra").clicked()
                    || (edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                if submit {
                    let trimmed = state.new_work_input.trim().to_string();
                    if !trimmed.is_empty() {
                        create_work_name = Some(trimmed);
                        state.new_work_input.clear();
                    }
                }

                ui.separator();

                theme::named(
                    ui.add(
                        egui::TextEdit::singleline(&mut state.filter)
                            .desired_width((ui.available_width() - 80.0).max(160.0))
                            .hint_text(theme::hint_text(palette, "Filtrar obras…")),
                    ),
                    "Filtrar obras",
                );
                if !state.filter.is_empty() && ui.button("Limpiar").clicked() {
                    state.filter.clear();
                }
            });

            ui.add_space(theme::SPACE_SM);

            let filter_key = work_match_key(&state.filter);
            let filter_lower = state.filter.trim().to_lowercase();
            let filtered: Vec<&WorkSummary> = if filter_lower.is_empty() {
                works.iter().collect()
            } else {
                works
                    .iter()
                    .filter(|w| {
                        w.name.to_lowercase().contains(&filter_lower)
                            || (!filter_key.is_empty() && work_match_key(&w.name).contains(&filter_key))
                    })
                    .collect()
            };

            if filtered.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(theme::SPACE_XL * 2.0);
                    let msg = if works.is_empty() {
                        "No hay obras registradas todavía. Crea una obra, propón desde los nombres de archivo o selecciona líneas de texto en el inspector."
                    } else {
                        "Ninguna obra coincide con el filtro actual."
                    };
                    ui.label(RichText::new(msg).color(palette.text_secondary.color32()));
                });
                return;
            }

            // Virtualized list of works (scales cleanly to hundreds/thousands of works).
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(ui, ROW_HEIGHT, filtered.len(), |ui, row_range| {
                    for row in row_range {
                        let work = filtered[row];
                        let selected = state.selected == Some(work.label);
                        let bg = if selected {
                            palette.accent_bg.color32()
                        } else {
                            palette.bg_card.color32()
                        };
                        let stroke = if selected {
                            Stroke::new(theme::FOCUS_STROKE_WIDTH, palette.accent.color32())
                        } else {
                            Stroke::new(1.0, palette.border.color32())
                        };

                        Frame::new()
                            .fill(bg)
                            .stroke(stroke)
                            .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                            .inner_margin(Margin::symmetric(
                                theme::SPACE_SM as i8,
                                theme::SPACE_XS as i8,
                            ))
                            .show(ui, |ui| {
                                let inner_h = (ROW_HEIGHT - theme::SPACE_SM * 2.0).max(24.0);
                                ui.allocate_ui_with_layout(
                                    Vec2::new(ui.available_width(), inner_h),
                                    Layout::right_to_left(Align::Center),
                                    |ui| {
                                        let mentions_btn =
                                            count_phrase(work.images, "mención", "menciones");
                                        if ui
                                            .button(mentions_btn)
                                            .on_hover_text(
                                                "Mostrar en la galería las imágenes que mencionan esta obra",
                                            )
                                            .clicked()
                                        {
                                            outcome.open_in_gallery = Some(work.label);
                                        }
                                        let belongs_btn =
                                            count_phrase(work.belongs_to, "página", "páginas");
                                        if ui
                                            .add_enabled(
                                                work.belongs_to > 0,
                                                egui::Button::new(belongs_btn),
                                            )
                                            .on_hover_text(
                                                "Mostrar las imágenes que pertenecen a esta obra ordenadas por capítulo y página",
                                            )
                                            .clicked()
                                        {
                                            outcome.open_belongs_in_gallery =
                                                Some((work.label, work.name.clone()));
                                        }
                                        if work.aliases > 0 {
                                            ui.label(
                                                RichText::new(format!("{} alias", work.aliases))
                                                    .size(theme::FONT_SMALL)
                                                    .color(palette.text_secondary.color32()),
                                            );
                                        }
                                        ui.with_layout(
                                            Layout::left_to_right(Align::Center),
                                            |ui| {
                                                let resp = ui.add(
                                                    egui::Button::new(
                                                        RichText::new(&work.name)
                                                            .color(palette.text_primary.color32())
                                                            .strong(),
                                                    )
                                                    .frame(false)
                                                    .truncate(),
                                                );
                                                if resp.clicked() {
                                                    state.selected = Some(work.label);
                                                    select_changed = true;
                                                }
                                            },
                                        );
                                    },
                                );
                            });
                        ui.add_space(theme::SPACE_XS);
                    }
                });
        });

    // Proposals modal dialog
    if state.proposals_open {
        let ctx = ui.ctx();
        let mut close_requested = false;
        if !ctx.egui_wants_keyboard_input()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
        {
            close_requested = true;
        }

        let mut open_flag = true;
        egui::Window::new("Proponer obras desde nombres de archivo")
            .collapsible(false)
            .resizable(false)
            .fade_in(false)
            .frame(Frame::window(&ctx.style_of(ctx.theme())).fill(palette.bg_panel.color32()))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .min_width(PROPOSALS_WINDOW_WIDTH)
            .max_width(PROPOSALS_WINDOW_WIDTH)
            .default_width(PROPOSALS_WINDOW_WIDTH)
            .open(&mut open_flag)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "Títulos detectados en nombres de archivo con marcador de capítulo (Capítulo, Chapter, Cap., Ch., Episodio, Episode, Ep.). Crear obras solo las añade a la lista; nunca se asignan por el nombre del archivo.",
                    )
                    .size(theme::FONT_SMALL)
                    .color(palette.text_secondary.color32()),
                );
                ui.add_space(theme::SPACE_SM);

                if state.proposals.is_empty() {
                    ui.label(
                        RichText::new("No se encontraron títulos nuevos en los nombres de archivo.")
                            .color(palette.text_secondary.color32()),
                    );
                } else {
                    ui.horizontal(|ui| {
                        if ui.button("Seleccionar todo").clicked() {
                            state.checked_proposals = state
                                .proposals
                                .iter()
                                .map(|p| p.title.clone())
                                .collect();
                        }
                        if ui.button("Ninguna").clicked() {
                            state.checked_proposals.clear();
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} de {} marcadas",
                                    state.checked_proposals.len(),
                                    state.proposals.len()
                                ))
                                .size(theme::FONT_SMALL)
                                .color(palette.text_secondary.color32()),
                            );
                        });
                    });
                    ui.add_space(theme::SPACE_XS);

                    Frame::new()
                        .fill(palette.bg_card.color32())
                        .stroke(Stroke::new(1.0, palette.border.color32()))
                        .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                        .inner_margin(Margin::same(theme::SPACE_SM as i8))
                        .show(ui, |ui| {
                            ScrollArea::vertical()
                                .max_height(260.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    for prop in &state.proposals {
                                        let mut checked =
                                            state.checked_proposals.contains(&prop.title);
                                        let label = format!(
                                            "{} ({})",
                                            prop.title,
                                            count_phrase(prop.files, "archivo", "archivos")
                                        );
                                        if ui.checkbox(&mut checked, label).changed() {
                                            if checked {
                                                state.checked_proposals.insert(prop.title.clone());
                                            } else {
                                                state.checked_proposals.remove(&prop.title);
                                            }
                                        }
                                    }
                                });
                        });
                }

                ui.add_space(theme::SPACE_MD);
                ui.separator();
                ui.add_space(theme::SPACE_SM);

                ui.horizontal(|ui| {
                    if ui.button("Cerrar").clicked() {
                        close_requested = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let checked_count = state.checked_proposals.len();
                        let label = if checked_count > 0 {
                            format!("Crear seleccionadas ({checked_count})")
                        } else {
                            "Crear seleccionadas".to_string()
                        };
                        if ui
                            .add_enabled(checked_count > 0, egui::Button::new(label))
                            .clicked()
                        {
                            create_proposals = state
                                .proposals
                                .iter()
                                .filter(|p| state.checked_proposals.contains(&p.title))
                                .map(|p| p.title.clone())
                                .collect();
                            close_requested = true;
                        }
                    });
                });
            });

        if !open_flag || close_requested {
            state.proposals_open = false;
        }
    }

    // Apply catalog mutations
    if let Some(c) = cat {
        if open_proposals {
            match c.propose_works_from_filenames() {
                Ok(props) => {
                    state.proposals = props;
                    state.checked_proposals.clear(); // None checked by default
                    state.proposals_open = true;
                }
                Err(e) => {
                    outcome.error = Some(format!("No se pudieron proponer obras: {e}"));
                }
            }
        }
        if let Some(name) = create_work_name {
            match c.create_work(&name) {
                Ok(id) => {
                    state.selected = Some(id);
                    outcome.catalog_mutated = true;
                    outcome.status_notice = Some(
                        "Obra creada. Pulsa «Buscar obras en el texto leído» para encontrarla."
                            .into(),
                    );
                }
                Err(e) => {
                    outcome.error = Some(format!("No se pudo crear la obra: {e}"));
                }
            }
        }
        if !create_proposals.is_empty() {
            let mut created = 0usize;
            for title in &create_proposals {
                match c.create_work(title) {
                    Ok(id) => {
                        created += 1;
                        if state.selected.is_none() {
                            state.selected = Some(id);
                        }
                    }
                    Err(e) => {
                        outcome.error = Some(format!("No se pudo crear una obra: {e}"));
                        break;
                    }
                }
            }
            if created > 0 {
                outcome.catalog_mutated = true;
                outcome.status_notice = Some(format!(
                    "Se añadieron {} a la lista.",
                    count_phrase(created, "obra", "obras")
                ));
            }
        }
        if let Some(work_id) = accept_all_work {
            state.confirm_accept_all = None;
            match c.accept_all_suggested_work_ownership(work_id, Some(roots)) {
                Ok(n) => {
                    outcome.catalog_mutated = true;
                    outcome.status_notice = Some(format!(
                        "Se confirmaron {} de pertenencia.",
                        count_phrase(n, "sugerencia", "sugerencias")
                    ));
                }
                Err(e) => {
                    outcome.error = Some(format!("No se pudieron confirmar las sugerencias: {e}"));
                }
            }
        }
        if let Some(work_id) = delete_work {
            state.confirm_delete = None;
            match c.delete_work(work_id) {
                Ok(_) => {
                    state.selected = None;
                    outcome.deleted = Some(work_id);
                    outcome.catalog_mutated = true;
                    outcome.status_notice = Some("Obra eliminada del catálogo.".into());
                }
                Err(e) => {
                    outcome.error = Some(format!("No se pudo eliminar la obra: {e}"));
                }
            }
        }
        if let Some((work_id, alias)) = add_alias {
            match c.add_work_alias(work_id, &alias) {
                Ok(true) => {
                    outcome.catalog_mutated = true;
                    outcome.status_notice = Some(
                        "Alias añadido. Pulsa «Buscar obras en el texto leído» para aplicarlo."
                            .into(),
                    );
                }
                Ok(false) => {
                    outcome.status_notice = Some(
                        "Ese alias ya existe o tiene menos de 2 caracteres alfanuméricos.".into(),
                    );
                }
                Err(e) => {
                    outcome.error = Some(format!("No se pudo añadir el alias: {e}"));
                }
            }
        }
        if let Some((work_id, key)) = remove_alias {
            match c.remove_work_alias(work_id, &key) {
                Ok(_) => {
                    outcome.catalog_mutated = true;
                    outcome.status_notice = Some("Alias eliminado.".into());
                }
                Err(e) => {
                    outcome.error = Some(format!("No se pudo quitar el alias: {e}"));
                }
            }
        }
        if select_changed || outcome.catalog_mutated {
            state.reload_aliases(Some(c));
        }
    }

    outcome.thumbs_ready =
        visible_hashes.is_empty() || (!visible_hashes.is_empty() && to_request.is_empty());
    thumb_pool.set_visible(&visible_hashes);
    for req in to_request {
        thumb_pool.request(req);
    }

    outcome
}

fn draw_thumb_box(
    ui: &egui::Ui,
    palette: &Palette,
    rect: egui::Rect,
    texture: Option<&TextureHandle>,
    failed: bool,
) {
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius::same(theme::RADIUS_SM),
        palette.bg_canvas.color32(),
    );
    if let Some(tex) = texture {
        let fit = fit_rect(rect, tex.size_vec2());
        painter.image(
            tex.id(),
            fit,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        let placeholder = if failed { "Sin miniatura" } else { "..." };
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            placeholder,
            egui::FontId::proportional(theme::FONT_SMALL),
            palette.text_secondary.color32(),
        );
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
    let origin = bounds.center() - size * 0.5;
    egui::Rect::from_min_size(origin, size)
}
