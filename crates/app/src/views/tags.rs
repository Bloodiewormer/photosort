use crate::model::count_phrase;
use crate::theme::{self, Palette};
use crate::views::label_actions::{LabelAction, draw_label_menu_items, flatten_category_tree};
use crate::vision_worker::TagsApplyEvent;
use catalog::{
    Catalog, DEFAULT_CUSTOM_THRESHOLD, LabelKind, LabelNode, RuleId, RuleMode, VisionRule,
};
use eframe::egui::{
    self, Align, CornerRadius, Frame, Key, Layout, Margin, ProgressBar, RichText, ScrollArea,
    Stroke,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagsApplyState {
    Idle,
    LoadingModel,
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

pub struct TagsViewState {
    pub selected: Option<RuleId>,
    pub edit_rule_id: Option<RuleId>,
    pub edit_enabled: bool,
    pub edit_threshold: f32,
    pub edit_positives: String,
    pub edit_negatives: String,
    pub edit_dirty: bool,
    pub confirm_delete: Option<RuleId>,

    pub new_open: bool,
    pub new_name: String,
    pub new_positives: String,
    pub new_negatives: String,

    pub search_query: String,
    pub apply_state: TagsApplyState,
    pub rank_error: Option<String>,
}

impl Default for TagsViewState {
    fn default() -> Self {
        Self {
            selected: None,
            edit_rule_id: None,
            edit_enabled: true,
            edit_threshold: DEFAULT_CUSTOM_THRESHOLD,
            edit_positives: String::new(),
            edit_negatives: String::new(),
            edit_dirty: false,
            confirm_delete: None,
            new_open: false,
            new_name: String::new(),
            new_positives: String::new(),
            new_negatives: String::new(),
            search_query: String::new(),
            apply_state: TagsApplyState::Idle,
            rank_error: None,
        }
    }
}

impl TagsViewState {
    pub fn apply_event(&mut self, ev: &TagsApplyEvent) {
        match ev {
            TagsApplyEvent::LoadingModel => {
                self.apply_state = TagsApplyState::LoadingModel;
            }
            TagsApplyEvent::Progress {
                done,
                total,
                matched_assets,
                suggestions,
            } => {
                self.apply_state = TagsApplyState::Running {
                    done: *done,
                    total: *total,
                    matched_assets: *matched_assets,
                    suggestions: *suggestions,
                };
            }
            TagsApplyEvent::Finished {
                stopped,
                processed,
                matched_assets,
                suggestions,
            } => {
                self.apply_state = TagsApplyState::Done {
                    stopped: *stopped,
                    processed: *processed,
                    matched_assets: *matched_assets,
                    suggestions: *suggestions,
                };
            }
            TagsApplyEvent::Failed(msg) => {
                self.apply_state = TagsApplyState::Failed(msg.clone());
            }
        }
    }

    pub fn sync_selection(&mut self, rules: &[VisionRule]) {
        let valid = self.selected.filter(|id| rules.iter().any(|r| r.id == *id));
        let next = valid.or_else(|| rules.first().map(|r| r.id));
        self.selected = next;
        if let Some(id) = next {
            if (self.edit_rule_id != Some(id) || !self.edit_dirty)
                && let Some(rule) = rules.iter().find(|r| r.id == id)
            {
                self.load_editor(rule);
            }
        } else {
            self.edit_rule_id = None;
            self.edit_dirty = false;
        }
    }

    fn load_editor(&mut self, rule: &VisionRule) {
        self.edit_rule_id = Some(rule.id);
        self.edit_enabled = rule.enabled;
        self.edit_threshold = rule.threshold;
        self.edit_positives = rule.positives.join("\n");
        self.edit_negatives = rule.negatives.join("\n");
        self.edit_dirty = false;
        self.confirm_delete = None;
    }
}

#[derive(Default)]
pub struct TagsViewOutcome {
    pub catalog_mutated: bool,
    pub start_apply: bool,
    pub cancel_apply: bool,
    /// `(title, texts, threshold)` to rank in the gallery on a background thread.
    pub rank_request: Option<(String, Vec<String>, Option<f32>)>,
    pub label_action: Option<LabelAction>,
    pub status_notice: Option<String>,
    pub error: Option<String>,
}

fn split_lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut TagsViewState,
    rules: &[VisionRule],
    all_labels: &[LabelNode],
    analyzed_count: usize,
    cat: Option<&mut Catalog>,
    apply_running: bool,
    rank_loading: bool,
    other_heavy_task: Option<&str>,
) -> TagsViewOutcome {
    let mut outcome = TagsViewOutcome::default();

    struct RuleEdit {
        id: RuleId,
        enabled: bool,
        threshold: f32,
        positives: Vec<String>,
        negatives: Vec<String>,
    }

    let mut toggle_rule: Option<(RuleId, bool)> = None;
    let mut save_edit: Option<RuleEdit> = None;
    let mut delete_rule: Option<RuleId> = None;
    let mut create_rule: Option<(String, Vec<String>, Vec<String>)> = None;
    let mut select_rule: Option<RuleId> = None;

    let busy = apply_running || other_heavy_task.is_some();

    // Right panel: selected rule editor
    egui::Panel::right("tags_detail")
        .default_size(380.0)
        .size_range(300.0..=520.0)
        .resizable(true)
        .frame(
            Frame::new()
                .fill(palette.bg_panel.color32())
                .inner_margin(Margin::same(theme::SPACE_MD as i8))
                .stroke(Stroke::new(1.0, palette.border.color32())),
        )
        .show(ui, |ui| {
            let selected_rule = state
                .selected
                .and_then(|id| rules.iter().find(|r| r.id == id));

            let Some(rule) = selected_rule else {
                ui.vertical_centered(|ui| {
                    ui.add_space(theme::SPACE_XL * 2.0);
                    ui.label(
                        RichText::new("Selecciona una regla para editar sus descripciones y umbral.")
                            .color(palette.text_secondary.color32()),
                    );
                });
                return;
            };

            if state.edit_rule_id != Some(rule.id) {
                state.load_editor(rule);
            }

            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                let mode_text = match (rule.mode, rule.kind) {
                    (RuleMode::Type, _) => "Tipo exclusivo",
                    (RuleMode::Independent, LabelKind::Category) => {
                        "Categoría independiente"
                    }
                    (RuleMode::Independent, _) => "Etiqueta independiente",
                };
                if ui.available_width() < 280.0 {
                    ui.label(RichText::new(&rule.name).size(theme::FONT_HEADING).strong());
                    ui.label(
                        RichText::new(mode_text)
                            .size(theme::FONT_SMALL)
                            .color(palette.text_secondary.color32()),
                    );
                } else {
                    egui::Sides::new().shrink_left().truncate().show(
                        ui,
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&rule.name).size(theme::FONT_HEADING).strong(),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&rule.name);
                        },
                        |ui| {
                            ui.label(
                                RichText::new(mode_text)
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                        },
                    );
                }
                ui.add_space(theme::SPACE_XS);

                // Accuracy counts for this rule (accepted / rejected AI suggestions)
                Frame::new()
                    .fill(palette.bg_card.color32())
                    .stroke(Stroke::new(1.0, palette.border.color32()))
                    .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                    .inner_margin(Margin::same(theme::SPACE_SM as i8))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(
                            RichText::new(format!(
                                "Sugerencias de IA · Aceptadas: {} · Rechazadas: {} · Pendientes: {}",
                                rule.accepted_ai, rule.rejected_ai, rule.suggested_ai
                            ))
                            .size(theme::FONT_SMALL)
                            .color(palette.text_primary.color32()),
                        );
                    });

                ui.add_space(theme::SPACE_SM);

                if ui
                    .checkbox(&mut state.edit_enabled, "Regla activada")
                    .changed()
                {
                    state.edit_dirty = true;
                }

                ui.add_space(theme::SPACE_SM);
                ui.label(RichText::new("Umbral de similitud coseno").strong());
                ui.horizontal(|ui| {
                    let slider = egui::Slider::new(&mut state.edit_threshold, 0.0..=0.50)
                        .step_by(0.005)
                        .fixed_decimals(3);
                    if theme::named(ui.add(slider), "Umbral de similitud coseno").changed() {
                        state.edit_dirty = true;
                    }
                });
                ui.label(
                    RichText::new(
                        "La similitud coseno NO es una probabilidad. Los umbrales iniciales no están calibrados; usa «Probar» para elegir el corte mirando las imágenes.",
                    )
                    .size(theme::FONT_SMALL)
                    .color(palette.text_secondary.color32()),
                );

                ui.add_space(theme::SPACE_MD);
                ui.label(RichText::new("Descripciones positivas (una por línea)").strong());
                ui.label(
                    RichText::new(
                        "Las predeterminadas están en inglés; también puedes escribir en español.",
                    )
                    .size(theme::FONT_SMALL)
                    .color(palette.text_secondary.color32()),
                );
                ui.add_space(theme::SPACE_XS);
                if theme::named(
                    ui.add(
                        egui::TextEdit::multiline(&mut state.edit_positives)
                            .desired_width(ui.available_width())
                            .desired_rows(4)
                            .hint_text(theme::hint_text(
                                palette,
                                "Una descripción por línea…",
                            )),
                    ),
                    "Descripciones positivas",
                )
                .changed()
                {
                    state.edit_dirty = true;
                }

                ui.add_space(theme::SPACE_SM);
                ui.label(
                    RichText::new("Descripciones negativas (opcional, una por línea)").strong(),
                );
                ui.add_space(theme::SPACE_XS);
                if theme::named(
                    ui.add(
                        egui::TextEdit::multiline(&mut state.edit_negatives)
                            .desired_width(ui.available_width())
                            .desired_rows(3)
                            .hint_text(theme::hint_text(
                                palette,
                                "Descripciones que descartan esta etiqueta…",
                            )),
                    ),
                    "Descripciones negativas",
                )
                .changed()
                {
                    state.edit_dirty = true;
                }

                ui.add_space(theme::SPACE_MD);
                ui.horizontal_wrapped(|ui| {
                    let pos_lines = split_lines(&state.edit_positives);
                    let can_save = state.edit_dirty && !pos_lines.is_empty();
                    if ui
                        .add_enabled(can_save, egui::Button::new("Guardar cambios"))
                        .clicked()
                    {
                        let neg_lines = split_lines(&state.edit_negatives);
                        save_edit = Some(RuleEdit {
                            id: rule.id,
                            enabled: state.edit_enabled,
                            threshold: state.edit_threshold,
                            positives: pos_lines.clone(),
                            negatives: neg_lines,
                        });
                    }
                    if state.edit_dirty && ui.button("Descartar").clicked() {
                        state.load_editor(rule);
                    }
                    let can_test = analyzed_count > 0 && !pos_lines.is_empty() && !rank_loading;
                    if ui
                        .add_enabled(can_test, egui::Button::new("Probar en la galería"))
                        .on_disabled_hover_text(if analyzed_count == 0 {
                            "Activa «Reconocer imágenes» en Configuración primero"
                        } else if rank_loading {
                            "Calculando similitud…"
                        } else {
                            "Añade al menos una descripción positiva"
                        })
                        .clicked()
                    {
                        outcome.rank_request = Some((
                            format!("Prueba: {}", rule.name),
                            pos_lines,
                            Some(state.edit_threshold),
                        ));
                    }
                });

                ui.add_space(theme::SPACE_MD);
                ui.separator();
                ui.add_space(theme::SPACE_SM);

                if state.confirm_delete == Some(rule.id) {
                    ui.label(format!(
                        "¿Eliminar la regla «{}»? Se conservan la etiqueta y tus decisiones (aceptadas y rechazadas). Sus sugerencias de IA se quitarán al volver a aplicar. Las fotos no cambian.",
                        rule.name
                    ));
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("Sí, eliminar regla"))
                            .clicked()
                        {
                            delete_rule = Some(rule.id);
                        }
                        if ui.button("Conservar").clicked() {
                            state.confirm_delete = None;
                        }
                    });
                } else if ui
                    .add_enabled(!busy, egui::Button::new("Eliminar regla…"))
                    .on_disabled_hover_text("Espera a que termine la tarea en curso.")
                    .clicked()
                {
                    state.confirm_delete = Some(rule.id);
                }
            });
        });

    // Central panel: actions, free-text search by description, create custom tag, rule list
    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(palette.bg_canvas.color32())
                .inner_margin(Margin::same(theme::SPACE_MD as i8)),
        )
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("Etiquetas ({})", rules.len()))
                        .size(theme::FONT_HEADING)
                        .strong(),
                );
                ui.label(
                    RichText::new(format!(
                        "· {}",
                        count_phrase(
                            analyzed_count,
                            "imagen analizada",
                            "imágenes analizadas"
                        )
                    ))
                    .size(theme::FONT_SMALL)
                    .color(palette.text_secondary.color32()),
                );
                ui.add_space(theme::SPACE_SM);

                if apply_running {
                    if ui.button("Cancelar aplicación").clicked() {
                        outcome.cancel_apply = true;
                    }
                } else {
                    let can_apply =
                        cat.is_some() && analyzed_count > 0 && other_heavy_task.is_none();
                    let tip = if let Some(task) = other_heavy_task {
                        format!("Espera a que termine {task}.")
                    } else if analyzed_count == 0 {
                        "Activa «Reconocer imágenes» en Configuración primero".into()
                    } else {
                        "Recalcula sugerencias usando los vectores guardados sin leer archivos de imagen.".into()
                    };
                    if ui
                        .add_enabled(
                            can_apply,
                            egui::Button::new("Aplicar a las imágenes analizadas"),
                        )
                        .on_hover_text(tip.clone())
                        .on_disabled_hover_text(tip)
                        .clicked()
                    {
                        outcome.start_apply = true;
                    }
                }

                let toggle_label = if state.new_open {
                    "Ocultar nueva etiqueta"
                } else {
                    "Nueva etiqueta…"
                };
                if ui.button(toggle_label).clicked() {
                    state.new_open = !state.new_open;
                }
            });

            // Apply worker progress / status
            match &state.apply_state {
                TagsApplyState::Idle => {}
                TagsApplyState::LoadingModel => {
                    ui.add_space(theme::SPACE_SM);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new("Cargando modelo de texto para las descripciones…")
                                .size(theme::FONT_SMALL)
                                .color(palette.text_secondary.color32()),
                        );
                    });
                }
                TagsApplyState::Running {
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
                            "Aplicando reglas: {done} de {total} · {} con sugerencias ({})",
                            count_phrase(*matched_assets, "imagen", "imágenes"),
                            count_phrase(*suggestions, "sugerencia", "sugerencias"),
                        ))
                        .size(theme::FONT_SMALL)
                        .color(palette.text_secondary.color32()),
                    );
                }
                TagsApplyState::Done {
                    stopped,
                    processed,
                    matched_assets,
                    suggestions,
                } => {
                    ui.add_space(theme::SPACE_SM);
                    let prefix = if *stopped {
                        "Aplicación detenida"
                    } else {
                        "Reglas aplicadas"
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
                                    "{prefix}: {} evaluadas, {} con sugerencias ({}).",
                                    count_phrase(*processed, "imagen", "imágenes"),
                                    count_phrase(*matched_assets, "imagen", "imágenes"),
                                    count_phrase(*suggestions, "sugerencia", "sugerencias"),
                                ))
                                .size(theme::FONT_SMALL)
                                .color(palette.chip_require_fg.color32()),
                            );
                        });
                }
                TagsApplyState::Failed(err) => {
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
                                RichText::new(format!("No se pudieron aplicar las reglas: {err}"))
                                    .size(theme::FONT_SMALL)
                                    .color(palette.chip_exclude_fg.color32()),
                            );
                        });
                }
            }

            ui.add_space(theme::SPACE_MD);

            // Free-text "Buscar por descripción" bar
            Frame::new()
                .fill(palette.bg_panel.color32())
                .stroke(Stroke::new(1.0, palette.border.color32()))
                .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                .inner_margin(Margin::same(theme::SPACE_SM as i8))
                .show(ui, |ui| {
                    ui.label(RichText::new("Buscar por descripción").strong());
                    ui.add_space(theme::SPACE_XS);
                    ui.horizontal(|ui| {
                        let edit = theme::named(
                            ui.add(
                                egui::TextEdit::singleline(&mut state.search_query)
                                    .desired_width((ui.available_width() - 150.0).max(180.0))
                                    .hint_text(theme::hint_text(
                                        palette,
                                        "Describe lo que buscas (inglés o español)…",
                                    )),
                            ),
                            "Buscar por descripción",
                        );
                        let can_search = analyzed_count > 0
                            && !state.search_query.trim().is_empty()
                            && !rank_loading;
                        let submit = ui
                            .add_enabled(can_search, egui::Button::new("Buscar similares"))
                            .clicked()
                            || (can_search
                                && edit.lost_focus()
                                && ui.input(|i| i.key_pressed(Key::Enter)));
                        if submit {
                            let q = state.search_query.trim().to_string();
                            state.rank_error = None;
                            outcome.rank_request =
                                Some((format!("Búsqueda visual: «{q}»"), vec![q], None));
                        }
                    });
                    if rank_loading {
                        ui.add_space(theme::SPACE_XS);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(
                                RichText::new("Codificando descripción y ordenando imágenes…")
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                        });
                    }
                    if let Some(err) = &state.rank_error {
                        ui.add_space(theme::SPACE_XS);
                        ui.label(
                            RichText::new(err)
                                .size(theme::FONT_SMALL)
                                .color(palette.chip_exclude_fg.color32()),
                        );
                    }
                });

            // Optional inline form to create a custom independent tag rule
            if state.new_open {
                ui.add_space(theme::SPACE_SM);
                Frame::new()
                    .fill(palette.bg_panel.color32())
                    .stroke(Stroke::new(1.0, palette.accent.color32()))
                    .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                    .inner_margin(Margin::same(theme::SPACE_SM as i8))
                    .show(ui, |ui| {
                        ui.label(RichText::new("Nueva etiqueta personalizada").strong());
                        ui.add_space(theme::SPACE_XS);
                        ui.horizontal(|ui| {
                            ui.label("Nombre:");
                            theme::named(
                                ui.add(
                                    egui::TextEdit::singleline(&mut state.new_name)
                                        .desired_width(220.0)
                                        .hint_text(theme::hint_text(palette, "p. ej. paisaje, gato…")),
                                ),
                                "Nombre de la etiqueta",
                            );
                        });
                        ui.add_space(theme::SPACE_XS);
                        ui.label(
                            RichText::new("Descripciones positivas (una por línea, inglés o español):")
                                .size(theme::FONT_SMALL)
                                .color(palette.text_secondary.color32()),
                        );
                        theme::named(
                            ui.add(
                                egui::TextEdit::multiline(&mut state.new_positives)
                                    .desired_width(ui.available_width())
                                    .desired_rows(2)
                                    .hint_text(theme::hint_text(
                                        palette,
                                        "a photo of a cat\nuna foto de un gato",
                                    )),
                            ),
                            "Descripciones positivas de la etiqueta nueva",
                        );
                        ui.add_space(theme::SPACE_XS);
                        ui.horizontal(|ui| {
                            let pos = split_lines(&state.new_positives);
                            let can_create =
                                !state.new_name.trim().is_empty() && !pos.is_empty();
                            if ui
                                .add_enabled(can_create, egui::Button::new("Crear etiqueta"))
                                .clicked()
                            {
                                create_rule = Some((
                                    state.new_name.trim().to_string(),
                                    pos,
                                    split_lines(&state.new_negatives),
                                ));
                            }
                            if ui.button("Cancelar").clicked() {
                                state.new_open = false;
                            }
                        });
                    });
            }

            ui.add_space(theme::SPACE_MD);

            // Scrollable catalog tree + rules list grouped by mode
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    draw_catalog_labels_section(ui, palette, all_labels, &mut outcome);
                    ui.add_space(theme::SPACE_MD);
                    ui.separator();
                    ui.add_space(theme::SPACE_MD);

                    let type_rules: Vec<&VisionRule> = rules
                        .iter()
                        .filter(|r| r.mode == RuleMode::Type)
                        .collect();
                    let ind_cat_rules: Vec<&VisionRule> = rules
                        .iter()
                        .filter(|r| r.mode == RuleMode::Independent && r.kind == LabelKind::Category)
                        .collect();
                    let ind_tag_rules: Vec<&VisionRule> = rules
                        .iter()
                        .filter(|r| r.mode == RuleMode::Independent && r.kind != LabelKind::Category)
                        .collect();

                    for (heading, subtitle, group) in [
                        (
                            "Tipo de imagen (exclusivo)",
                            "Como máximo una categoría por imagen; las más ambiguas aparecen primero en Sugerencias",
                            type_rules,
                        ),
                        (
                            "Categorías independientes",
                            "Se combinan libremente con el tipo de imagen. «Portada de manga» solo se sugiere cuando el tipo ganador es Manga o Ilustración",
                            ind_cat_rules,
                        ),
                        (
                            "Etiquetas independientes",
                            "Se evalúan por separado. «persona» también se activa con rostros YuNet ≥ 0,80 en Foto, Meme o Capturas de pantalla",
                            ind_tag_rules,
                        ),
                    ] {
                        ui.label(RichText::new(heading).strong());
                        ui.label(
                            RichText::new(subtitle)
                                .size(theme::FONT_SMALL)
                                .color(palette.text_secondary.color32()),
                        );
                        ui.add_space(theme::SPACE_XS);

                        if group.is_empty() {
                            ui.label(
                                RichText::new("Sin reglas en este grupo.")
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                        } else {
                            for rule in group {
                                let selected = state.selected == Some(rule.id);
                                let bg = if selected {
                                    palette.accent_bg.color32()
                                } else {
                                    palette.bg_card.color32()
                                };
                                let stroke = if selected {
                                    Stroke::new(
                                        theme::FOCUS_STROKE_WIDTH,
                                        palette.accent.color32(),
                                    )
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
                                        ui.horizontal(|ui| {
                                            let mut enabled = rule.enabled;
                                            let chk = ui
                                                .checkbox(&mut enabled, "")
                                                .on_hover_text("Activar o desactivar esta regla");
                                            chk.widget_info(|| {
                                                egui::WidgetInfo::selected(
                                                    egui::Role::CheckBox,
                                                    true,
                                                    enabled,
                                                    format!("Regla activada: {}", rule.name),
                                                )
                                            });
                                            if chk.changed() {
                                                toggle_rule = Some((rule.id, enabled));
                                            }

                                            let name_btn = ui.add(
                                                egui::Button::new(
                                                    RichText::new(&rule.name)
                                                        .color(if rule.enabled {
                                                            palette.text_primary.color32()
                                                        } else {
                                                            palette.text_secondary.color32()
                                                        })
                                                        .strong(),
                                                )
                                                .frame(false),
                                            );
                                            if name_btn.clicked() {
                                                select_rule = Some(rule.id);
                                            }

                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    let can_test = analyzed_count > 0
                                                        && !rule.positives.is_empty()
                                                        && !rank_loading;
                                                    if ui
                                                        .add_enabled(
                                                            can_test,
                                                            egui::Button::new("Probar"),
                                                        )
                                                        .on_hover_text(
                                                            "Ordenar la galería por similitud con esta regla",
                                                        )
                                                        .clicked()
                                                    {
                                                        outcome.rank_request = Some((
                                                            format!("Prueba: {}", rule.name),
                                                            rule.positives.clone(),
                                                            Some(rule.threshold),
                                                        ));
                                                    }
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "Umbral {:.2} · Acept. {} · Rech. {}",
                                                            rule.threshold,
                                                            rule.accepted_ai,
                                                            rule.rejected_ai
                                                        ))
                                                        .size(theme::FONT_SMALL)
                                                        .color(palette.text_secondary.color32()),
                                                    )
                                                    .on_hover_text(format!(
                                                        "Umbral coseno: {:.3} · Aceptadas: {} · Rechazadas: {} · Pendientes: {}",
                                                        rule.threshold,
                                                        rule.accepted_ai,
                                                        rule.rejected_ai,
                                                        rule.suggested_ai
                                                    ));
                                                },
                                            );
                                        });
                                    });
                                ui.add_space(theme::SPACE_XS);
                            }
                        }
                        ui.add_space(theme::SPACE_MD);
                    }
                });
        });

    if let Some(id) = select_rule {
        state.selected = Some(id);
        if let Some(rule) = rules.iter().find(|r| r.id == id) {
            state.load_editor(rule);
        }
    }

    // Apply catalog mutations
    if let Some(c) = cat {
        if let Some((id, enabled)) = toggle_rule
            && let Some(rule) = rules.iter().find(|r| r.id == id)
        {
            match c.update_vision_rule(
                id,
                enabled,
                rule.threshold,
                &rule.positives,
                &rule.negatives,
            ) {
                Ok(_) => {
                    if state.edit_rule_id == Some(id) {
                        state.edit_enabled = enabled;
                    }
                    outcome.catalog_mutated = true;
                    outcome.status_notice = Some(
                        "Regla actualizada. Pulsa «Aplicar a las imágenes analizadas» para reflejar el cambio."
                            .into(),
                    );
                }
                Err(e) => {
                    outcome.error = Some(format!("No se pudo actualizar la regla: {e}"));
                }
            }
        }
        if let Some(edit) = save_edit {
            match c.update_vision_rule(
                edit.id,
                edit.enabled,
                edit.threshold,
                &edit.positives,
                &edit.negatives,
            ) {
                Ok(_) => {
                    state.edit_dirty = false;
                    outcome.catalog_mutated = true;
                    outcome.status_notice = Some(
                        "Cambios guardados. Pulsa «Aplicar a las imágenes analizadas» para recalcular sugerencias."
                            .into(),
                    );
                }
                Err(e) => {
                    outcome.error = Some(format!("No se pudieron guardar los cambios: {e}"));
                }
            }
        }
        if let Some(id) = delete_rule {
            state.confirm_delete = None;
            match c.delete_vision_rule(id) {
                Ok(_) => {
                    state.selected = None;
                    state.edit_rule_id = None;
                    state.edit_dirty = false;
                    outcome.catalog_mutated = true;
                    outcome.status_notice = Some(
                        "Regla eliminada. Se conservaron la etiqueta y tus decisiones; pulsa «Aplicar» para limpiar sus sugerencias pendientes."
                            .into(),
                    );
                }
                Err(e) => {
                    outcome.error = Some(format!("No se pudo eliminar la regla: {e}"));
                }
            }
        }
        // Also across kinds: a tag called like a type would be confusing in every filter.
        if let Some((name, _, _)) = &create_rule
            && rules
                .iter()
                .any(|r| r.name.to_lowercase() == name.to_lowercase())
        {
            outcome.error = Some(
                "Ya existe una regla con ese nombre. Selecciónala en la lista para editarla."
                    .into(),
            );
        } else if let Some((name, pos, neg)) = create_rule {
            match c.create_vision_rule(
                &name,
                LabelKind::Tag,
                RuleMode::Independent,
                DEFAULT_CUSTOM_THRESHOLD,
                &pos,
                &neg,
            ) {
                Ok(id) => {
                    state.selected = Some(id);
                    state.edit_rule_id = None;
                    state.new_name.clear();
                    state.new_positives.clear();
                    state.new_negatives.clear();
                    state.new_open = false;
                    outcome.catalog_mutated = true;
                    outcome.status_notice = Some(
                        "Etiqueta creada. Usa «Probar» para ajustar su umbral o «Aplicar» para sugerirla."
                            .into(),
                    );
                }
                Err(e) => {
                    outcome.error = Some(format!("No se pudo crear la etiqueta: {e}"));
                }
            }
        }
    }

    outcome
}

