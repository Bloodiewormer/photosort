use crate::model::count_phrase;
use crate::review_flow::{ReviewDecision, ReviewFlow, THRESHOLD_MAX, THRESHOLD_MIN, UndoOutcome};
use crate::theme::{self, Palette};
use crate::worker::{ThumbPool, ThumbRequest};
use catalog::{Catalog, LabelKind, Source};
use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Key, Layout, Margin, Modifiers, RichText,
    ScrollArea, Sense, Stroke, TextureHandle, Vec2,
};
use std::collections::{HashMap, HashSet};

const REVIEW_THUMB_SIZE: Vec2 = Vec2::new(140.0, 104.0);
const GROUPS_PANEL_WIDTH: f32 = 264.0;

#[derive(Debug, Default)]
pub struct ReviewViewOutcome {
    pub catalog_mutated: bool,
    pub status_notice: Option<String>,
    pub error: Option<String>,
    pub thumbs_ready: bool,
}

/// Renders the review queue tab: confidence threshold slider, label groups with counts,
/// per-row thumbnails/metadata/Accept/Reject buttons, confirmed group actions, and Ctrl+Z undo.
pub fn draw(
    ui: &mut egui::Ui,
    palette: &Palette,
    flow: &mut ReviewFlow,
    catalog: Option<&Catalog>,
    thumb_pool: &ThumbPool,
    thumb_textures: &HashMap<String, TextureHandle>,
    thumb_failed: &HashSet<String>,
) -> ReviewViewOutcome {
    let mut outcome = ReviewViewOutcome::default();
    let mut visible_hashes: HashSet<String> = HashSet::new();
    let mut to_request: Vec<ThumbRequest> = Vec::new();

    // Keyboard shortcuts when the Review tab is active and no text input has focus.
    let mut kb_decision: Option<ReviewDecision> = None;
    let mut kb_undo = false;
    let ctx = ui.ctx().clone();
    if !ctx.egui_wants_keyboard_input() {
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::COMMAND, Key::Z) {
                kb_undo = true;
            }
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                flow.cancel_group_action();
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                flow.move_prev();
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                flow.move_next();
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                flow.move_group_prev();
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                flow.move_group_next();
            }
            if flow.pending_group_action().is_none() {
                if i.consume_key(Modifiers::NONE, Key::A) {
                    kb_decision = Some(ReviewDecision::Accept);
                } else if i.consume_key(Modifiers::NONE, Key::R) {
                    kb_decision = Some(ReviewDecision::Reject);
                }
            }
        });
    }

    if kb_undo && let Some(cat) = catalog {
        apply_undo(flow, cat, &mut outcome);
    } else if let Some(decision) = kb_decision
        && let Some(cat) = catalog
    {
        match flow.decide_current(cat, decision) {
            Ok(true) => outcome.catalog_mutated = true,
            Ok(false) => {}
            Err(e) => outcome.error = Some(format!("No se pudo guardar la decisión: {e}")),
        }
    }

    // Left panel: groups by label with counts.
    let mut selected_group = None;
    egui::Panel::left("review_groups_panel")
        .default_size(GROUPS_PANEL_WIDTH)
        .size_range(210.0..=360.0)
        .resizable(true)
        .frame(
            Frame::new()
                .fill(palette.bg_panel.color32())
                .inner_margin(Margin::same(theme::SPACE_MD as i8))
                .stroke(Stroke::new(1.0, palette.border.color32())),
        )
        .show(ui, |ui| {
            ui.label(
                RichText::new("Grupos por etiqueta")
                    .size(theme::FONT_HEADING)
                    .strong(),
            );
            ui.add_space(theme::SPACE_XS);
            ui.label(
                RichText::new(count_phrase(
                    flow.total_items(),
                    "sugerencia pendiente",
                    "sugerencias pendientes",
                ))
                .size(theme::FONT_SMALL)
                .color(palette.text_secondary.color32()),
            );
            ui.add_space(theme::SPACE_SM);

            if flow.groups().is_empty() {
                ui.label(
                    RichText::new("Sin grupos pendientes.")
                        .size(theme::FONT_SMALL)
                        .color(palette.text_secondary.color32()),
                );
                return;
            }

            let active_id = flow.active_group_label();
            ScrollArea::vertical().show(ui, |ui| {
                for group in flow.groups() {
                    let is_active = active_id == Some(group.label);
                    let bg = if is_active {
                        palette.accent_bg.color32()
                    } else {
                        palette.bg_card.color32()
                    };
                    let stroke = if is_active {
                        Stroke::new(theme::FOCUS_STROKE_WIDTH, palette.accent.color32())
                    } else {
                        Stroke::new(1.0, palette.border.color32())
                    };
                    let label_text = format!(
                        "{} · {} ({})",
                        kind_label(group.kind),
                        group.name,
                        group.count()
                    );
                    let btn = egui::Button::new(
                        RichText::new(label_text)
                            .size(theme::FONT_BODY)
                            .color(palette.text_primary.color32()),
                    )
                    .fill(bg)
                    .stroke(stroke)
                    .min_size(Vec2::new(ui.available_width(), 34.0));
                    if ui.add(btn).clicked() {
                        selected_group = Some(group.label);
                    }
                    ui.add_space(theme::SPACE_XS);
                }
            });
        });

    if let Some(label) = selected_group {
        flow.select_group(label);
    }

    // Central panel: threshold slider, group actions + confirmation, and item rows.
    let mut row_action: Option<(usize, ReviewDecision)> = None;
    let mut row_clicked: Option<usize> = None;
    let mut request_group: Option<ReviewDecision> = None;
    let mut confirm_group = false;
    let mut cancel_group = false;
    let mut undo_clicked = false;
    let mut threshold_changed = false;

    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(palette.bg_canvas.color32())
                .inner_margin(Margin::same(theme::SPACE_MD as i8)),
        )
        .show(ui, |ui| {
            // Controls bar
            Frame::new()
                .fill(palette.bg_panel.color32())
                .stroke(Stroke::new(1.0, palette.border.color32()))
                .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                .inner_margin(Margin::same(theme::SPACE_MD as i8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Cola de revisión")
                                .size(theme::FONT_HEADING)
                                .strong(),
                        );
                        ui.separator();
                        let mut threshold = flow.threshold();
                        let slider = egui::Slider::new(&mut threshold, THRESHOLD_MIN..=THRESHOLD_MAX)
                            .fixed_decimals(2)
                            .text("Umbral de confianza");
                        if ui.add(slider).changed() && flow.set_threshold(threshold) {
                            threshold_changed = true;
                        }
                        ui.separator();
                        let undo_btn = ui.add_enabled(
                            flow.can_undo(),
                            egui::Button::new("Deshacer (Ctrl+Z)"),
                        );
                        if let Some(desc) = flow.last_undo_description() {
                            if undo_btn.on_hover_text(desc).clicked() {
                                undo_clicked = true;
                            }
                        } else if undo_btn.clicked() {
                            undo_clicked = true;
                        }
                    });
                    ui.add_space(theme::SPACE_XS);
                    ui.label(
                        RichText::new(
                            "Atajos: A aceptar · R rechazar · ↑/↓ mover fila · ←/→ cambiar grupo · Ctrl+Z deshacer",
                        )
                        .size(theme::FONT_SMALL)
                        .color(palette.text_secondary.color32()),
                    );
                });

            ui.add_space(theme::SPACE_MD);

            let Some(group) = flow.active_group() else {
                ui.vertical_centered(|ui| {
                    ui.add_space(theme::SPACE_XL * 2.0);
                    ui.label(
                        RichText::new(
                            "No hay sugerencias por debajo del umbral actual. Sube el umbral o pulsa \"Analizar\" para evaluar nuevas imágenes.",
                        )
                        .size(theme::FONT_BODY)
                        .color(palette.text_secondary.color32()),
                    );
                });
                return;
            };

            // Active group header & bulk actions
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{}: {} ({})",
                        kind_label(group.kind),
                        group.name,
                        group.count()
                    ))
                    .size(theme::FONT_HEADING)
                    .strong(),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Rechazar todo el grupo").clicked() {
                        request_group = Some(ReviewDecision::Reject);
                    }
                    if ui.button("Aceptar todo el grupo").clicked() {
                        request_group = Some(ReviewDecision::Accept);
                    }
                });
            });
            ui.add_space(theme::SPACE_SM);

            // Explicit confirmation banner before applying a bulk group action
            if let Some(pending) = flow.pending_group_action() {
                let verb = match pending.decision {
                    ReviewDecision::Accept => "aceptar",
                    ReviewDecision::Reject => "rechazar",
                };
                Frame::new()
                    .fill(palette.review_badge_bg.color32())
                    .stroke(Stroke::new(
                        theme::FOCUS_STROKE_WIDTH,
                        palette.accent.color32(),
                    ))
                    .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                    .inner_margin(Margin::same(theme::SPACE_SM as i8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "¿Confirmas {verb} {} del grupo \"{}\"?",
                                    count_phrase(pending.count, "sugerencia", "sugerencias"),
                                    pending.group_name
                                ))
                                .size(theme::FONT_BODY)
                                .strong()
                                .color(palette.review_badge_fg.color32()),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Cancelar").clicked() {
                                    cancel_group = true;
                                }
                                if ui.button("Confirmar").clicked() {
                                    confirm_group = true;
                                }
                            });
                        });
                    });
                ui.add_space(theme::SPACE_SM);
            }

            let cursor = flow.cursor();
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (idx, row) in group.items.iter().enumerate() {
                        visible_hashes.insert(row.sha256.clone());
                        if !thumb_textures.contains_key(&row.sha256)
                            && !thumb_failed.contains(&row.sha256)
                        {
                            to_request.push(ThumbRequest {
                                source: row.path.clone(),
                                sha256: row.sha256.clone(),
                            });
                        }

                        let is_current = idx == cursor;
                        let bg = if is_current {
                            palette.accent_bg.color32()
                        } else {
                            palette.bg_card.color32()
                        };
                        let stroke = if is_current {
                            Stroke::new(theme::FOCUS_STROKE_WIDTH, palette.accent.color32())
                        } else {
                            Stroke::new(1.0, palette.border.color32())
                        };

                        let card_resp = Frame::new()
                            .fill(bg)
                            .stroke(stroke)
                            .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                            .inner_margin(Margin::same(theme::SPACE_SM as i8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let (thumb_rect, thumb_resp) =
                                        ui.allocate_exact_size(REVIEW_THUMB_SIZE, Sense::click());
                                    if thumb_resp.clicked() {
                                        row_clicked = Some(idx);
                                    }
                                    draw_thumb_box(
                                        ui,
                                        palette,
                                        thumb_rect,
                                        thumb_textures.get(&row.sha256),
                                        thumb_failed.contains(&row.sha256),
                                    );
                                    ui.add_space(theme::SPACE_SM);

                                    ui.vertical(|ui| {
                                        let filename = row
                                            .path
                                            .file_name()
                                            .map(|n| n.to_string_lossy().into_owned())
                                            .unwrap_or_else(|| row.path.display().to_string());
                                        ui.label(
                                            RichText::new(filename)
                                                .size(theme::FONT_BODY)
                                                .strong(),
                                        );
                                        ui.label(
                                            RichText::new(row.path.display().to_string())
                                                .size(theme::FONT_SMALL)
                                                .color(palette.text_secondary.color32()),
                                        );
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label(
                                                RichText::new(format!(
                                                    "{}: {}",
                                                    kind_label(row.label_kind),
                                                    row.label_name
                                                ))
                                                .size(theme::FONT_SMALL)
                                                .strong(),
                                            );
                                            ui.separator();
                                            ui.label(
                                                RichText::new(format!(
                                                    "Confianza: {:.0}%",
                                                    row.confidence * 100.0
                                                ))
                                                .size(theme::FONT_SMALL)
                                                .color(palette.status_suggested_fg.color32()),
                                            );
                                            ui.separator();
                                            ui.label(
                                                RichText::new(format!(
                                                    "Fuente: {}",
                                                    source_label(row.source)
                                                ))
                                                .size(theme::FONT_SMALL)
                                                .color(palette.text_secondary.color32()),
                                            );
                                        });
                                    });

                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.button("Rechazar").clicked() {
                                            row_action = Some((idx, ReviewDecision::Reject));
                                        }
                                        if ui.button("Aceptar").clicked() {
                                            row_action = Some((idx, ReviewDecision::Accept));
                                        }
                                    });
                                });
                            });
                        if card_resp.response.interact(Sense::click()).clicked() {
                            row_clicked = Some(idx);
                        }
                        ui.add_space(theme::SPACE_SM);
                    }
                });
        });

    // Apply deferred UI actions outside borrows of `flow`.
    if let Some(idx) = row_clicked {
        flow.select_row(idx);
    }
    if threshold_changed
        && let Some(cat) = catalog
        && let Err(e) = flow.reload(cat)
    {
        outcome.error = Some(format!("No se pudo recargar la cola de revisión: {e}"));
    }
    if let Some(decision) = request_group {
        flow.request_group_action(decision);
    }
    if cancel_group {
        flow.cancel_group_action();
    }
    if confirm_group && let Some(cat) = catalog {
        match flow.confirm_group_action(cat) {
            Ok(count) if count > 0 => {
                outcome.catalog_mutated = true;
                outcome.status_notice = Some(format!(
                    "Acción aplicada a {}. Pulsa Ctrl+Z para deshacer.",
                    count_phrase(count, "sugerencia", "sugerencias")
                ));
            }
            Ok(_) => {}
            Err(e) => outcome.error = Some(format!("No se pudo aplicar la acción al grupo: {e}")),
        }
    }
    if let Some((idx, decision)) = row_action
        && let Some(cat) = catalog
    {
        match flow.decide_at(cat, idx, decision) {
            Ok(true) => outcome.catalog_mutated = true,
            Ok(false) => {}
            Err(e) => outcome.error = Some(format!("No se pudo guardar la decisión: {e}")),
        }
    }
    if undo_clicked && let Some(cat) = catalog {
        apply_undo(flow, cat, &mut outcome);
    }

    outcome.thumbs_ready = flow.active_group().is_none_or(|g| {
        g.items.is_empty() || (!visible_hashes.is_empty() && to_request.is_empty())
    });

    thumb_pool.set_visible(&visible_hashes);
    for req in to_request {
        thumb_pool.request(req);
    }

    outcome
}

fn apply_undo(flow: &mut ReviewFlow, cat: &Catalog, outcome: &mut ReviewViewOutcome) {
    match flow.undo(cat) {
        Ok(Some(UndoOutcome { restored, skipped })) => {
            if restored > 0 {
                outcome.catalog_mutated = true;
            }
            outcome.status_notice = Some(if skipped == 0 {
                format!(
                    "Deshecho: {}.",
                    count_phrase(restored, "sugerencia restaurada", "sugerencias restauradas")
                )
            } else if restored == 0 {
                "No se deshizo ningún cambio porque un análisis o decisión posterior ya modificó la etiqueta.".into()
            } else {
                format!(
                    "Deshecho parcialmente: {}, {} por cambios posteriores.",
                    count_phrase(restored, "restaurada", "restauradas"),
                    count_phrase(skipped, "omitida", "omitidas")
                )
            });
        }
        Ok(None) => {}
        Err(e) => outcome.error = Some(format!("No se pudo deshacer: {e}")),
    }
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
        let placeholder = if failed {
            "Sin miniatura"
        } else {
            "Cargando..."
        };
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
    egui::Rect::from_center_size(bounds.center(), size)
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
