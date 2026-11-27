use crate::model::{count_phrase, format_bytes, format_timestamp_utc, truncate_middle};
use crate::near_dupes_worker::NearDupesEvent;
use crate::theme::{self, Palette};
use crate::worker::{ThumbPool, ThumbRequest};
use catalog::{
    DEFAULT_NEAR_DUPLICATE_THRESHOLD, DuplicateGroup, Entry, NearDuplicateGroup, NearDuplicateMeta,
};
use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Sense, Stroke,
    TextureHandle, Vec2,
};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::Command;

const DUP_CARD_WIDTH: f32 = 260.0;
const DUP_THUMB_HEIGHT: f32 = 132.0;
const DUP_PATH_MAX_CHARS: usize = 38;

pub const READ_ONLY_NOTE: &str = "Nota: photosort solo muestra duplicados para revisión. Este programa nunca borra, mueve ni renombra ningún archivo.";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DuplicatesSummary {
    pub group_count: usize,
    pub total_copies: usize,
    pub redundant_copies: usize,
    pub wasted_bytes: u64,
}

pub fn group_wasted_bytes(group: &DuplicateGroup) -> u64 {
    group
        .size
        .saturating_mul(group.entries.len().saturating_sub(1) as u64)
}

pub fn summarize(groups: &[DuplicateGroup]) -> DuplicatesSummary {
    let mut summary = DuplicatesSummary {
        group_count: groups.len(),
        ..Default::default()
    };
    for g in groups {
        let copies = g.entries.len();
        summary.total_copies = summary.total_copies.saturating_add(copies);
        summary.redundant_copies = summary
            .redundant_copies
            .saturating_add(copies.saturating_sub(1));
        summary.wasted_bytes = summary.wasted_bytes.saturating_add(group_wasted_bytes(g));
    }
    summary
}

pub fn sort_groups(groups: &mut [DuplicateGroup]) {
    for g in groups.iter_mut() {
        g.entries.sort_by(|a, b| a.path.cmp(&b.path));
    }
    groups.sort_by(|a, b| {
        let wa = u128::from(a.size) * u128::from(a.entries.len().saturating_sub(1) as u64);
        let wb = u128::from(b.size) * u128::from(b.entries.len().saturating_sub(1) as u64);
        Reverse(wa)
            .cmp(&Reverse(wb))
            .then_with(|| a.sha256.cmp(&b.sha256))
    });
}

pub fn format_summary_line(summary: &DuplicatesSummary) -> String {
    format!(
        "{} · {} · {} de espacio duplicado",
        count_phrase(summary.group_count, "grupo", "grupos"),
        count_phrase(
            summary.redundant_copies,
            "copia redundante",
            "copias redundantes"
        ),
        format_bytes(summary.wasted_bytes),
    )
}

pub fn format_group_heading(group: &DuplicateGroup) -> String {
    format!(
        "{} · {} cada una · {} duplicados",
        count_phrase(group.entries.len(), "copia", "copias"),
        format_bytes(group.size),
        format_bytes(group_wasted_bytes(group)),
    )
}

/// Builds the arguments for `explorer.exe /select,<path>` without lossy UTF-8 conversion.
pub fn explorer_select_args(path: &Path) -> [OsString; 2] {
    [OsString::from("/select,"), path.as_os_str().to_os_string()]
}