fn draw_catalog_labels_section(
    ui: &mut egui::Ui,
    palette: &Palette,
    all_labels: &[LabelNode],
    outcome: &mut TagsViewOutcome,
) {
    let cat_tree = flatten_category_tree(all_labels, true);
    let tags: Vec<&LabelNode> = all_labels
        .iter()
        .filter(|l| l.kind == LabelKind::Tag)
        .collect();

    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new("Categorías y etiquetas del catálogo")
                .size(theme::FONT_HEADING)
                .strong(),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Nueva etiqueta").clicked() {
                outcome.label_action = Some(LabelAction::CreateTag);
            }
            if ui.button("Nueva categoría").clicked() {
                outcome.label_action = Some(LabelAction::CreateCategory { parent: None });
            }
        });
    });
    ui.label(
        RichText::new(
            "Organiza categorías en árbol o gestiona etiquetas; ninguna acción modifica archivos",
        )
        .size(theme::FONT_SMALL)
        .color(palette.text_secondary.color32()),
    );
    ui.add_space(theme::SPACE_SM);

    egui::CollapsingHeader::new(RichText::new(format!("Categorías ({})", cat_tree.len())).strong())
        .id_salt("tags_view_catalog_categories")
        .default_open(true)
        .show(ui, |ui| {
            if cat_tree.is_empty() {
                ui.label(
                    RichText::new("Sin categorías todavía")
                        .size(theme::FONT_SMALL)
                        .color(palette.text_secondary.color32()),
                );
            } else {
                for (node, depth) in cat_tree {
                    draw_catalog_label_row(ui, palette, node, depth, outcome);
                }
            }
        });

    ui.add_space(theme::SPACE_XS);
    egui::CollapsingHeader::new(RichText::new(format!("Etiquetas ({})", tags.len())).strong())
        .id_salt("tags_view_catalog_tags")
        .default_open(false)
        .show(ui, |ui| {
            if tags.is_empty() {
                ui.label(
                    RichText::new("Sin etiquetas todavía")
                        .size(theme::FONT_SMALL)
                        .color(palette.text_secondary.color32()),
                );
            } else {
                for node in tags {
                    draw_catalog_label_row(ui, palette, node, 0, outcome);
                }
            }
        });
}

