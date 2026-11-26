use crate::model::{GridLayout, Selection, count_phrase, gallery_cell_tooltip, selected_phrase};
use crate::theme::{self, Palette};
use crate::worker::{ThumbPool, ThumbRequest};
use catalog::{AssetId, Entry, Sort};
use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Id, Layout, Margin, Pos2, Rect, RichText,
    ScrollArea, Sense, Stroke, TextureHandle, Vec2,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub const SUGGESTIONS_BADGE_TOOLTIP: &str = "Tiene sugerencias pendientes";

pub struct SimilarityView {
    pub title: String,
    pub threshold: Option<f32>,
    pub scores: HashMap<AssetId, f32>,
}

#[derive(Default)]
pub struct GalleryOutcome {
    pub selection_changed: bool,
    pub sort_changed: bool,
    pub exit_similarity: bool,
    pub open_viewer: bool,
    pub review_selected: bool,
    pub open_export: bool,
    pub open_photo_menu: Option<(usize, Pos2)>,
    pub thumbs_ready: bool,
    pub last_columns: usize,
    pub primary_cell_pos: Option<Pos2>,
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    ui: &mut egui::Ui,
    palette: &Palette,
    entries: &[Entry],
    selection: &mut Selection,
    sort: &mut Sort,
    similarity_view: Option<&SimilarityView>,
    reviewed_assets: &HashSet<AssetId>,
    pending_suggestions_assets: &HashSet<AssetId>,
    has_indexed_locations: bool,
    thumb_pool: &ThumbPool,
    thumb_textures: &HashMap<String, TextureHandle>,
    thumb_failed: &HashSet<String>,
) -> GalleryOutcome {
    let mut outcome = GalleryOutcome {
        last_columns: 1,
        ..Default::default()
    };
    let mut visible_hashes: HashSet<String> = HashSet::new();
    let mut to_request: Vec<ThumbRequest> = Vec::new();

    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(palette.bg_canvas.color32())
                .inner_margin(Margin::same(theme::SPACE_LG as i8)),
        )
        .show(ui, |ui| {
            // Optional similarity banner (from "Buscar parecidas", "Probar", or visual search)
            if let Some(sim) = similarity_view {
                let passing = sim.threshold.map(|t| {
                    entries
                        .iter()
                        .filter(|e| sim.scores.get(&e.asset).is_some_and(|&s| s >= t))
                        .count()
                });
                Frame::new()
                    .fill(palette.bg_panel.color32())
                    .stroke(Stroke::new(1.0, palette.accent.color32()))
                    .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                    .inner_margin(Margin::symmetric(
                        theme::SPACE_MD as i8,
                        theme::SPACE_SM as i8,
                    ))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&sim.title).strong());
                            let info = match (sim.threshold, passing) {
                                (Some(t), Some(n)) => format!(
                                    "· Similitud coseno (no es probabilidad) · Umbral {t:.3}: {} superan el umbral",
                                    count_phrase(n, "imagen", "imágenes")
                                ),
                                _ => "· Ordenado por similitud coseno (no es probabilidad)"
                                    .to_string(),
                            };
                            ui.label(
                                RichText::new(info)
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Volver a la galería").clicked() {
                                    outcome.exit_similarity = true;
                                }
                            });
                        });
                    });
                ui.add_space(theme::SPACE_MD);
            }

            // Gallery header: count and sort
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(count_phrase(entries.len(), "foto", "fotos"))
                        .size(theme::FONT_BODY)
                        .color(palette.text_secondary.color32()),
                );
                if similarity_view.is_none() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let prev_sort = *sort;
                        egui::ComboBox::from_id_salt("gallery_sort_combo")
                            .width(140.0)
                            .selected_text(sort_label(*sort))
                            .show_ui(ui, |ui| {
                                for option in [
                                    Sort::PathAsc,
                                    Sort::PathDesc,
                                    Sort::SizeAsc,
                                    Sort::SizeDesc,
                                    Sort::ModifiedAsc,
                                    Sort::ModifiedDesc,
                                ] {
                                    ui.selectable_value(sort, option, sort_label(option));
                                }
                            });
                        if *sort != prev_sort {
                            outcome.sort_changed = true;
                        }
                        ui.label(
                            RichText::new("Orden")
                                .size(theme::FONT_SMALL)
                                .color(palette.text_hint.color32()),
                        );
                    });
                }
            });
            ui.add_space(theme::SPACE_SM);

            // Contextual selection bar (appears ONLY while something is selected)
            if !selection.is_empty() {
                let all_reviewed = selection
                    .selected_indices()
                    .into_iter()
                    .filter_map(|i| entries.get(i))
                    .all(|e| reviewed_assets.contains(&e.asset));
                let review_btn_text = if all_reviewed {
                    "Volver a revisar"
                } else {
                    "Revisar"
                };

                Frame::new()
                    .fill(palette.bg_panel.color32())
                    .stroke(Stroke::new(1.0, palette.border.color32()))
                    .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                    .inner_margin(Margin::symmetric(
                        theme::SPACE_MD as i8,
                        theme::SPACE_SM as i8,
                    ))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                RichText::new(selected_phrase(selection.len()))
                                    .strong()
                                    .color(palette.text_primary.color32()),
                            );
                            ui.add_space(theme::SPACE_XS);
                            if ui
                                .add(theme::primary_button(palette, review_btn_text))
                                .clicked()
                            {
                                outcome.review_selected = true;
                            }
                            if ui.button("Exportar…").clicked() {
                                outcome.open_export = true;
                            }
                            if ui.button("Quitar selección").clicked() {
                                selection.clear();
                                outcome.selection_changed = true;
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new("Clic derecho para más opciones")
                                        .size(theme::FONT_SMALL)
                                        .color(palette.text_hint.color32()),
                                );
                            });
                        });
                    });
                ui.add_space(theme::SPACE_MD);
            }

            if entries.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(theme::SPACE_2XL * 2.0);
                    let msg = if !has_indexed_locations {
                        "Aún no hay fotos indexadas. Activa la revisión o busca cambios en Configuración"
                    } else {
                        "Ninguna foto coincide con los filtros actuales"
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
                entries.len(),
            );
            outcome.last_columns = layout.columns;

            let grid_rect = ui.available_rect_before_wrap();
            let mut any_cell_secondary_clicked = false;

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
                                let entry = &entries[idx];
                                visible_hashes.insert(entry.sha256.clone());
                                if !thumb_textures.contains_key(&entry.sha256)
                                    && !thumb_failed.contains(&entry.sha256)
                                {
                                    to_request.push(ThumbRequest {
                                        source: entry.path.clone(),
                                        sha256: entry.sha256.clone(),
                                    });
                                }

                                let selected = selection.contains(idx);
                                let is_primary = selection.primary() == Some(idx);
                                let is_reviewed = reviewed_assets.contains(&entry.asset);
                                let has_suggestions =
                                    pending_suggestions_assets.contains(&entry.asset);

                                let (rect, resp) = ui.allocate_exact_size(
                                    Vec2::new(theme::CELL_WIDTH, theme::CELL_HEIGHT),
                                    Sense::click(),
                                );
                                if is_primary {
                                    outcome.primary_cell_pos = Some(rect.center());
                                }
                                if resp.clicked() {
                                    let mods = ui.input(|i| i.modifiers);
                                    if mods.shift {
                                        selection.shift_click(idx, entries.len());
                                    } else if mods.command {
                                        selection.ctrl_click(idx, entries.len());
                                    } else {
                                        selection.click(idx, entries.len());
                                    }
                                    outcome.selection_changed = true;
                                }
                                if resp.secondary_clicked() {
                                    any_cell_secondary_clicked = true;
                                    selection.right_click(idx, entries.len());
                                    outcome.selection_changed = true;
                                    let pos = ui
                                        .input(|i| i.pointer.interact_pos())
                                        .unwrap_or_else(|| rect.center());
                                    outcome.open_photo_menu = Some((idx, pos));
                                }
                                if resp.double_clicked() {
                                    selection.click(idx, entries.len());
                                    outcome.open_viewer = true;
                                    outcome.selection_changed = true;
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

                                let thumb_rect = Rect::from_min_size(
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

                                if let Some(tex) = thumb_textures.get(&entry.sha256) {
                                    let fit = fit_rect(thumb_rect, tex.size_vec2());
                                    painter.image(
                                        tex.id(),
                                        fit,
                                        Rect::from_min_max(
                                            egui::pos2(0.0, 0.0),
                                            egui::pos2(1.0, 1.0),
                                        ),
                                        Color32::WHITE,
                                    );
                                } else {
                                    let placeholder = if thumb_failed.contains(&entry.sha256) {
                                        "Sin miniatura"
                                    } else {
                                        "Cargando…"
                                    };
                                    painter.text(
                                        thumb_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        placeholder,
                                        egui::FontId::proportional(theme::FONT_SMALL),
                                        palette.text_secondary.color32(),
                                    );
                                }

                                // Top-left: pending suggestions badge (renamed from "Revisar")
                                let badge_rect = has_suggestions.then(|| {
                                    let r = Rect::from_min_size(
                                        thumb_rect.min + Vec2::splat(theme::SPACE_XS),
                                        Vec2::new(84.0, 20.0),
                                    );
                                    painter.rect_filled(
                                        r,
                                        CornerRadius::same(theme::RADIUS_SM),
                                        palette.review_badge_bg.color32(),
                                    );
                                    painter.text(
                                        r.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "Sugerencias",
                                        egui::FontId::proportional(theme::FONT_SMALL),
                                        palette.review_badge_fg.color32(),
                                    );
                                    r
                                });

                                // Top-right: signature ochre grease-pencil loop for reviewed photos
                                let reviewed_rect = is_reviewed.then(|| {
                                    let r = Rect::from_min_size(
                                        Pos2::new(
                                            thumb_rect.max.x - 24.0 - theme::SPACE_XS,
                                            thumb_rect.min.y + theme::SPACE_XS,
                                        ),
                                        Vec2::splat(24.0),
                                    );
                                    theme::paint_reviewed_mark(painter, r, palette);
                                    r
                                });

                                // Bottom-right: similarity score when in similarity mode
                                if let Some(sim) = similarity_view
                                    && let Some(&score) = sim.scores.get(&entry.asset)
                                {
                                    let passes = sim.threshold.is_none_or(|t| score >= t);
                                    let (s_bg, s_fg, prefix) = match sim.threshold {
                                        Some(_) if passes => (
                                            palette.chip_require_bg.color32(),
                                            palette.chip_require_fg.color32(),
                                            "✓ ",
                                        ),
                                        Some(_) => (
                                            palette.bg_panel.color32(),
                                            palette.text_secondary.color32(),
                                            "",
                                        ),
                                        None => (
                                            palette.accent_bg.color32(),
                                            palette.text_primary.color32(),
                                            "",
                                        ),
                                    };
                                    let score_rect = Rect::from_min_size(
                                        Pos2::new(
                                            thumb_rect.max.x - 62.0 - theme::SPACE_XS,
                                            thumb_rect.max.y - 20.0 - theme::SPACE_XS,
                                        ),
                                        Vec2::new(62.0, 20.0),
                                    );
                                    painter.rect_filled(
                                        score_rect,
                                        CornerRadius::same(theme::RADIUS_SM),
                                        s_bg,
                                    );
                                    painter.text(
                                        score_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        format!("{prefix}{score:.3}"),
                                        egui::FontId::monospace(theme::FONT_SMALL),
                                        s_fg,
                                    );
                                }

                                let filename = entry
                                    .path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| entry.path.display().to_string());
                                let label_rect = Rect::from_min_max(
                                    Pos2::new(
                                        rect.min.x + theme::SPACE_SM,
                                        thumb_rect.max.y + theme::SPACE_XS,
                                    ),
                                    Pos2::new(
                                        rect.max.x - theme::SPACE_SM,
                                        rect.max.y - theme::SPACE_XS,
                                    ),
                                );
                                let mut child_ui = ui
                                    .new_child(egui::UiBuilder::new().max_rect(label_rect));
                                child_ui.add(
                                    egui::Label::new(
                                        RichText::new(&filename)
                                            .size(theme::FONT_SMALL)
                                            .color(palette.text_primary.color32()),
                                    )
                                    .truncate()
                                    .sense(Sense::empty()),
                                );

                                let a11y_label = if is_reviewed {
                                    format!("{filename} · Revisada")
                                } else {
                                    filename
                                };
                                resp.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::Role::Button,
                                        true,
                                        selected,
                                        a11y_label.clone(),
                                    )
                                });

                                if reviewed_rect.is_some_and(|r| ui.rect_contains_pointer(r)) {
                                    resp.on_hover_text("Revisada");
                                } else if badge_rect.is_some_and(|r| ui.rect_contains_pointer(r)) {
                                    resp.on_hover_text(SUGGESTIONS_BADGE_TOOLTIP);
                                } else {
                                    let entry_path = &entry.path;
                                    resp.on_hover_ui(|ui| {
                                        ui.label(gallery_cell_tooltip(entry_path));
                                    });
                                }
                            }
                        });
                    }
                },
            );

            // Empty gallery area context menu (when right-clicking background, not a cell)
            let bg_resp = ui.interact(
                grid_rect,
                Id::unique("gallery_empty_area_menu"),
                Sense::click(),
            );
            if !any_cell_secondary_clicked {
                bg_resp.context_menu(|ui| {
                    if ui.button("Seleccionar todo").clicked() {
                        selection.select_all(entries.len());
                        outcome.selection_changed = true;
                        ui.close();
                    }
                    if ui.button("Invertir selección").clicked() {
                        selection.invert(entries.len());
                        outcome.selection_changed = true;
                        ui.close();
                    }
                });
            }
        });

    outcome.thumbs_ready = !visible_hashes.is_empty() && to_request.is_empty();
    thumb_pool.set_visible(&visible_hashes);
    for req in to_request {
        thumb_pool.request(req);
    }
    outcome
}