/// Reveals `path` in Windows File Explorer without modifying any file.
pub fn reveal_in_explorer(path: &Path) -> io::Result<()> {
    let args = explorer_select_args(path);
    Command::new("explorer.exe").args(&args).spawn()?;
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DuplicatesSection {
    #[default]
    Exact,
    Near,
}

#[derive(Debug, Clone)]
pub struct DuplicatesViewState {
    pub section: DuplicatesSection,
    pub threshold: f32,
    pub running: Option<(u64, u64)>,
}

impl Default for DuplicatesViewState {
    fn default() -> Self {
        Self {
            section: DuplicatesSection::Exact,
            threshold: DEFAULT_NEAR_DUPLICATE_THRESHOLD,
            running: None,
        }
    }
}

impl DuplicatesViewState {
    pub fn apply_event(&mut self, ev: &NearDupesEvent) {
        match ev {
            NearDupesEvent::Progress {
                done_pairs,
                total_pairs,
            } => {
                self.running = Some((*done_pairs, *total_pairs));
            }
            NearDupesEvent::Finished { .. } | NearDupesEvent::Failed(_) => {
                self.running = None;
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct DuplicatesViewOutcome {
    pub error: Option<String>,
    pub thumbs_ready: bool,
    pub start_near_search: Option<f32>,
    pub cancel_near_search: bool,
    pub open_near_group_in_gallery: Option<usize>,
}

/// Renders the read-only duplicate review tab with separate Exact and Near-duplicate sections.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut DuplicatesViewState,
    groups: &[DuplicateGroup],
    near_groups: &[NearDuplicateGroup],
    near_meta: Option<NearDuplicateMeta>,
    vision_analyzed_count: usize,
    other_heavy: Option<&str>,
    thumb_pool: &ThumbPool,
    thumb_textures: &HashMap<String, TextureHandle>,
    thumb_failed: &HashSet<String>,
) -> DuplicatesViewOutcome {
    let summary = summarize(groups);
    let mut visible_hashes: HashSet<String> = HashSet::new();
    let mut to_request: Vec<ThumbRequest> = Vec::new();
    let mut outcome = DuplicatesViewOutcome::default();

    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(palette.bg_canvas.color32())
                .inner_margin(Margin::same(theme::SPACE_MD as i8)),
        )
        .show(ui, |ui| {
            // Top bar: section switcher (Exactos / Parecidos) + read-only guarantee notice.
            Frame::new()
                .fill(palette.bg_panel.color32())
                .stroke(Stroke::new(1.0, palette.border.color32()))
                .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                .inner_margin(Margin::same(theme::SPACE_MD as i8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        for (sec, label) in [
                            (
                                DuplicatesSection::Exact,
                                format!("Duplicados exactos ({})", groups.len()),
                            ),
                            (
                                DuplicatesSection::Near,
                                format!("Parecidos ({})", near_groups.len()),
                            ),
                        ] {
                            let active = state.section == sec;
                            let (bg, fg, stroke) = if active {
                                (
                                    palette.accent_bg.color32(),
                                    palette.text_primary.color32(),
                                    Stroke::new(1.5, palette.accent.color32()),
                                )
                            } else {
                                (
                                    palette.bg_card.color32(),
                                    palette.text_secondary.color32(),
                                    Stroke::new(1.0, palette.border.color32()),
                                )
                            };
                            let mut rt = RichText::new(label).size(theme::FONT_BODY).color(fg);
                            if active {
                                rt = rt.strong();
                            }
                            if ui
                                .add(
                                    egui::Button::new(rt)
                                        .fill(bg)
                                        .stroke(stroke)
                                        .corner_radius(CornerRadius::same(theme::RADIUS_SM)),
                                )
                                .clicked()
                            {
                                state.section = sec;
                            }
                        }
                        if state.section == DuplicatesSection::Exact {
                            ui.separator();
                            ui.label(
                                RichText::new(format_summary_line(&summary))
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                        }
                    });
                    ui.add_space(theme::SPACE_XS);
                    ui.label(
                        RichText::new(READ_ONLY_NOTE)
                            .size(theme::FONT_SMALL)
                            .color(palette.text_secondary.color32()),
                    );
                });

            ui.add_space(theme::SPACE_MD);

            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match state.section {
                    DuplicatesSection::Exact => {
                        if groups.is_empty() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(theme::SPACE_XL * 2.0);
                                ui.label(
                                    RichText::new(
                                        "No se encontraron duplicados exactos en el catálogo",
                                    )
                                    .size(theme::FONT_BODY)
                                    .color(palette.text_secondary.color32()),
                                );
                            });
                            return;
                        }
                        for (group_idx, group) in groups.iter().enumerate() {
                            Frame::new()
                                .fill(palette.bg_panel.color32())
                                .stroke(Stroke::new(1.0, palette.border.color32()))
                                .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                                .inner_margin(Margin::same(theme::SPACE_MD as i8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(
                                            RichText::new(format!(
                                                "Grupo {} — {}",
                                                group_idx + 1,
                                                format_group_heading(group)
                                            ))
                                            .size(theme::FONT_BODY)
                                            .strong(),
                                        );
                                        let short_sha = if group.sha256.len() > 12 {
                                            format!("SHA-256: {}…", &group.sha256[..12])
                                        } else {
                                            format!("SHA-256: {}", group.sha256)
                                        };
                                        ui.label(
                                            RichText::new(short_sha)
                                                .size(theme::FONT_SMALL)
                                                .monospace()
                                                .color(palette.text_secondary.color32()),
                                        )
                                        .on_hover_text(&group.sha256);
                                    });
                                    ui.add_space(theme::SPACE_SM);

                                    draw_entry_cards(
                                        ui,
                                        palette,
                                        &group.entries,
                                        &mut visible_hashes,
                                        &mut to_request,
                                        thumb_textures,
                                        thumb_failed,
                                        &mut outcome.error,
                                    );
                                });
                            ui.add_space(theme::SPACE_SM);
                        }
                    }
                    DuplicatesSection::Near => {
                        Frame::new()
                            .fill(palette.bg_panel.color32())
                            .stroke(Stroke::new(1.0, palette.border.color32()))
                            .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                            .inner_margin(Margin::same(theme::SPACE_MD as i8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        RichText::new("Parecidos")
                                            .size(theme::FONT_HEADING)
                                            .strong(),
                                    );
                                    ui.separator();
                                    let summary_txt = match near_meta {
                                        Some(meta) => format!(
                                            "{} · umbral {:.2}",
                                            count_phrase(near_groups.len(), "grupo", "grupos"),
                                            meta.threshold
                                        ),
                                        None => "Sin búsqueda guardada todavía".to_string(),
                                    };
                                    ui.label(
                                        RichText::new(summary_txt)
                                            .size(theme::FONT_BODY)
                                            .color(palette.text_secondary.color32()),
                                    );
                                });

                                ui.add_space(theme::SPACE_SM);

                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        RichText::new("Umbral de similitud")
                                            .size(theme::FONT_SMALL)
                                            .color(palette.text_secondary.color32()),
                                    );
                                    theme::named(
                                        ui.add_enabled(
                                            state.running.is_none(),
                                            egui::Slider::new(&mut state.threshold, 0.80..=0.99)
                                                .step_by(0.01)
                                                .fixed_decimals(2),
                                        ),
                                        "Umbral de similitud",
                                    );

                                    if let Some((done, total)) = state.running {
                                        let frac = if total > 0 {
                                            (done as f32 / total as f32).clamp(0.0, 1.0)
                                        } else {
                                            0.0
                                        };
                                        ui.add(
                                            egui::ProgressBar::new(frac)
                                                .desired_width(180.0)
                                                .text(format!("{:.0} %", frac * 100.0)),
                                        );
                                        if ui.button("Cancelar").clicked() {
                                            outcome.cancel_near_search = true;
                                        }
                                    } else {
                                        let can_run =
                                            other_heavy.is_none() && vision_analyzed_count >= 2;
                                        let btn = ui.add_enabled(
                                            can_run,
                                            theme::primary_button(palette, "Buscar parecidos"),
                                        );
                                        if let Some(task) = other_heavy {
                                            btn.on_disabled_hover_text(format!(
                                                "Espera a que termine {task}"
                                            ));
                                        } else if vision_analyzed_count < 2 {
                                            btn.on_disabled_hover_text(
                                                "Se necesitan al menos 2 fotos con reconocimiento visual",
                                            );
                                        } else if btn.clicked() {
                                            outcome.start_near_search = Some(state.threshold);
                                        }
                                    }
                                });

                                if let Some(meta) = near_meta
                                    && (meta.threshold - state.threshold).abs() >= 0.005
                                    && state.running.is_none()
                                {
                                    ui.add_space(theme::SPACE_XS);
                                    ui.label(
                                        RichText::new(
                                            "El umbral ha cambiado; pulsa «Buscar parecidos» para recalcular",
                                        )
                                        .size(theme::FONT_SMALL)
                                        .color(palette.text_hint.color32()),
                                    );
                                }
                            });

                        ui.add_space(theme::SPACE_SM);

                        if near_groups.is_empty() {
                            let empty_msg = if vision_analyzed_count < 2 {
                                "Aún no hay suficientes fotos con reconocimiento visual para buscar parecidos"
                            } else if near_meta.is_none() {
                                "Pulsa «Buscar parecidos» para comparar los vectores visuales guardados"
                            } else {
                                "No se encontraron grupos parecidos con este umbral"
                            };
                            ui.label(
                                RichText::new(empty_msg)
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                        } else {
                            for (group_idx, group) in near_groups.iter().enumerate() {
                                Frame::new()
                                    .fill(palette.bg_panel.color32())
                                    .stroke(Stroke::new(1.0, palette.border.color32()))
                                    .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                                    .inner_margin(Margin::same(theme::SPACE_MD as i8))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(format!(
                                                    "Grupo {} — {} · similitud {:.1} %",
                                                    group_idx + 1,
                                                    count_phrase(
                                                        group.entries.len(),
                                                        "foto",
                                                        "fotos"
                                                    ),
                                                    group.similarity * 100.0,
                                                ))
                                                .size(theme::FONT_BODY)
                                                .strong(),
                                            );
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    if ui.button("Ver en galería").clicked() {
                                                        outcome.open_near_group_in_gallery =
                                                            Some(group_idx);
                                                    }
                                                },
                                            );
                                        });
                                        ui.add_space(theme::SPACE_SM);

                                        draw_entry_cards(
                                            ui,
                                            palette,
                                            &group.entries,
                                            &mut visible_hashes,
                                            &mut to_request,
                                            thumb_textures,
                                            thumb_failed,
                                            &mut outcome.error,
                                        );
                                    });
                                ui.add_space(theme::SPACE_SM);
                            }
                        }
                    }
                });
        });

    let active_empty = match state.section {
        DuplicatesSection::Exact => groups.is_empty(),
        DuplicatesSection::Near => near_groups.is_empty(),
    };
    outcome.thumbs_ready = active_empty || (!visible_hashes.is_empty() && to_request.is_empty());

    thumb_pool.set_visible(&visible_hashes);
    for req in to_request {
        thumb_pool.request(req);
    }

    outcome
}

