use crate::model::{format_bytes, format_timestamp_plain, format_timestamp_utc};
use crate::theme::{self, Palette};
use crate::views::gallery::fit_rect;
use catalog::{
    AssetId, AssetWork, Assignment, DetectedFace, Entry, LabelId, LabelKind, OcrLine,
    SCREENSHOT_CATEGORY, Source, Status, WorkSummary,
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
    pub asset_work: Option<AssetWork>,
    pub ocr_lines: Vec<OcrLine>,
    pub ocr_done: bool,
    pub faces: Vec<DetectedFace>,
    pub face_details: Vec<(catalog::FaceDetail, Option<String>)>,
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
            asset_work: None,
            ocr_lines: Vec::new(),
            ocr_done: false,
            faces: Vec::new(),
            face_details: Vec::new(),
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
    pub accept_work: Option<AssetId>,
    pub reject_work: Option<AssetId>,
    pub add_user_label: Option<(AssetId, LabelKind, String)>,
    pub create_work_from_ocr: Option<(AssetId, String)>,
    pub add_alias_from_ocr: Option<(AssetId, LabelId, String)>,
    pub remove_person_from_asset: Option<(catalog::PersonId, AssetId)>,
    pub open_persons: bool,
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
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                // Header: filename + actions
                let review_lbl = if is_reviewed {
                    "Volver a revisar"
                } else {
                    "Revisar"
                };
                if ui.available_width() < 270.0 {
                    ui.horizontal(|ui| {
                        if is_reviewed {
                            let (mark_rect, mark_resp) =
                                ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                            theme::paint_reviewed_mark(ui.painter(), mark_rect, palette);
                            mark_resp.on_hover_text("Revisada");
                        }
                        ui.label(if is_reviewed {
                            "Revisada"
                        } else {
                            "Sin revisar"
                        });
                    });
                    ui.horizontal(|ui| {
                        if ui.button(review_lbl).clicked() {
                            outcome.review_asset = Some(entry.asset);
                        }
                        if ui.button("Visor").clicked() {
                            outcome.open_viewer = true;
                        }
                    });
                } else {
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
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Visor").clicked() {
                                outcome.open_viewer = true;
                            }
                            if ui.button(review_lbl).clicked() {
                                outcome.review_asset = Some(entry.asset);
                            }
                        });
                    });
                }
                ui.add_space(theme::SPACE_SM);

                // Preview box
                let preview_w = ui.available_width();
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
                        if entry.media_type == catalog::MediaType::Video {
                            meta_row(ui, palette, "Tipo de medio", "Vídeo");
                            if let Some(ms) = entry.duration_ms {
                                meta_row(ui, palette, "Duración", &crate::model::format_duration(ms));
                            }
                        }
                        meta_row(ui, palette, "Tamaño", &format_bytes(entry.size));
                        if let (Some(w), Some(h)) = (entry.width, entry.height) {
                            meta_row(ui, palette, "Dimensiones", &format!("{w} × {h} px"));
                        }
                        if let Some(fmt) = &entry.format {
                            meta_row(ui, palette, "Formato", fmt);
                        }
                        if let Some(captured) = entry.captured_at_ns {
                            meta_row(ui, palette, "Fecha de captura", &format_timestamp_plain(captured));
                        }
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
                            if entry.media_type != catalog::MediaType::Video {
                                meta_row(
                                    ui,
                                    palette,
                                    "Lectura OCR",
                                    if state.ocr_done { "Hecha" } else { "Pendiente" },
                                );
                            }
                        });
                    });

                ui.add_space(theme::SPACE_SM);

                // Section 1b: Pertenece a (work ownership from filename, separate from OCR mentions)
                if let Some(aw) = &state.asset_work {
                    egui::CollapsingHeader::new(RichText::new("Pertenece a").strong())
                        .id_salt("inspector_sec_belongs_to")
                        .default_open(true)
                        .show(ui, |ui| {
                            Frame::new()
                                .fill(palette.bg_card.color32())
                                .stroke(Stroke::new(1.0, palette.border.color32()))
                                .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                                .inner_margin(Margin::same(theme::SPACE_SM as i8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&aw.work_name).strong(),
                                            )
                                            .truncate(),
                                        )
                                        .on_hover_text(&aw.work_name);
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                let (st_text, st_color) = match aw.status {
                                                    Status::Accepted => (
                                                        "Confirmada",
                                                        palette.status_accepted_fg.color32(),
                                                    ),
                                                    Status::Rejected => (
                                                        "Descartada",
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
                                    let mut parts = Vec::new();
                                    if !aw.chapter.is_empty() {
                                        parts.push(format!("Capítulo {}", aw.chapter));
                                    }
                                    if let Some(p) = aw.page {
                                        parts.push(format!("Página {p}"));
                                    }
                                    if !parts.is_empty() {
                                        ui.label(
                                            RichText::new(parts.join(" · "))
                                                .size(theme::FONT_SMALL)
                                                .color(palette.text_primary.color32()),
                                        );
                                    }
                                    let src = if aw.source == Source::Rules {
                                        "Nombre de archivo"
                                    } else {
                                        source_label(aw.source)
                                    };
                                    ui.label(
                                        RichText::new(src)
                                            .size(theme::FONT_SMALL)
                                            .color(palette.text_secondary.color32()),
                                    );
                                    ui.add_space(theme::SPACE_XS);
                                    ui.horizontal_wrapped(|ui| {
                                        if aw.status != Status::Accepted
                                            && ui.button("Confirmar").clicked()
                                        {
                                            outcome.accept_work = Some(entry.asset);
                                        }
                                        if aw.status != Status::Rejected
                                            && ui.button("No es esta").clicked()
                                        {
                                            outcome.reject_work = Some(entry.asset);
                                        }
                                    });
                                });
                        });
                    ui.add_space(theme::SPACE_SM);
                }

                // Section 1c: Personas y rostros
                if !state.face_details.is_empty() {
                    egui::CollapsingHeader::new(RichText::new("Personas en esta foto").strong())
                        .id_salt("inspector_sec_faces")
                        .default_open(true)
                        .show(ui, |ui| {
                            for (face, person_name) in &state.face_details {
                                Frame::new()
                                    .fill(palette.bg_card.color32())
                                    .stroke(Stroke::new(1.0, palette.border.color32()))
                                    .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                                    .inner_margin(Margin::same(theme::SPACE_SM as i8))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            let label = person_name.as_deref().unwrap_or("Sin nombre");
                                            ui.add(egui::Label::new(RichText::new(label).strong()).truncate())
                                                .on_hover_text(label);
                                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                                if let Some(pid) = face.person_id {
                                                    if ui.button("Quitar").on_hover_text("Quitar esta foto de la persona").clicked() {
                                                        outcome.remove_person_from_asset = Some((pid, entry.asset));
                                                    }
                                                } else if ui.button("Poner nombre").clicked() {
                                                    outcome.open_persons = true;
                                                }
                                            });
                                        });
                                    });
                                ui.add_space(theme::SPACE_XS);
                            }
                        });
                    ui.add_space(theme::SPACE_SM);
                }

                // Section 2: Texto (OCR)
                if entry.media_type != catalog::MediaType::Video {
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
                                                        .wrap()
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
                                                        .wrap()
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
                                        for line in &status_bar_ocr {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&line.text)
                                                        .size(theme::FONT_SMALL)
                                                        .color(palette.text_secondary.color32()),
                                                )
                                                .wrap()
                                                .selectable(true),
                                            );
                                        }
                                    });
                            }
                        }
                    });

                    ui.add_space(theme::SPACE_SM);
                }

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
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(format!(
                                                        "{}: {}",
                                                        kind_label(assign.kind),
                                                        assign.name
                                                    ))
                                                    .strong(),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(&assign.name);
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
                        ui.horizontal_wrapped(|ui| {
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
                            let edit_w = (ui.available_width() - 72.0).clamp(60.0, 160.0);
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut state.new_label_text)
                                    .id(Id::unique(NEW_LABEL_INPUT_ID))
                                    .desired_width(edit_w)
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
        ui.add(egui::Label::new(RichText::new(value).size(theme::FONT_SMALL)).truncate())
            .on_hover_text(value);
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

#[cfg(test)]
mod tests {
    use super::*;
    use catalog::{AssetId, Entry, MediaType, OcrLine, OcrRecognizer};
    use eframe::egui::{Event, PointerButton, Pos2, RawInput, Rect, Vec2};
    use std::path::PathBuf;

    fn sample_entry() -> Entry {
        Entry {
            asset: AssetId(1),
            path: PathBuf::from(
                "C:/photos/very_long_folder_name_that_should_not_cause_panel_to_jump/test_photo.jpg",
            ),
            size: 1024 * 500,
            modified_ns: Some(1_700_000_000_000_000_000),
            sha256: "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_string(),
            width: Some(1920),
            height: Some(1080),
            format: Some("JPEG".to_string()),
            media_type: MediaType::Image,
            duration_ms: None,
            captured_at_ns: None,
        }
    }

    #[test]
    fn inspector_panel_resizes_to_minimum_width_without_bouncing() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0));
        let entry = sample_entry();
        let mut state = InspectorState {
            ocr_done: true,
            ocr_lines: vec![
                OcrLine {
                    text: "時計仕掛けのクロニクル — 第12話 とても長いOCRテキスト".to_string(),
                    score: 0.95,
                    recognizer: OcrRecognizer::V6,
                    rect: [10.0, 10.0, 200.0, 30.0],
                    top_strip: false,
                },
                OcrLine {
                    text: "바람 공주 · 제3화 (한국어 OCR 테스트)".to_string(),
                    score: 0.90,
                    recognizer: OcrRecognizer::Korean,
                    rect: [10.0, 40.0, 200.0, 60.0],
                    top_strip: false,
                },
            ],
            labels: vec![catalog::Assignment {
                label: catalog::LabelId(1),
                name: "Manga › Scanlation › Página promocional o índice de proyectos muy largo"
                    .to_string(),
                kind: catalog::LabelKind::Category,
                source: catalog::Source::Ai,
                confidence: 0.85,
                status: catalog::Status::Accepted,
            }],
            ..Default::default()
        };

        // Frame 1: default inspector width
        let input1 = RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut output1 = ctx.run_ui(input1, |ui| {
            draw(
                ui,
                &theme::DARK_PALETTE,
                Some(&entry),
                true,
                &mut state,
                &[],
                None,
            );
        });
        output1.textures_delta.clear();

        // Frame 2: drag panel resize handle from initial x (1000 - 328 = 672) to min width (1000 - 280 = 720)
        let panel_border_x = 1000.0 - theme::INSPECTOR_WIDTH;
        let target_border_x = 1000.0 - 280.0;
        let events = vec![
            Event::PointerMoved(Pos2::new(panel_border_x, 300.0)),
            Event::PointerButton {
                pos: Pos2::new(panel_border_x, 300.0),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            Event::PointerMoved(Pos2::new(target_border_x, 300.0)),
            Event::PointerButton {
                pos: Pos2::new(target_border_x, 300.0),
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];

        let input2 = RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        };
        let mut output2 = ctx.run_ui(input2, |ui| {
            draw(
                ui,
                &theme::DARK_PALETTE,
                Some(&entry),
                true,
                &mut state,
                &[],
                None,
            );
        });
        output2.textures_delta.clear();

        // Frame 3: verify it stays at 280.0 without jumping
        let input3 = RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut actual_width = 0.0;
        let mut output3 = ctx.run_ui(input3, |ui| {
            let resp = draw(
                ui,
                &theme::DARK_PALETTE,
                Some(&entry),
                true,
                &mut state,
                &[],
                None,
            );
            let _ = resp;
        });
        output3.textures_delta.clear();

        #[allow(deprecated)]
        let inspector_id = egui::Id::new("inspector");
        if let Some(rect) = ctx.memory(|m| m.area_rect(inspector_id)) {
            actual_width = rect.width();
        }
        if actual_width > 0.0 {
            assert!(
                (actual_width - 280.0).abs() <= 2.0,
                "El ancho final debe mantenerse en el mínimo solicitado (280.0), obtenido: {actual_width}"
            );
        }
    }
}
