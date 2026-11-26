use crate::model::{format_bytes, format_timestamp_utc};
use crate::theme::{self, Palette};
use crate::views::gallery::fit_rect;
use catalog::{
    AssetId, Assignment, DetectedFace, Entry, LabelId, LabelKind, OcrLine, SCREENSHOT_CATEGORY,
    Source, Status, WorkSummary,
};
use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Id, Key, Layout, Margin, Rect, RichText, ScrollArea,
    Sense, Stroke, TextureHandle, Vec2,
};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub const NEW_LABEL_INPUT_ID: &str = "photosort_inspector_new_label";

pub struct InspectorState {
    pub asset: Option<AssetId>,
    pub labels: Vec<Assignment>,
    pub ocr_lines: Vec<OcrLine>,
    pub ocr_done: bool,
    pub faces: Vec<DetectedFace>,
    pub vision_done: bool,
    pub ocr_checked: BTreeSet<usize>,
    pub ocr_draft: String,
    pub alias_target: Option<LabelId>,
    pub new_label_kind: LabelKind,
    pub new_label_text: String,
    pub focus_new_label_next_frame: bool,
}

impl Default for InspectorState {
    fn default() -> Self {
        Self {
            asset: None,
            labels: Vec::new(),
            ocr_lines: Vec::new(),
            ocr_done: false,
            faces: Vec::new(),
            vision_done: false,
            ocr_checked: BTreeSet::new(),
            ocr_draft: String::new(),
            alias_target: None,
            new_label_kind: LabelKind::Tag,
            new_label_text: String::new(),
            focus_new_label_next_frame: false,
        }
    }
}

#[derive(Default)]
pub struct InspectorOutcome {
    pub open_viewer: bool,
    pub review_asset: Option<AssetId>,
    pub accept_action: Option<(AssetId, LabelId)>,
    pub reject_action: Option<(AssetId, LabelId)>,
    pub add_user_label: Option<(AssetId, LabelKind, String)>,
    pub create_work_from_ocr: Option<(AssetId, String)>,
    pub add_alias_from_ocr: Option<(AssetId, LabelId, String)>,
}