/// Fullscreen read-only image viewer overlay.
pub fn draw_viewer_overlay(
    ctx: &egui::Context,
    palette: &Palette,
    viewer_open: &mut bool,
    entries: &[Entry],
    selection: &mut Selection,
    preview_loaded: Option<&(PathBuf, Result<TextureHandle, String>)>,
) -> bool {
    if !*viewer_open {
        return false;
    }
    let Some(idx) = selection.primary() else {
        *viewer_open = false;
        return false;
    };
    let Some(entry) = entries.get(idx).cloned() else {
        *viewer_open = false;
        return false;
    };

    let mut selection_changed = false;
    let screen = ctx.content_rect();
    egui::Area::new(Id::unique("image_viewer_overlay"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            Frame::new()
                .fill(Color32::from_rgba_unmultiplied(15, 15, 14, 244))
                .inner_margin(Margin::same(theme::SPACE_LG as i8))
                .show(ui, |ui| {
                    ui.set_min_size(screen.size() - Vec2::splat(theme::SPACE_LG * 2.0));
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} / {} — {}",
                                idx + 1,
                                entries.len(),
                                entry.path.display()
                            ))
                            .color(theme::DARK_PALETTE.text_primary.color32()),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Cerrar").clicked() {
                                *viewer_open = false;
                            }
                            if ui.button("Siguiente").clicked() {
                                selection.move_cursor(
                                    crate::model::MoveDir::Right,
                                    1,
                                    entries.len(),
                                    false,
                                );
                                selection_changed = true;
                            }
                            if ui.button("Anterior").clicked() {
                                selection.move_cursor(
                                    crate::model::MoveDir::Left,
                                    1,
                                    entries.len(),
                                    false,
                                );
                                selection_changed = true;
                            }
                        });
                    });
                    ui.add_space(theme::SPACE_MD);

                    let avail = ui.available_size();
                    let (rect, _) = ui.allocate_exact_size(avail, Sense::hover());
                    match preview_loaded {
                        Some((p, Ok(tex))) if p == &entry.path => {
                            let fit = fit_rect(rect, tex.size_vec2());
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
                                format!("Error al cargar imagen: {e}"),
                                egui::FontId::proportional(theme::FONT_BODY),
                                palette.text_secondary.color32(),
                            );
                        }
                        _ => {
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "Cargando imagen…",
                                egui::FontId::proportional(theme::FONT_BODY),
                                theme::DARK_PALETTE.text_secondary.color32(),
                            );
                        }
                    }
                });
        });

    selection_changed
}

pub fn fit_rect(bounds: Rect, content: Vec2) -> Rect {
    if content.x <= 0.0 || content.y <= 0.0 || bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return bounds;
    }
    let scale = (bounds.width() / content.x)
        .min(bounds.height() / content.y)
        .min(1.0);
    let size = content * scale;
    Rect::from_center_size(bounds.center(), size)
}

pub fn sort_label(sort: Sort) -> &'static str {
    match sort {
        Sort::PathAsc => "Ruta (A-Z)",
        Sort::PathDesc => "Ruta (Z-A)",
        Sort::SizeAsc => "Tamaño (menor)",
        Sort::SizeDesc => "Tamaño (mayor)",
        Sort::ModifiedAsc => "Fecha (antigua)",
        Sort::ModifiedDesc => "Fecha (reciente)",
    }
}
