use crate::model::count_phrase;
use crate::theme::{self, Palette};
use crate::works_worker::WorksEvent;
use catalog::{Catalog, LabelId, WorkAlias, WorkProposal, WorkSummary, work_match_key};
use eframe::egui::{
    self, Align, CornerRadius, Frame, Key, Layout, Margin, ProgressBar, RichText, ScrollArea,
    Stroke, Vec2,
};
use std::collections::BTreeSet;

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
    pub start_match: bool,
    pub cancel_match: bool,
    pub catalog_mutated: bool,
    /// A work that no longer exists, so that no filter keeps pointing at it.
    pub deleted: Option<LabelId>,
    pub status_notice: Option<String>,
    pub error: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut WorksViewState,
    works: &[WorkSummary],
    cat: Option<&mut Catalog>,
    worker_running: bool,
    indexing: bool,
    ocr_running: bool,
) -> WorksViewOutcome {
    let mut outcome = WorksViewOutcome::default();

    let mut create_work_name: Option<String> = None;
    let mut add_alias: Option<(LabelId, String)> = None;
    let mut remove_alias: Option<(LabelId, String)> = None;
    let mut open_proposals = false;
    let mut create_proposals: Vec<String> = Vec::new();
    let mut select_changed = false;
    let mut delete_work: Option<LabelId> = None;
    // A running worker would write against the deleted work.
    let busy = worker_running || indexing || ocr_running;

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

            ScrollArea::vertical().show(ui, |ui| {
                ui.label(RichText::new(&work.name).size(theme::FONT_HEADING).strong());
                ui.add_space(theme::SPACE_XS);
                ui.label(
                    RichText::new(format!(
                        "{} · {}",
                        count_phrase(work.images, "imagen", "imágenes"),
                        count_phrase(work.aliases, "alias adicional", "alias adicionales"),
                    ))
                    .size(theme::FONT_SMALL)
                    .color(palette.text_secondary.color32()),
                );
                ui.add_space(theme::SPACE_SM);

                if ui.button("Ver imágenes en la galería").clicked() {
                    outcome.open_in_gallery = Some(work.label);
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
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut state.new_alias_input)
                            .desired_width((ui.available_width() - 108.0).max(120.0))
                            .hint_text(theme::hint_text(palette, "Añadir alias…")),
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
                    let can_match = cat.is_some() && !indexing && !ocr_running;
                    let disabled_tip = if indexing {
                        "Espera a que termine el indexado antes de buscar obras."
                    } else if ocr_running {
                        "Espera a que termine la lectura OCR antes de buscar obras."
                    } else {
                        "No hay catálogo abierto."
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

            ui.add_space(theme::SPACE_MD);

            // Create new work + filter bar
            ui.horizontal(|ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut state.new_work_input)
                        .desired_width(240.0)
                        .hint_text(theme::hint_text(palette, "Nueva obra o grupo…")),
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

                ui.add(
                    egui::TextEdit::singleline(&mut state.filter)
                        .desired_width((ui.available_width() - 80.0).max(160.0))
                        .hint_text(theme::hint_text(palette, "Filtrar obras…")),
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
                                        let img_btn_label =
                                            count_phrase(work.images, "imagen", "imágenes");
                                        if ui
                                            .button(img_btn_label)
                                            .on_hover_text("Mostrar estas imágenes en la galería")
                                            .clicked()
                                        {
                                            outcome.open_in_gallery = Some(work.label);
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

    outcome
}