/// Renders the right-side inspector ONLY when `selected_entry` is `Some`.
/// Hidden completely when nothing is selected.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    ui: &mut egui::Ui,
    palette: &Palette,
    selected_entry: Option<&Entry>,
    is_reviewed: bool,
    state: &mut InspectorState,
    works: &[WorkSummary],
    preview_loaded: Option<&(PathBuf, Result<TextureHandle, String>)>,
) -> InspectorOutcome {
    let mut outcome = InspectorOutcome::default();
    let Some(entry) = selected_entry else {
        return outcome;
    };

    egui::Panel::right("inspector")
        .default_size(theme::INSPECTOR_WIDTH)
        .size_range(280.0..=500.0)
        .resizable(true)
        .frame(
            Frame::new()
                .fill(palette.bg_panel.color32())
                .inner_margin(Margin::same(theme::SPACE_LG as i8))
                .stroke(Stroke::new(1.0, palette.border.color32())),
        )
        .show(ui, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                // Header: filename + actions
                ui.horizontal(|ui| {
                    if is_reviewed {
                        let (mark_rect, mark_resp) =
                            ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                        theme::paint_reviewed_mark(ui.painter(), mark_rect, palette);
                        mark_resp.on_hover_text("Revisada");
                    }
                    // The mark alone would say it by shape and colour only.
                    ui.label(if is_reviewed {
                        "Revisada"
                    } else {
                        "Sin revisar"
                    });
                    let review_lbl = if is_reviewed {
                        "Volver a revisar"
                    } else {
                        "Revisar"
                    };
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Visor").clicked() {
                            outcome.open_viewer = true;
                        }
                        if ui.button(review_lbl).clicked() {
                            outcome.review_asset = Some(entry.asset);
                        }
                    });
                });
                ui.add_space(theme::SPACE_SM);

                // Preview box
                let preview_w = ui.available_width().max(120.0);
                let preview_h = 200.0;
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(preview_w, preview_h), Sense::hover());
                ui.painter().rect(
                    rect,
                    CornerRadius::same(theme::RADIUS_MD),
                    palette.bg_canvas.color32(),
                    Stroke::new(1.0, palette.border.color32()),
                    egui::StrokeKind::Inside,
                );
                match preview_loaded {
                    Some((p, Ok(tex))) if p == &entry.path => {
                        let fit = fit_rect(rect.shrink(theme::SPACE_XS), tex.size_vec2());
                        ui.painter().image(
                            tex.id(),
                            fit,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
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
                            "Cargando vista previa…",
                            egui::FontId::proportional(theme::FONT_SMALL),
                            palette.text_secondary.color32(),
                        );
                    }
                }

                ui.add_space(theme::SPACE_MD);

                // Section 1: Información (with collapsed Detalles for SHA-256 & technical info)
                egui::CollapsingHeader::new(RichText::new("Información").strong())
                    .id_salt("inspector_sec_info")
                    .default_open(true)
                    .show(ui, |ui| {
                        let filename = entry
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| entry.path.display().to_string());
                        meta_row(ui, palette, "Archivo", &filename);
                        if let Some(parent) = entry.path.parent() {
                            meta_row(ui, palette, "Carpeta", &parent.display().to_string());
                        }
                        meta_row(ui, palette, "Tamaño", &format_bytes(entry.size));
                        let date_str = entry
                            .modified_ns
                            .map(format_timestamp_utc)
                            .unwrap_or_else(|| "Desconocida".into());
                        meta_row(ui, palette, "Modificado", &date_str);

                        ui.add_space(theme::SPACE_XS);
                        egui::CollapsingHeader::new(
                            RichText::new("Detalles")
                                .size(theme::FONT_SMALL)
                                .color(palette.text_secondary.color32()),
                        )
                        .id_salt("inspector_sec_details")
                        .default_open(false)
                        .show(ui, |ui| {
                            meta_row(ui, palette, "Ruta", &entry.path.display().to_string());
                            let short_sha = if entry.sha256.len() > 16 {
                                format!("{}…", &entry.sha256[..16])
                            } else {
                                entry.sha256.clone()
                            };
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("SHA-256")
                                        .size(theme::FONT_SMALL)
                                        .color(palette.text_secondary.color32()),
                                );
                                ui.label(
                                    RichText::new(&short_sha)
                                        .size(theme::FONT_SMALL)
                                        .monospace(),
                                )
                                .on_hover_text(&entry.sha256);
                            });
                            if state.vision_done {
                                let face_count = state.faces.len();
                                let face_label = if face_count == 0 {
                                    "0".to_string()
                                } else {
                                    let max_score = state
                                        .faces
                                        .iter()
                                        .map(|f| f.score)
                                        .fold(0.0f32, f32::max);
                                    format!("{face_count} (máx. {max_score:.2})")
                                };
                                meta_row(ui, palette, "Rostros", &face_label);
                            } else {
                                meta_row(ui, palette, "Reconocimiento visual", "Pendiente");
                            }
                            meta_row(
                                ui,
                                palette,
                                "Lectura OCR",
                                if state.ocr_done { "Hecha" } else { "Pendiente" },
                            );
                        });
                    });

                ui.add_space(theme::SPACE_SM);

                // Section 2: Texto (OCR)
                let is_screenshot = state.labels.iter().any(|a| {
                    a.kind == LabelKind::Category
                        && a.name.eq_ignore_ascii_case(SCREENSHOT_CATEGORY)
                        && a.status != Status::Rejected
                });
                let mut main_ocr: Vec<&OcrLine> = Vec::new();
                let mut status_bar_ocr: Vec<&OcrLine> = Vec::new();
                for line in &state.ocr_lines {
                    if line.top_strip && is_screenshot {
                        status_bar_ocr.push(line);
                    } else {
                        main_ocr.push(line);
                    }
                }

                let text_title = if main_ocr.is_empty() {
                    "Texto".to_string()
                } else {
                    format!("Texto ({})", main_ocr.len())
                };
                egui::CollapsingHeader::new(RichText::new(text_title).strong())
                    .id_salt("inspector_sec_text")
                    .default_open(!main_ocr.is_empty())
                    .show(ui, |ui| {
                        if main_ocr.is_empty() && status_bar_ocr.is_empty() {
                            let empty_msg = if state.ocr_done {
                                "Sin texto detectado"
                            } else {
                                "Sin lectura todavía"
                            };
                            ui.label(
                                RichText::new(empty_msg)
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                        } else {
                            if !main_ocr.is_empty() {
                                ui.horizontal(|ui| {
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.button("Copiar texto").clicked() {
                                            let joined: String = main_ocr
                                                .iter()
                                                .map(|l| l.text.as_str())
                                                .collect::<Vec<_>>()
                                                .join("\n");
                                            ui.ctx().copy_text(joined);
                                        }
                                    });
                                });
                                ui.add_space(theme::SPACE_XS);

                                let mut ticks_changed = false;
                                Frame::new()
                                    .fill(palette.bg_card.color32())
                                    .stroke(Stroke::new(1.0, palette.border.color32()))
                                    .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                                    .inner_margin(Margin::same(theme::SPACE_SM as i8))
                                    .show(ui, |ui| {
                                        ui.set_min_width(ui.available_width());
                                        for (idx, line) in main_ocr.iter().enumerate() {
                                            let mut checked = state.ocr_checked.contains(&idx);
                                            ui.horizontal_wrapped(|ui| {
                                                let tick = ui.checkbox(&mut checked, "").on_hover_text(
                                                    "Usar esta línea como título de obra o alias",
                                                );
                                                tick.widget_info(|| {
                                                    egui::WidgetInfo::selected(
                                                        egui::Role::CheckBox,
                                                        true,
                                                        checked,
                                                        format!("Usar como título: {}", line.text),
                                                    )
                                                });
                                                if tick.changed() {
                                                    if checked {
                                                        state.ocr_checked.insert(idx);
                                                    } else {
                                                        state.ocr_checked.remove(&idx);
                                                    }
                                                    ticks_changed = true;
                                                }
                                                if line.score < ocr::select::REVIEW_BELOW {
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(&line.text).color(
                                                                palette.text_secondary.color32(),
                                                            ),
                                                        )
                                                        .selectable(true),
                                                    );
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "({:.0}%)",
                                                            line.score * 100.0
                                                        ))
                                                        .size(theme::FONT_SMALL)
                                                        .color(palette.text_hint.color32()),
                                                    );
                                                } else {
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(&line.text).color(
                                                                palette.text_primary.color32(),
                                                            ),
                                                        )
                                                        .selectable(true),
                                                    );
                                                }
                                            });
                                        }
                                    });

                                if ticks_changed {
                                    state.ocr_draft = state
                                        .ocr_checked
                                        .iter()
                                        .filter_map(|&idx| main_ocr.get(idx).map(|l| l.text.trim()))
                                        .filter(|s| !s.is_empty())
                                        .collect::<Vec<_>>()
                                        .join(" ");
                                }

                                if !state.ocr_checked.is_empty()
                                    || !state.ocr_draft.trim().is_empty()
                                {
                                    ui.add_space(theme::SPACE_XS);
                                    ui.add(
                                        egui::TextEdit::singleline(&mut state.ocr_draft)
                                            .desired_width(ui.available_width())
                                            .hint_text(theme::hint_text(
                                                palette,
                                                "Título o alias…",
                                            )),
                                    );
                                    ui.add_space(theme::SPACE_XS);
                                    let draft_ready = !state.ocr_draft.trim().is_empty();
                                    ui.horizontal_wrapped(|ui| {
                                        if ui
                                            .add_enabled(
                                                draft_ready,
                                                egui::Button::new("Crear obra"),
                                            )
                                            .clicked()
                                        {
                                            outcome.create_work_from_ocr = Some((
                                                entry.asset,
                                                state.ocr_draft.trim().to_string(),
                                            ));
                                        }
                                        if !works.is_empty() {
                                            let target_name = state
                                                .alias_target
                                                .and_then(|id| {
                                                    works.iter().find(|w| w.label == id)
                                                })
                                                .map(|w| w.name.as_str())
                                                .unwrap_or("Elegir obra…");
                                            egui::ComboBox::from_id_salt("inspector_alias_target")
                                                .selected_text(target_name)
                                                .width(140.0)
                                                .show_ui(ui, |ui| {
                                                    for w in works {
                                                        ui.selectable_value(
                                                            &mut state.alias_target,
                                                            Some(w.label),
                                                            &w.name,
                                                        );
                                                    }
                                                });
                                            let can_alias =
                                                draft_ready && state.alias_target.is_some();
                                            if ui
                                                .add_enabled(
                                                    can_alias,
                                                    egui::Button::new("Añadir alias"),
                                                )
                                                .clicked()
                                                && let Some(target) = state.alias_target
                                            {
                                                outcome.add_alias_from_ocr = Some((
                                                    entry.asset,
                                                    target,
                                                    state.ocr_draft.trim().to_string(),
                                                ));
                                            }
                                        }
                                    });
                                }
                            }
                            if !status_bar_ocr.is_empty() {
                                ui.add_space(theme::SPACE_XS);
                                ui.label(
                                    RichText::new("Barra de estado (no se busca)")
                                        .size(theme::FONT_SMALL)
                                        .color(palette.text_hint.color32()),
                                );
                                Frame::new()
                                    .fill(palette.bg_canvas.color32())
                                    .stroke(Stroke::new(1.0, palette.border.color32()))
                                    .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                                    .inner_margin(Margin::same(theme::SPACE_SM as i8))
                                    .show(ui, |ui| {
                                        ui.set_min_width(ui.available_width());
                                        for line in &status_bar_ocr {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&line.text)
                                                        .size(theme::FONT_SMALL)
                                                        .color(palette.text_secondary.color32()),
                                                )
                                                .selectable(true),
                                            );
                                        }
                                    });
                            }
                        }
                    });

                ui.add_space(theme::SPACE_SM);

                // Section 3: Etiquetas
                let labels_title = if state.labels.is_empty() {
                    "Etiquetas".to_string()
                } else {
                    format!("Etiquetas ({})", state.labels.len())
                };
                egui::CollapsingHeader::new(RichText::new(labels_title).strong())
                    .id_salt("inspector_sec_labels")
                    .default_open(true)
                    .show(ui, |ui| {
                        if state.labels.is_empty() {
                            ui.label(
                                RichText::new("Sin etiquetas asignadas")
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                        } else {
                            for assign in &state.labels {
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
                                            let src_text = if assign.source == Source::User {
                                                "Usuario".to_string()
                                            } else {
                                                format!(
                                                    "{} · {:.0}%",
                                                    source_label(assign.source),
                                                    assign.confidence * 100.0
                                                )
                                            };
                                            let lbl = ui.label(
                                                RichText::new(src_text)
                                                    .size(theme::FONT_SMALL)
                                                    .color(palette.text_secondary.color32()),
                                            );
                                            if assign.source == Source::Ai {
                                                lbl.on_hover_text(
                                                    "Sugerencia automática de IA. El número solo ordena Sugerencias; no es una probabilidad",
                                                );
                                            }
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    if assign.status != Status::Rejected
                                                        && ui.button("Rechazar").clicked()
                                                    {
                                                        outcome.reject_action =
                                                            Some((entry.asset, assign.label));
                                                    }
                                                    if assign.status != Status::Accepted
                                                        && ui.button("Aceptar").clicked()
                                                    {
                                                        outcome.accept_action =
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
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("new_label_kind")
                                .selected_text(kind_label(state.new_label_kind))
                                .width(92.0)
                                .show_ui(ui, |ui| {
                                    for k in [LabelKind::Tag, LabelKind::Category, LabelKind::Entity]
                                    {
                                        ui.selectable_value(
                                            &mut state.new_label_kind,
                                            k,
                                            kind_label(k),
                                        );
                                    }
                                });
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut state.new_label_text)
                                    .id(Id::unique(NEW_LABEL_INPUT_ID))
                                    .desired_width((ui.available_width() - 72.0).max(80.0))
                                    .hint_text(theme::hint_text(palette, "Nueva etiqueta")),
                            );
                            if state.focus_new_label_next_frame {
                                edit.request_focus();
                                state.focus_new_label_next_frame = false;
                            }
                            let submit = ui.button("Añadir").clicked()
                                || (edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                            if submit {
                                let name = state.new_label_text.trim().to_owned();
                                if !name.is_empty() {
                                    outcome.add_user_label =
                                        Some((entry.asset, state.new_label_kind, name));
                                    state.new_label_text.clear();
                                }
                            }
                        });
                    });
            });
        });

    outcome
}

fn meta_row(ui: &mut egui::Ui, palette: &Palette, key: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("{key}:"))
                .size(theme::FONT_SMALL)
                .color(palette.text_secondary.color32()),
        );
        ui.label(RichText::new(value).size(theme::FONT_SMALL));
    });
}

fn kind_label(kind: LabelKind) -> &'static str {
    match kind {
        LabelKind::Category => "Categoría",
        LabelKind::Tag => "Etiqueta",
        LabelKind::Entity => "Entidad",
    }
}

fn source_label(source: Source) -> &'static str {
    match source {
        Source::Ai => "IA",
        Source::Ocr => "OCR",
        Source::Rules => "Reglas",
        Source::User => "Usuario",
    }
}