#[allow(clippy::too_many_arguments)]
fn draw_entry_cards(
    ui: &mut egui::Ui,
    palette: &Palette,
    entries: &[Entry],
    visible_hashes: &mut HashSet<String>,
    to_request: &mut Vec<ThumbRequest>,
    thumb_textures: &HashMap<String, TextureHandle>,
    thumb_failed: &HashSet<String>,
    open_error: &mut Option<String>,
) {
    ui.with_layout(
        egui::Layout::left_to_right(egui::Align::TOP).with_main_wrap(true),
        |ui| {
            for entry in entries {
                visible_hashes.insert(entry.sha256.clone());
                if !thumb_textures.contains_key(&entry.sha256)
                    && !thumb_failed.contains(&entry.sha256)
                {
                    to_request.push(ThumbRequest {
                        source: entry.path.clone(),
                        sha256: entry.sha256.clone(),
                    });
                }

                Frame::new()
                    .fill(palette.bg_card.color32())
                    .stroke(Stroke::new(1.0, palette.border.color32()))
                    .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                    .inner_margin(Margin::same(theme::SPACE_SM as i8))
                    .show(ui, |ui| {
                        // the row lays out left to right; a card must stack its content
                        ui.vertical(|ui| {
                            ui.set_width(DUP_CARD_WIDTH);
                            let (thumb_rect, _) = ui.allocate_exact_size(
                                Vec2::new(DUP_CARD_WIDTH, DUP_THUMB_HEIGHT),
                                Sense::hover(),
                            );
                            draw_thumb_box(
                                ui,
                                palette,
                                thumb_rect,
                                thumb_textures.get(&entry.sha256),
                                thumb_failed.contains(&entry.sha256),
                            );
                            ui.add_space(theme::SPACE_XS);

                            let filename = entry
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| entry.path.display().to_string());
                            let full_path = entry.path.display().to_string();
                            ui.label(
                                RichText::new(truncate_middle(&filename, DUP_PATH_MAX_CHARS))
                                    .size(theme::FONT_BODY)
                                    .strong(),
                            )
                            .on_hover_text(&full_path);
                            let folder = entry
                                .path
                                .parent()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default();
                            ui.label(
                                RichText::new(truncate_middle(&folder, DUP_PATH_MAX_CHARS))
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            )
                            .on_hover_text(&full_path);

                            let mut meta_parts = Vec::new();
                            if let (Some(w), Some(h)) = (entry.width, entry.height) {
                                meta_parts.push(format!("{w}×{h}"));
                            }
                            meta_parts.push(format_bytes(entry.size));
                            if let Some(ns) = entry.modified_ns {
                                meta_parts.push(format_timestamp_utc(ns));
                            }
                            ui.label(
                                RichText::new(meta_parts.join(" · "))
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                            ui.add_space(theme::SPACE_XS);

                            if ui.button("Mostrar en el Explorador").clicked()
                                && let Err(e) = reveal_in_explorer(&entry.path)
                            {
                                *open_error = Some(format!("No se pudo abrir el Explorador: {e}"));
                            }
                        });
                    });
            }
        },
    );
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