fn draw_catalog_label_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    node: &LabelNode,
    depth: usize,
    outcome: &mut TagsViewOutcome,
) {
    let row_resp = Frame::new()
        .fill(palette.bg_card.color32())
        .stroke(Stroke::new(1.0, palette.border.color32()))
        .corner_radius(CornerRadius::same(theme::RADIUS_SM))
        .inner_margin(Margin::symmetric(
            theme::SPACE_SM as i8,
            theme::SPACE_XS as i8,
        ))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if depth > 0 {
                    ui.add_space((depth.min(6) as f32) * 16.0);
                    ui.label(
                        RichText::new("└")
                            .size(theme::FONT_SMALL)
                            .color(palette.text_hint.color32()),
                    );
                }
                ui.label(
                    RichText::new(&node.name)
                        .color(palette.text_primary.color32())
                        .strong(),
                );
                ui.label(
                    RichText::new(count_phrase(node.images.max(0) as usize, "foto", "fotos"))
                        .size(theme::FONT_SMALL)
                        .color(palette.text_hint.color32()),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.menu_button("Acciones", |ui| {
                        if let Some(act) = draw_label_menu_items(ui, node.kind, node.label) {
                            outcome.label_action = Some(act);
                        }
                    });
                });
            });
        })
        .response
        // Right click only; «Acciones» is the keyboard path.
        .interact(egui::Sense::CLICK);

    row_resp.context_menu(|ui| {
        if let Some(act) = draw_label_menu_items(ui, node.kind, node.label) {
            outcome.label_action = Some(act);
        }
    });
    ui.add_space(2.0);
}
